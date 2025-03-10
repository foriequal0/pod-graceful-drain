use chrono::{DateTime, Timelike, Utc};
use eyre::Result;
use k8s_openapi::api::core::v1::Pod;
use thiserror::Error;

use crate::LoadBalancingConfig;
use crate::api_resolver::ApiResolver;
use crate::error_types::Bug;
use crate::labels_and_annotations::{
    DrainingLabelValue, get_pod_draining_label_value, get_pod_evict_after,
    set_pod_drain_controller, set_pod_evict_after,
};
use crate::patch::evict::PatchToEvictOutcome;
use crate::patch::resource_patch_util::{MutationOutcome, ResourcePatchError, patch};

#[derive(Debug, Error)]
pub enum PatchToEvictLaterError {
    #[error("failed to patch pod")]
    ResourcePatchError(#[from] ResourcePatchError),
    #[error("pod has unknown label: {label:?}")]
    PodDrainingStateIsInvalid { label: String },
    #[error(transparent)]
    Bug(#[from] Bug),
}

pub async fn patch_to_evict_later(
    pod: &Pod,
    timestamp: DateTime<Utc>,
    api_resolver: &ApiResolver,
    loadbalancing: &LoadBalancingConfig,
) -> Result<PatchToEvictOutcome, PatchToEvictLaterError> {
    patch(api_resolver, pod, |pod| {
        mutate_to_evict_later(pod, timestamp, loadbalancing)
    })
    .await
}

fn mutate_to_evict_later(
    pod: Option<&Pod>,
    evict_after: DateTime<Utc>,
    loadbalancing: &LoadBalancingConfig,
) -> Result<MutationOutcome<PatchToEvictOutcome, Pod>, PatchToEvictLaterError> {
    let Some(pod) = pod else {
        return Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Gone));
    };

    let trimmed = evict_after
        .with_nanosecond(0)
        .expect("shouldn't panic since 'nano' < 2,000,000,000");
    let draining_state = get_pod_draining_label_value(pod);
    match draining_state {
        Ok(Some(DrainingLabelValue::Evicting)) => {
            if let Ok(Some(existing_evict_after)) = get_pod_evict_after(pod) {
                if existing_evict_after >= trimmed {
                    return Ok(MutationOutcome::DesiredState(
                        PatchToEvictOutcome::WaitingForPodDisruptionBudget,
                    ));
                }
            }

            let mut pod = pod.clone();

            set_pod_drain_controller(&mut pod, loadbalancing);
            set_pod_evict_after(&mut pod, Some(evict_after));

            Ok(MutationOutcome::RequirePatch(pod))
        }
        Ok(Some(DrainingLabelValue::Draining)) => {
            Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Draining))
        }
        Ok(None) => Err(Bug {
            message: String::from("pod is not waiting for evicting"),
            source: None,
        }
        .into()),
        Err(label) => Err(PatchToEvictLaterError::PodDrainingStateIsInvalid { label }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::from_json;
    use crate::patch::drain;
    use crate::patch::evict::mutate_to_evict;
    use chrono::{DateTime, Utc};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;

    #[test]
    fn smoke_test() {
        let pod: Pod = from_json!({});

        let timestamp1 = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let timestamp2 = DateTime::parse_from_rfc3339("2025-03-14T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let loadbalancing1 = LoadBalancingConfig::with_str("instance-id-1");
        let loadbalancing2 = LoadBalancingConfig::with_str("instance-id-2");

        let Ok(MutationOutcome::RequirePatch(patched_pod)) = mutate_to_evict(
            Some(&pod),
            timestamp1,
            &loadbalancing1,
            &DeleteOptions::default(),
        ) else {
            panic!("should be patched");
        };

        let result = mutate_to_evict_later(Some(&patched_pod), timestamp2, &loadbalancing2);
        assert_matches!(
            result,
            Ok(MutationOutcome::RequirePatch(pod)) if pod == from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "evicting",
                    },
                    "annotations": {
                        "pod-graceful-drain/controller": "instance-id-2",
                        "pod-graceful-drain/delete-options": "{}",
                        "pod-graceful-drain/evict-after": "2025-03-14T00:00:00Z",
                    },
                },
            })
        );
    }

    #[test]
    fn should_return_gone_if_pod_is_none() {
        let timestamp = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");

        let result = mutate_to_evict_later(None, timestamp, &loadbalancing);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Gone))
        );
    }

    #[test]
    fn should_not_regress_timestamp() {
        let pod = from_json!({});

        let timestamp1 = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let timestamp2 = DateTime::parse_from_rfc3339("2025-03-14T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let loadbalancing1 = LoadBalancingConfig::with_str("instance-id-1");
        let loadbalancing2 = LoadBalancingConfig::with_str("instance-id-2");

        let Ok(MutationOutcome::RequirePatch(patched_pod)) = mutate_to_evict(
            Some(&pod),
            timestamp2,
            &loadbalancing2,
            &DeleteOptions::default(),
        ) else {
            panic!("should be patched");
        };

        let result = mutate_to_evict_later(Some(&patched_pod), timestamp1, &loadbalancing1);
        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(
                PatchToEvictOutcome::WaitingForPodDisruptionBudget
            ))
        );
    }

    #[test]
    fn should_not_regress_from_draining() {
        let pod: Pod = from_json!({});

        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let result = drain::mutate_to_drain(Some(&pod), timestamp, &loadbalancing, true);
        let Ok(MutationOutcome::RequirePatch(pod)) = result else {
            panic!("should patch pod");
        };

        let result = mutate_to_evict_later(Some(&pod), timestamp, &loadbalancing);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Draining))
        );
    }
}
