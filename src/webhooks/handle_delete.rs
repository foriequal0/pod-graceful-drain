use std::time::Duration;

use chrono::{DateTime, Utc};
use eyre::{Context, Result, eyre};
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::AdmissionRequest;

use crate::labels_and_annotations::{
    DrainingLabelValue, get_pod_drain_timestamp, get_pod_draining_label_value,
};
use crate::patch::drain::{PatchToDrainCaller, PatchToDrainOutcome, patch_to_drain};
use crate::pod_state::{is_pod_exposed, is_pod_ready, is_pod_running};
use crate::report::{debug_report_for, report_for};
use crate::webhooks::AppState;
use crate::webhooks::handle_common::InterceptResult;

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
///   so it is controlled by ReplicaSet controller.
pub async fn handle_delete(
    state: &AppState,
    request: &AdmissionRequest<Pod>,
    timeout: Duration,
) -> Result<InterceptResult> {
    const DEADLINE_OFFSET: Duration = Duration::from_secs(5);

    let started_at = Utc::now();
    let deadline = started_at + timeout - DEADLINE_OFFSET;

    let pod = request
        .old_object
        .as_ref()
        .ok_or(eyre!("old_object for validation is missing"))?;

    if !is_pod_running(pod) {
        debug_report_for(
            &state.recorder,
            pod,
            "AllowDeletion",
            "AlreadyTerminated",
            "Pod is not running".to_string(),
        )
        .await;
        return Ok(InterceptResult::Allow);
    }

    let draining_label_value = get_pod_draining_label_value(pod);
    match draining_label_value {
        // first delete request
        Ok(None)
        // eviction requested then deletion requested
        | Ok(Some(DrainingLabelValue::Evicting)) => {
            if !is_pod_exposed(&state.config, &state.stores, pod) {
                debug_report_for(
                    &state.recorder,
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
                    &state.recorder,
                    pod,
                    "AllowDeletion",
                    "NotReady",
                    "Deletion is allowed because the pod is not ready".to_string(),
                )
                    .await;
                return Ok(InterceptResult::Allow);
            }

            let outcome = patch_to_drain(
                pod,
                &state.api_resolver,
                &state.loadbalancing,
                PatchToDrainCaller::Webhook,
            )
                .await
                .context("patch")?;

            let drain_until = match outcome {
                PatchToDrainOutcome::Gone => {
                    debug_report_for(
                    &state.recorder,
                        pod,
                        "AllowDeletion",
                        "Gone",
                        "Pod is already gone".to_string(),
                    )
                        .await;
                    return Ok(InterceptResult::Allow);
                }
                PatchToDrainOutcome::Draining { drain_timestamp } => {
                    // TODO: precisely wait until deleted
                    drain_timestamp + state.config.delete_after
                }
            };

            report_for(
                &state.recorder,
                pod,
                "DelayDeletion",
                "Drain",
                String::from(
                    "Deletion is delayed, and the pod is deregistering. It'll be deleted soon",
                ),
            )
                .await;

            drain_until_or_deadline(state, pod, drain_until, deadline).await;
            Ok(InterceptResult::Allow)
        }
        Ok(Some(DrainingLabelValue::Draining)) => {
            if let Ok(Some(drain_timestamp)) = get_pod_drain_timestamp(pod) {
                // TODO: precisely wait until deleted
                let drain_until = drain_timestamp + state.config.delete_after;
                if Utc::now() < drain_until {
                    report_for(
                        &state.recorder,
                        pod,
                        "DelayDeletion",
                        "Draining",
                        "Deletion is delayed, it'll be deleted soon".to_owned(),
                    )
                        .await;

                    drain_until_or_deadline(state, pod, drain_until, deadline).await;
                    Ok(InterceptResult::Allow)
                } else {
                    debug_report_for(
                        &state.recorder,
                        pod,
                        "AllowDeletion",
                        "Expired",
                        "Deletion is allowed because the pod is drained enough".to_string(),
                    )
                        .await;

                    Ok(InterceptResult::Allow)
                }
            } else {
                debug_report_for(
                    &state.recorder,
                    pod,
                    "AllowDeletion",
                    "Manual",
                    "Deletion is allowed, manual resolve".to_string(),
                )
                    .await;

                // TODO: report error, or recover error
                Ok(InterceptResult::Allow)
            }
        }
        Err(_) => {
            debug_report_for(
                &state.recorder,
                pod,
                "AllowDeletion",
                "Manual",
                "Deletion is allowed, manual resolve".to_string(),
            )
                .await;

            Ok(InterceptResult::Allow)
        }
    }
}

/// I could've returned 429 TOO_MANY_REQUESTS with `retry-after` instead.
/// But this is much easier with the same result.
async fn drain_until_or_deadline(
    state: &AppState,
    pod: &Pod,
    drain_until: DateTime<Utc>,
    deadline: DateTime<Utc>,
) {
    if drain_until < deadline {
        let to_sleep = (drain_until - Utc::now()).to_std().unwrap_or_default();

        tokio::time::sleep(to_sleep).await;

        report_for(
            &state.recorder,
            pod,
            "AllowDeletion",
            "Drained",
            "Deletion is allowed because the pod is drained enough".to_string(),
        )
        .await;
    } else {
        let to_sleep = (deadline - Utc::now()).to_std().unwrap_or_default();
        tokio::time::sleep(to_sleep).await;

        report_for(
            &state.recorder,
            pod,
            "AllowDeletion",
            "Timeout",
            "Deletion is allowed because admission webhook timeout".to_string(),
        )
        .await;
    }
}
