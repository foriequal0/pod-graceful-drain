use eyre::{Context, Result, eyre};
use k8s_openapi::api::policy::v1::Eviction;
use kube::core::admission::{AdmissionRequest, AdmissionResponse};

use crate::labels_and_annotations::{
    DRAINING_LABEL_KEY, DrainingLabelValue, get_pod_draining_label_value,
};
use crate::patch::evict::{PatchToEvictOutcome, patch_to_evict};
use crate::patch::eviction_admission::make_patch_eviction_to_dry_run;
use crate::pod_state::{is_pod_exposed, is_pod_ready, is_pod_running};
use crate::report::{debug_report_for, report_for, warn_report_for};
use crate::try_some;
use crate::utils::get_object_ref_from_name;
use crate::webhooks::handle_common::InterceptResult;
use crate::webhooks::{AppState, debug_report_for_ref};

/// The handler patches CREATE Eviction request as dry-run.
/// The controller will delete them later anyhow.
///
/// # Compatibility
///
/// The handler cannot deny the admission request due to the following compatibility reasons.
///
/// * `kubectl drain`: fail and stop if it meets the first pod that cannot be deleted.
pub async fn handle_eviction(
    state: &AppState,
    request: &AdmissionRequest<Eviction>,
) -> Result<InterceptResult> {
    let eviction = request
        .object
        .as_ref()
        .ok_or(eyre!("object for mutation is missing"))?;

    let object_ref = get_object_ref_from_name(&request.name, request.namespace.as_ref());
    if let Some(dry_run) = try_some!(eviction.delete_options?.dry_run?) {
        if !dry_run.is_empty() {
            debug_report_for_ref(
                &state.recorder,
                &object_ref,
                "Allow",
                "DryRun",
                format!("Eviction request is allowed because `eviction.deleteOptions.dryRun = {dry_run:?}`"),
            )
                .await;
            return Ok(InterceptResult::Allow);
        }
    }

    let Some(pod) = state.stores.get_pod(&object_ref) else {
        debug_report_for_ref(
            &state.recorder,
            &object_ref,
            "AllowEviction",
            "Gone",
            "Pod is already gone".to_string(),
        )
        .await;
        return Ok(InterceptResult::Allow);
    };

    if !is_pod_running(&pod) {
        debug_report_for(
            &state.recorder,
            &pod,
            "AllowEviction",
            "AlreadyTerminated",
            "Pod is not running".to_string(),
        )
        .await;
        return Ok(InterceptResult::Allow);
    }

    let draining_label_value = get_pod_draining_label_value(&pod);
    match draining_label_value {
        Ok(None) => {
            if !is_pod_exposed(&state.config, &state.stores, &pod) {
                debug_report_for(
                    &state.recorder,
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
                    &state.recorder,
                    &pod,
                    "AllowEviction",
                    "NotReady",
                    "Eviction is allowed because the pod is not ready".to_string(),
                )
                .await;
                return Ok(InterceptResult::Allow);
            }

            let patch_result = patch_to_evict(
                &pod,
                &state.api_resolver,
                &state.loadbalancing,
                &eviction.delete_options.clone().unwrap_or_default(),
            )
            .await
            .context("patch")?;

            match patch_result {
                PatchToEvictOutcome::Gone => {
                    debug_report_for(
                        &state.recorder,
                        &pod,
                        "AllowEviction",
                        "Gone",
                        "Pod is already gone".to_string(),
                    )
                    .await;

                    return Ok(InterceptResult::Allow);
                }
                PatchToEvictOutcome::Draining => {
                    report_for(
                        &state.recorder,
                        &pod,
                        "InterceptEviction",
                        "Draining",
                        "Eviction is intercepted, pod is draining now.".to_string(),
                    )
                    .await;
                }
                PatchToEvictOutcome::WaitingForPodDisruptionBudget => {
                    report_for(
                        &state.recorder,
                        &pod,
                        "InterceptEviction",
                        "WaitingForPodDisruptionBudget",
                        "Eviction is intercepted, pod is waiting for pod disruption budget"
                            .to_string(),
                    )
                    .await;
                }
            };

            Ok(intercept_eviction(request, eviction)?)
        }
        // eviction requested multiple times
        Ok(Some(DrainingLabelValue::Evicting)) => {
            report_for(
                &state.recorder,
                &pod,
                "InterceptEviction",
                "WaitingForPodDisruptionBudget",
                "Eviction is intercepted, pod is waiting for pod disruption budget".to_string(),
            )
            .await;

            Ok(intercept_eviction(request, eviction)?)
        }
        // deletion requested then eviction requested
        Ok(Some(DrainingLabelValue::Draining)) => {
            report_for(
                &state.recorder,
                &pod,
                "InterceptEviction",
                "Draining",
                "Eviction is intercepted, It'll be deleted later".to_string(),
            )
            .await;

            Ok(intercept_eviction(request, eviction)?)
        }
        Err(other) => {
            warn_report_for(
                &state.recorder,
                &pod,
                "DenyEviction",
                "InvalidLabel",
                format!("Invalid value for label '{DRAINING_LABEL_KEY}': {other}"),
            )
            .await;

            Ok(InterceptResult::Deny(format!(
                "Invalid value for label '{DRAINING_LABEL_KEY}': {other}"
            )))
        }
    }
}

fn intercept_eviction(
    request: &AdmissionRequest<Eviction>,
    eviction: &Eviction,
) -> Result<InterceptResult> {
    let eviction_patch = make_patch_eviction_to_dry_run(eviction).context("patch")?;
    let response = AdmissionResponse::from(request)
        .with_patch(eviction_patch)
        .context("attaching patch")?;

    Ok(InterceptResult::Patch(Box::new(response)))
}
