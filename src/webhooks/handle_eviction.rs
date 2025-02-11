use std::default::Default;

use chrono::{Duration, SecondsFormat, Utc};
use eyre::{eyre, Context, Result};
use k8s_openapi::api::authentication::v1::UserInfo;
use k8s_openapi::api::core::v1::{ObjectReference, Pod};
use k8s_openapi::api::policy::v1::Eviction;
use kube::api::{EvictParams, PostParams, Request};
use kube::client::Status;
use kube::core::admission::{AdmissionRequest, AdmissionResponse};
use kube::{Api, ResourceExt};

use crate::impersonate::impersonate;
use crate::pod_draining_info::{get_pod_draining_info, PodDrainingInfo};
use crate::pod_state::{is_pod_exposed, is_pod_ready};
use crate::status::{is_404_not_found_error, is_410_gone_error};
use crate::utils::{get_object_ref_from_name, to_delete_params};
use crate::webhooks::patch::make_patch_eviction_to_dry_run;
use crate::webhooks::report::{debug_report_for, report_for};
use crate::webhooks::{debug_report_for_ref, patch_pod_isolate, AppState, InterceptResult};
use crate::{try_some, ApiResolver};

/// The handler patches CREATE Eviction request as dry-run.
/// The controller will delete them later anyhow.
///
/// # Compatibility
///
/// The handler cannot deny the admission request due to the following compatibility reasons.
///
/// * `kubectl drain`: fail and stop if it meets the first pod that cannot be deleted.
pub async fn eviction_handler(
    state: &AppState,
    request: &AdmissionRequest<Eviction>,
    user_info: &UserInfo,
) -> Result<InterceptResult> {
    let eviction = request
        .object
        .as_ref()
        .ok_or(eyre!("object for mutation is missing"))?;

    let object_ref = get_object_ref_from_name(&request.name, request.namespace.as_ref());
    if let Some(dry_run) = try_some!(eviction.delete_options?.dry_run?) {
        if !dry_run.is_empty() {
            debug_report_for_ref(
                state,
                ObjectReference::from(object_ref),
                "AllowEviction",
                "DryRun",
                format!("Eviction request is allowed because `eviction.deleteOptions.dryRun = {dry_run:?}`"),
            )
                .await;
            return Ok(InterceptResult::Allow);
        }
    }

    let pod = state
        .stores
        .get_pod(&object_ref)
        .ok_or(eyre!("pod is not found"))?;

    let draining = get_pod_draining_info(&pod);
    match draining {
        PodDrainingInfo::None => {
            if !is_pod_exposed(&state.config, &state.stores, &pod) {
                debug_report_for(
                    state,
                    &pod,
                    "AllowEviction",
                    "NotExposed",
                    "Eviction is allowed because the pod is not exposed".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            if !is_pod_ready(&pod) {
                debug_report_for(
                    state,
                    &pod,
                    "AllowEviction",
                    "NotReady",
                    "Eviction is allowed because the pod is not ready".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            let drain_until = Utc::now() + Duration::from_std(state.config.delete_after)?;

            if let EvictionPermissionCheckResult::Gone =
                check_eviction_permission(&state.api_resolver, &pod, eviction, user_info)
                    .await
                    .context("checking permission")?
            {
                debug_report_for(
                    state,
                    &pod,
                    "AllowEviction",
                    "Gone",
                    "Pod is already gone".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            let patched_result = patch_pod_isolate(
                &state.api_resolver,
                &pod,
                drain_until,
                Some(&eviction.delete_options.clone().unwrap_or_default()),
                &state.loadbalancing,
            )
            .await
            .context("apply patch")?;

            if patched_result.is_none() {
                debug_report_for(
                    state,
                    &pod,
                    "AllowDeletion",
                    "Gone",
                    "Pod is already gone".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            report_for(
                state,
                &pod,
                "InterceptEviction",
                "Drain",
                format!(
                    "Eviction is intercepted, and the pod is isolated. It'll be deleted after '{}'",
                    drain_until.to_rfc3339_opts(SecondsFormat::Secs, true),
                ),
            )
            .await;
        }
        PodDrainingInfo::DrainUntil(drain_until) => {
            if Utc::now() > drain_until {
                debug_report_for(
                    state,
                    &pod,
                    "AllowEviction",
                    "Expired",
                    "Eviction is allowed because the pod is drained enough".to_string(),
                )
                .await;

                return Ok(InterceptResult::Allow);
            }

            report_for(
                state,
                &pod,
                "InterceptEviction",
                "Draining",
                format!(
                    "Eviction is intercepted. It'll be deleted after '{}'",
                    drain_until.to_rfc3339_opts(SecondsFormat::Secs, true),
                ),
            )
            .await;
        }
        PodDrainingInfo::Deleted => {
            debug_report_for(
                state,
                &pod,
                "AllowEviction",
                "AlreadyDeleted",
                "Pod already have 'deletionTimestamp' on it".to_string(),
            )
            .await;

            return Ok(InterceptResult::Allow);
        }
        PodDrainingInfo::DrainDisabled => {
            debug_report_for(
                state,
                &pod,
                "InterceptEviction",
                "Disabled",
                "Pod graceful drain is disabled".to_string(),
            )
            .await;
            return Ok(InterceptResult::Allow);
        }
        PodDrainingInfo::AnnotationParseError { message } => {
            return Err(eyre!(message));
        }
    };

    let eviction_patch = make_patch_eviction_to_dry_run(eviction).context("patch")?;
    let response = AdmissionResponse::from(request)
        .with_patch(eviction_patch)
        .context("attaching patch")?;

    Ok(InterceptResult::Patch(Box::new(response)))
}

#[must_use]
enum EvictionPermissionCheckResult {
    Ok,
    Gone,
}

async fn check_eviction_permission(
    api_resolver: &ApiResolver,
    pod: &Pod,
    eviction: &Eviction,
    user_info: &UserInfo,
) -> Result<EvictionPermissionCheckResult> {
    // we might've checked it using `SubjectAccessReview`,
    // but there might be other custom webhooks that implements custom access control.
    // so we dry-run delete to check them.
    let api: Api<Pod> = api_resolver.api_for(pod);

    let name = eviction.name_any();
    let delete_params =
        to_delete_params(eviction.delete_options.clone().unwrap_or_default(), true)?;
    let evict_params = EvictParams {
        delete_options: Some(delete_params),
        post_options: PostParams {
            dry_run: true,
            ..PostParams::default()
        },
    };

    let mut req = Request::new(api.resource_url())
        .evict(&name, &evict_params)
        .context("building request")?;
    req.extensions_mut().insert("evict-impersonated"); // for otel trace
    impersonate(&mut req, user_info)?;

    match api.into_client().request::<Status>(req).await {
        Ok(_) => Ok(EvictionPermissionCheckResult::Ok),
        Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
            Ok(EvictionPermissionCheckResult::Gone)
        }
        Err(err) => Err(err.into()),
    }
}
