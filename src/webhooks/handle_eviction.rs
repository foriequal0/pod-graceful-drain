use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use eyre::{Context, Result, eyre};
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::api::policy::v1::Eviction;
use kube::core::admission::{AdmissionRequest, AdmissionResponse};

use crate::patch::{make_patch_eviction_to_dry_run, patch_pod_isolate};
use crate::pod_draining_info::{PodDrainingInfo, get_pod_draining_info};
use crate::pod_state::{is_pod_exposed, is_pod_ready};
use crate::try_some;
use crate::utils::get_object_ref_from_name;
use crate::webhooks::handle_common::InterceptResult;
use crate::webhooks::report::{debug_report_for, report_for};
use crate::webhooks::{AppState, debug_report_for_ref};

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
    _timeout: Duration,
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

    if pod.metadata.deletion_timestamp.is_some() {
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

    let draining = get_pod_draining_info(&pod);
    match draining {
        PodDrainingInfo::None => {
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

            let drain_until = Utc::now() + state.config.delete_after;

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
                    "Eviction is intercepted, and the pod is isolated. It'll be evicted later ({})",
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
                    "Eviction is intercepted. It'll be evicted later ({})",
                    drain_until.to_rfc3339_opts(SecondsFormat::Secs, true),
                ),
            )
            .await;
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
