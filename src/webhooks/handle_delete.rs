use chrono::{Duration, SecondsFormat, Utc};
use eyre::{Context, Result, eyre};
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::AdmissionRequest;

use crate::patch::patch_pod_isolate;
use crate::pod_draining_info::{PodDrainingInfo, get_pod_draining_info};
use crate::pod_state::{is_pod_exposed, is_pod_ready};
use crate::webhooks::AppState;
use crate::webhooks::handle_common::InterceptResult;
use crate::webhooks::report::{debug_report_for, report_for};

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
) -> Result<InterceptResult> {
    let pod = request
        .old_object
        .as_ref()
        .ok_or(eyre!("old_object for validation is missing"))?;

    if pod.metadata.deletion_timestamp.is_some() {
        debug_report_for(
            state,
            pod,
            "AllowDeletion",
            "AlreadyDeleted",
            "Pod already have 'deletionTimestamp' on it".to_string(),
        )
        .await;
        return Ok(InterceptResult::Allow);
    }

    let draining = get_pod_draining_info(pod);
    match draining {
        PodDrainingInfo::None => {
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

            let drain_until = Utc::now() + Duration::from_std(state.config.delete_after)?;

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
