use chrono::{DateTime, Utc};
use eyre::Result;
use k8s_openapi::api::core::v1::Pod;
use thiserror::Error;

use crate::LoadBalancingConfig;
use crate::api_resolver::ApiResolver;
use crate::error_types::Bug;
use crate::labels_and_annotations::{
    DrainingLabelValue, get_pod_drain_timestamp, get_pod_draining_label_value,
    set_pod_delete_options, set_pod_drain_controller, set_pod_evict_after,
    try_backup_pod_original_labels, try_set_pod_drain_timestamp, try_set_pod_draining_label_value,
};
use crate::patch::resource_patch_util::{MutationOutcome, ResourcePatchError, patch};

#[derive(Debug)]
pub enum PatchToDrainOutcome {
    /// pod is gone
    Gone,
    /// pod is draining
    Draining { drain_timestamp: DateTime<Utc> },
}

pub enum PatchToDrainCaller {
    Webhook,
    Controller,
}

#[derive(Debug, Error)]
pub enum PatchToDrainError {
    #[error("failed to patch")]
    PatchError(#[from] ResourcePatchError),
    #[error(transparent)]
    Bug(#[from] Bug),
}

pub async fn patch_to_drain(
    pod: &Pod,
    api_resolver: &ApiResolver,
    loadbalancing: &LoadBalancingConfig,
    caller: PatchToDrainCaller,
) -> Result<PatchToDrainOutcome, PatchToDrainError> {
    let preserve_delete_options = match caller {
        PatchToDrainCaller::Webhook => true,
        PatchToDrainCaller::Controller => false,
    };

    patch(api_resolver, pod, |pod| {
        mutate_to_drain(pod, Utc::now(), loadbalancing, preserve_delete_options)
    })
    .await
}

pub(super) fn mutate_to_drain(
    pod: Option<&Pod>,
    now: DateTime<Utc>,
    loadbalancing: &LoadBalancingConfig,
    preserve_delete_options: bool,
) -> Result<MutationOutcome<PatchToDrainOutcome, Pod>, Bug> {
    let Some(pod) = pod else {
        return Ok(MutationOutcome::DesiredState(PatchToDrainOutcome::Gone));
    };

    let draining_state = get_pod_draining_label_value(pod);
    if let Ok(Some(DrainingLabelValue::Draining)) = draining_state {
        let drain_timestamp = get_pod_drain_timestamp(pod);
        if let Ok(Some(drain_timestamp)) = drain_timestamp {
            return Ok(MutationOutcome::DesiredState(
                PatchToDrainOutcome::Draining { drain_timestamp },
            ));
        }
    }

    let pod = (|| -> Result<_, Bug> {
        let mut pod = pod.clone();

        try_backup_pod_original_labels(&mut pod)?;
        try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Draining);
        try_set_pod_drain_timestamp(&mut pod, now);
        set_pod_evict_after(&mut pod, None);
        set_pod_drain_controller(&mut pod, loadbalancing);
        if !preserve_delete_options {
            set_pod_delete_options(&mut pod, None)?;
        }
        remove_owner_reference(&mut pod);

        Ok(pod)
    })()?;

    Ok(MutationOutcome::RequirePatch(pod))
}

/// To stop the pod controller's GC kicking in, we remove the OwnerReferences.
fn remove_owner_reference(pod: &mut Pod) {
    if let Some(owner_refs) = pod.metadata.owner_references.as_deref_mut() {
        for owner_ref in owner_refs {
            if owner_ref.api_version == "v1" && owner_ref.kind == "ReplicaSet" {
                owner_ref.controller = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;

    use super::*;
    use crate::from_json;
    use crate::patch::evict;

    #[test]
    fn test_mutate_should_return_gone_if_pod_is_none() {
        let drain_timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");

        let result = mutate_to_drain(None, drain_timestamp, &loadbalancing, false);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToDrainOutcome::Gone))
        );
    }

    #[test]
    fn smoke_test() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
                "labels": {
                    "app": "test",
                },
                "annotations": {
                    "pod-graceful-drain/delete-options": "{\"dryRun\":[\"All\"]}",
                },
                "ownerReferences": [{
                    "apiVersion": "v1",
                    "kind": "ReplicaSet",
                    "name": "owner",
                    "uid": "12345",
                    "controller": true,
                }],
            },
        });

        let drain_timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let result = mutate_to_drain(Some(&pod), drain_timestamp, &loadbalancing, false);

        assert_matches!(
            result,
            Ok(MutationOutcome::RequirePatch(pod)) if pod == from_json!({
                "metadata": {
                    "uid": "uid1234",
                    "resourceVersion": "version1234",
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                    "annotations": {
                        "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "instance-id-1",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    },
                    "ownerReferences": [{
                        "apiVersion": "v1",
                        "kind": "ReplicaSet",
                        "name": "owner",
                        "uid": "12345",
                    }],
                },
            })
        );
    }

    #[test]
    fn should_return_gone_if_pod_is_none() {
        let drain_timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let result = mutate_to_drain(None, drain_timestamp, &loadbalancing, false);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToDrainOutcome::Gone))
        );
    }

    #[test]
    fn should_be_idempotent() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "app": "test",
                },
            },
        });

        let drain_timestamp1 = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");

        let Ok(MutationOutcome::RequirePatch(patched_pod)) =
            mutate_to_drain(Some(&pod), drain_timestamp1, &loadbalancing, true)
        else {
            panic!("Expected a patch");
        };

        let drain_timestamp2 = DateTime::parse_from_rfc3339("2024-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-2");

        let outcome2 = mutate_to_drain(Some(&patched_pod), drain_timestamp2, &loadbalancing, true);

        assert_matches!(
            outcome2,
            Ok(MutationOutcome::DesiredState(PatchToDrainOutcome::Draining { drain_timestamp })) if drain_timestamp == drain_timestamp1
        );
    }

    #[test]
    fn should_progress() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "app": "test",
                },
            },
        });

        let timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing1 = LoadBalancingConfig::with_str("instance-id-1");
        let Ok(MutationOutcome::RequirePatch(patched_pod)) = evict::mutate_to_evict(
            Some(&pod),
            timestamp,
            &loadbalancing1,
            &DeleteOptions::default(),
        ) else {
            panic!("Expected a patch");
        };

        let loadbalancing2 = LoadBalancingConfig::with_str("instance-id-2");

        let result = mutate_to_drain(Some(&patched_pod), timestamp, &loadbalancing2, true);

        assert_matches!(result, Ok(MutationOutcome::RequirePatch(pod)) if pod ==from_json!({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {
                    "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                    "pod-graceful-drain/controller": "instance-id-2",
                    "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    "pod-graceful-drain/delete-options": "{}",
                },
            },
        }));
    }
}
