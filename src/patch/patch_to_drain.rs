use std::collections::btree_map::Entry;

use chrono::{DateTime, SecondsFormat, Utc};
use eyre::{Context, Result};
use genawaiter::{rc::r#gen, yield_};
use json_patch::Patch;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::ResourceExt;

use crate::LoadBalancingConfig;
use crate::api_resolver::ApiResolver;
use crate::labels_and_annotations::{
    DrainingLabelValue, get_pod_draining_label_value, set_pod_delete_options,
    set_pod_drain_controller, try_backup_pod_original_labels, try_set_pod_drain_timestamp,
    try_set_pod_draining_label_value,
};
use crate::patch::{apply_patch, build_diff, prepend_uid_and_resource_version_test};

pub async fn patch_to_drain(
    api_resolver: &ApiResolver,
    pod: &Pod,
    drain_timestamp: DateTime<Utc>,
    loadbalancing: &LoadBalancingConfig,
    delete_options: Option<&DeleteOptions>,
) -> Result<Option<Pod>> {
    let res = apply_patch(
        api_resolver,
        pod,
        |pod| make_patch_to_drain(pod, drain_timestamp, loadbalancing, delete_options),
        |pod| {
            let draining_state = get_pod_draining_label_value(pod);
            // draining_state == Draining && drain_timestamp set
        },
    )
    .await?;
    Ok(res)
}

/// This function should be idempotent.
/// It is responsible for multiple pod state transition path.
/// 1. from the delete webhook
/// 2. from the eviction controller
/// 3. from the delete webhook for a pod that is waiting for the eviction controller
fn make_patch_to_drain(
    pod: &Pod,
    drain_timestamp: DateTime<Utc>,
    loadbalancing: &LoadBalancingConfig,
    delete_options: Option<&DeleteOptions>,
) -> Result<Patch> {
    let patch = build_diff(pod, |pod| {
        try_backup_pod_original_labels(pod)?;
        try_set_pod_draining_label_value(pod, DrainingLabelValue::Draining);
        try_set_pod_drain_timestamp(pod, drain_timestamp);
        set_pod_drain_controller(pod, loadbalancing);
        set_pod_delete_options(pod, delete_options)?;
        remove_owner_reference(pod);
        Ok(())
    })?;

    let version_checked = prepend_uid_and_resource_version_test(patch, pod);
    return Ok(version_checked);

    /// To stop the pod controller's GC kicking in, we remove the OwnerReferences.
    fn remove_owner_reference(pod: &mut Pod) {
        for owner_ref in pod.owner_references_mut() {
            if owner_ref.api_version == "v1" && owner_ref.kind == "ReplicaSet" {
                owner_ref.controller = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::DateTime;
    use json_patch::{PatchOperation, TestOperation};
    use serde::{Deserialize, Serialize};
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

    fn apply_into<K>(res: &K, patch: &Patch) -> Result<K>
    where
        for<'de> K: Serialize + Deserialize<'de>,
    {
        let mut modified = serde_json::to_value(res)?;
        json_patch::patch(&mut modified, &patch.0)?;
        let value = serde_json::from_value(modified).expect("Invalid json");
        Ok(value)
    }

    #[test]
    fn test_patch_drain() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
                "labels": {
                    "app": "test",
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
        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch = make_patch_to_drain(&pod, drain_timestamp, &loadbalancing, &Default::default())
            .unwrap();

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
                        "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    },
                    "ownerReferences": [{
                        "apiVersion": "v1",
                        "kind": "ReplicaSet",
                        "name": "owner",
                        "uid": "12345",
                    }],
                },
            }),
        );
    }

    #[test]
    fn patch_drain_should_be_idempotent() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
                "labels": {
                    "app": "test",
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
        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");

        let patch1 =
            make_patch_to_drain(&pod, drain_timestamp, &loadbalancing, &Default::default())
                .unwrap();
        let patched_pod = apply_into(&pod, &patch1).unwrap();

        let drain_timestamp = DateTime::parse_from_rfc3339("2024-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("11111111-1111-1111-1111-111111111111");

        let patch2 = make_patch_to_drain(
            &patched_pod,
            drain_timestamp,
            &loadbalancing,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(patch2.0.len(), 0);
    }

    #[test]
    fn patch_drain_should_heal_draining_label() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "asdf",
                },
                "annotations": {
                    "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                },
            },
        });

        let drain_timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch = make_patch_to_drain(&pod, drain_timestamp, &loadbalancing, &Default::default())
            .unwrap();

        let applied = apply(&pod, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                    "annotations": {
                        "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                        "pod-graceful-drain/original-labels_1": "{\"pod-graceful-drain/draining\":\"asdf\"}",
                    },
                },
            })
        );
    }

    #[test]
    fn patch_drain_should_heal_drain_timestamp() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {
                    "pod-graceful-drain/drain-until": "malformed",
                },
            },
        });

        let drain_timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch = make_patch_to_drain(&pod, drain_timestamp, &loadbalancing, &Default::default())
            .unwrap();

        let applied = apply(&pod, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                    "annotations": {
                        "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    },
                },
            })
        );
    }

    #[test]
    fn patch_drain_should_update_controller() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {
                    "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                    "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                    "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                },
            },
        });

        let drain_until = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("11111111-1111-1111-1111-111111111111");
        let patch =
            make_patch_to_drain(&pod, drain_until, &loadbalancing, &Default::default()).unwrap();

        let applied = apply(&pod, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                    "annotations": {
                        "pod-graceful-drain/drain-timestamp": "2023-02-08T15:30:00Z",
                        "pod-graceful-drain/controller": "00000000-0000-0000-0000-000000000000",
                        "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                    },
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

        let drain_until = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("00000000-0000-0000-0000-000000000000");
        let patch =
            make_patch_to_drain(&pod, drain_until, &loadbalancing, &DeleteOptions::default())
                .unwrap();

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
