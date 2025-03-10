use eyre::Result;
use json_patch::Patch;
use k8s_openapi::api::core::v1::Pod;

use crate::LoadBalancingConfig;
use crate::api_resolver::ApiResolver;
use crate::labels_and_annotations::{
    DrainingLabelValue, set_pod_drain_controller, try_set_pod_draining_label_value,
};
use crate::patch::{apply_patch, build_diff, prepend_uid_and_resource_version_test};
use crate::pod_draining_state::{PodDrainingState, get_pod_draining_state};

pub async fn patch_to_evict(
    api_resolver: &ApiResolver,
    pod: &Pod,
    loadbalancing: &LoadBalancingConfig,
) -> Result<Option<Pod>> {
    let res = apply_patch(
        api_resolver,
        pod,
        |pod| make_patch_to_evict(pod, loadbalancing),
        |pod| !matches!(get_pod_draining_state(pod), PodDrainingState::None),
    )
    .await?;
    Ok(res)
}

/// This function should be idempotent.
/// This function is responsible for multiple pod state transition path.
/// 1. from the evict webhook
/// 3. from the evict webhook for a pod that is waiting for the eviction controller
fn make_patch_to_evict(pod: &Pod, loadbalancing: &LoadBalancingConfig) -> Result<Patch> {
    let patch = build_diff(pod, |pod| {
        try_set_pod_draining_label_value(pod, DrainingLabelValue::WaitingForPodDisruptionBudget);
        set_pod_drain_controller(pod, loadbalancing);
        Ok(())
    })?;

    let version_checked = prepend_uid_and_resource_version_test(patch, pod);
    Ok(version_checked)
}

#[cfg(test)]
mod tests {
    use super::*;

    use json_patch::{PatchOperation, TestOperation};
    use serde::Serialize;
    use serde_json::{Value, json};

    macro_rules! from_json {
        ($($json:tt)+) => {
            ::serde_json::from_value(::serde_json::json!($($json)+)).expect("Invalid json")
        };
    }

    fn apply<K>(res: &K, patch: &Patch) -> Result<Value>
    where
        K: Serialize,
    {
        let mut modified = serde_json::to_value(res)?;
        json_patch::patch(&mut modified, &patch.0)?;
        Ok(modified)
    }

    #[test]
    fn test_patch_evict() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
            }
        });

        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch = make_patch_to_evict(&pod, &loadbalancing).unwrap();

        let applied = apply(&pod, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "uid": "uid1234",
                    "resourceVersion": "version1234",
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                    "annotations": {
                        "pod-graceful-drain/drain-until": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    },
                    "ownerReferences": [{
                        "apiVersion": "v1",
                        "kind": "ReplicaSet",
                        "name": "owner",
                        "uid": "12345",
                    }]
                },
            })
        );
    }

    #[test]
    fn test_patch_drain_should_contain_test_resource_version() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
            }
        });

        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch = make_patch_to_evict(&pod, &loadbalancing).unwrap();

        assert_eq!(
            &patch[..2],
            &[
                PatchOperation::Test(TestOperation {
                    path: "/metadata/uid".try_into().unwrap(),
                    value: json!("uid1234"),
                }),
                PatchOperation::Test(TestOperation {
                    path: "/metadata/resourceVersion".try_into().unwrap(),
                    value: json!("version1234"),
                })
            ],
        );
    }
}
