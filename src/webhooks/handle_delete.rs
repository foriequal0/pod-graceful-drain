use chrono::{Duration, SecondsFormat, Utc};
use eyre::{eyre, Context, Result};
use k8s_openapi::api::authentication::v1::UserInfo;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use k8s_openapi::apimachinery::pkg::runtime::RawExtension;
use kube::core::admission::AdmissionRequest;
use kube::ResourceExt;
use serde::Deserialize;

use crate::pod_draining_info::{get_pod_draining_info, PodDrainingInfo};
use crate::pod_state::{is_pod_exposed, is_pod_ready};
use crate::status::{is_404_not_found_error, is_410_gone_error};
use crate::utils::to_delete_params;
use crate::webhooks::report::{debug_report_for, report_for};
use crate::webhooks::{patch_pod_isolate, AppState, InterceptResult};
use crate::ApiResolver;

/// This handler delays the admission of DELETE Pod request.
///
/// We can't patch out the DELETE Pod request, so we delay it.
///
/// # Compatibility
///
/// The handler cannot deny the request due to the following compatibility reasons.
///
/// * `kubectl drain --disable-eviction`: fail and stop if it meets the first pod that cannot be deleted.
/// * `kubectl delete`: returns non-zero exit code and prints the reason of denial.
///   Human operators might be able to read the reason, but machines don't.
///   We might break some existing tools that wraps `kubectl delete` if we deny the request.
///
/// These are known to be fine with the admission request denial.
///
/// * ReplicaSet controller: it can retry and progress.
/// * `kubectl rollout restart`: It patches the deployment's annotation `kubectl.kubernetes.io/restartedAt`,
///    so it is controlled by ReplicaSet controller.
pub async fn delete_handler(
    state: &AppState,
    request: &AdmissionRequest<Pod>,
    user_info: &UserInfo,
) -> Result<InterceptResult> {
    let pod = request
        .old_object
        .as_ref()
        .ok_or(eyre!("old_object for validation is missing"))?;

    match get_pod_draining_info(pod) {
        PodDrainingInfo::None => {
            if !is_pod_exposed(&state.config, &state.stores, pod) {
                debug_report_for(
                    state,
                    pod,
                    "AllowDeletion",
                    "NotExposed",
                    "Deletion is allowed because the pod is not exposed".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            if !is_pod_ready(pod) {
                debug_report_for(
                    state,
                    pod,
                    "AllowDeletion",
                    "NotReady",
                    "Deletion is allowed because the pod is not ready".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            let drain_until = Utc::now() + Duration::from_std(state.config.delete_after)?;

            if let DeletePermissionCheckResult::Gone =
                check_delete_permission(&state.api_resolver, pod, &request.options, user_info)
                    .await
                    .context("checking permission")?
            {
                debug_report_for(
                    state,
                    pod,
                    "AllowDeletion",
                    "Gone",
                    "Pod is already gone".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            let patched_result = patch_pod_isolate(
                &state.api_resolver,
                pod,
                drain_until,
                None,
                &state.loadbalancing,
            )
            .await
            .context("apply patch")?;

            if patched_result.is_none() {
                debug_report_for(
                    state,
                    pod,
                    "AllowDeletion",
                    "Gone",
                    "Pod is already gone".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            report_for(
                state,
                pod,
                "DelayDeletion",
                "Drain",
                format!(
                    "Deletion is delayed, and the pod is isolated. It'll be deleted after '{}'",
                    drain_until.to_rfc3339_opts(SecondsFormat::Secs, true),
                ),
            )
            .await;

            let duration = (drain_until - Utc::now()).to_std().unwrap_or_default();
            Ok(InterceptResult::Delay(duration))
        }
        PodDrainingInfo::DrainUntil(drain_until) => {
            if let Ok(duration) = (drain_until - Utc::now()).to_std() {
                report_for(
                    state,
                    pod,
                    "DelayDeletion",
                    "Draining",
                    format!(
                        "Deletion is delayed. It'll be deleted after '{}'",
                        drain_until.to_rfc3339_opts(SecondsFormat::Secs, true),
                    ),
                )
                .await;

                Ok(InterceptResult::Delay(duration))
            } else {
                debug_report_for(
                    state,
                    pod,
                    "AllowDeletion",
                    "Expired",
                    "Deletion is allowed because the pod is drained enough".to_string(),
                )
                .await;

                Ok(InterceptResult::Allow)
            }
        }
        PodDrainingInfo::Deleted => Ok(InterceptResult::Allow),
        PodDrainingInfo::DrainDisabled => {
            debug_report_for(
                state,
                pod,
                "AllowDeletion",
                "Disabled",
                "Pod graceful drain is disabled".to_string(),
            )
            .await;

            Ok(InterceptResult::Allow)
        }
        PodDrainingInfo::AnnotationParseError { message } => Err(eyre!(message)),
    }
}

#[must_use]
enum DeletePermissionCheckResult {
    Ok,
    Gone,
}

async fn check_delete_permission(
    api_resolver: &ApiResolver,
    pod: &Pod,
    raw_options: &Option<RawExtension>,
    user_info: &UserInfo,
) -> Result<DeletePermissionCheckResult> {
    // we might've checked it using `SubjectAccessReview`,
    // but there might be other custom webhooks that implements custom access control.
    // so we dry-run delete to check them.
    let api = api_resolver.impersonate_as(user_info)?.api_for(pod);

    let delete_options = if let Some(delete_options) = raw_options {
        DeleteOptions::deserialize(&delete_options.0)?
    } else {
        DeleteOptions::default()
    };

    let name = pod.name_any();
    let delete_params = to_delete_params(delete_options, true)?;
    match api.delete(&name, &delete_params).await {
        Ok(_) => Ok(DeletePermissionCheckResult::Ok),
        Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
            Ok(DeletePermissionCheckResult::Gone)
        }
        Err(err) => Err(err.into()),
    }
}
