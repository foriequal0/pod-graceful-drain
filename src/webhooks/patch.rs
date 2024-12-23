use std::fmt::Debug;

use backoff::backoff::Backoff;
use backoff::ExponentialBackoff;
use chrono::{DateTime, SecondsFormat, Utc};
use eyre::{eyre, Context, Result};
use json_patch::{Patch, PatchOperation, TestOperation};
use jsonptr::PointerBuf;
use k8s_openapi::api::{core::v1::Pod, policy::v1::Eviction};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use k8s_openapi::serde::de::DeserializeOwned;
use k8s_openapi::serde::Serialize;
use kube::api::PatchParams;
use kube::core::NamespaceResourceScope;
use kube::{Resource, ResourceExt};
use serde_json::Value;
use tracing::trace;

use crate::api_resolver::ApiResolver;
use crate::consts::{
    DELETE_OPTIONS_ANNOTATION_KEY, DRAINING_LABEL_KEY, DRAIN_CONTROLLER_ANNOTATION_KEY,
    DRAIN_UNTIL_ANNOTATION_KEY, ORIGINAL_LABELS_ANNOTATION_KEY,
};
use crate::pod_draining_info::{get_pod_draining_info, PodDrainingInfo};
use crate::status::{
    is_404_not_found_error, is_409_conflict_error, is_410_gone_error,
    is_generic_server_response_422_invalid_for_json_patch_error, is_transient_error,
};
use crate::LoadBalancingConfig;

async fn apply_patch<K>(
    api_resolver: &ApiResolver,
    res: &K,
    patch: impl Fn(&K) -> Result<Patch> + Clone,
    check: impl Fn(&K) -> bool + Clone,
) -> Result<Option<K>>
where
    K: Resource<Scope = NamespaceResourceScope> + Clone + Serialize + DeserializeOwned + Debug,
    K::DynamicType: Default,
{
    let api = api_resolver.api_for(res);
    let name = res.name_any();

    let mut res = res.clone();
    let mut backoff = ExponentialBackoff::default();
    'patch: while !check(&res) {
        let patch = patch(&res).context("patch")?;
        trace!(?patch, "patching");
        let result = api
            .patch(
                &name,
                &PatchParams::default(),
                &kube::api::Patch::<K>::Json(patch),
            )
            .await;

        let err = match result {
            Ok(new_res) => {
                return Ok(Some(new_res));
            }
            Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
                return Ok(None); // this is what we desire.
            }
            Err(err) => err,
        };

        if !(is_transient_error(&err)
            // kubernetes api server returns 422 when JsonPatch fails to test, not 409.
            // SEE: https://github.com/kubernetes/kubernetes/blob/2a1d4172e22abb6759b3d2ad21bb09a04eef596d/staging/src/k8s.io/apiserver/pkg/endpoints/handlers/patch.go#L394
            || is_generic_server_response_422_invalid_for_json_patch_error(&err)
            // Conflict is to reduce future confusion.
            || is_409_conflict_error(&err))
        {
            return Err(err.into());
        }

        // transient errors, conflict errors
        'refresh: loop {
            if let Some(backoff) = backoff.next_backoff() {
                tokio::time::sleep(backoff).await;
            } else {
                return Err(eyre!("no more backoff"));
            }

            let refreshed = api.get(&name).await;
            match refreshed {
                Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
                    // Resource is gone
                    return Ok(None);
                }
                Err(err) if is_transient_error(&err) => {
                    continue 'refresh;
                }
                Err(err) => {
                    return Err(err.into());
                }
                Ok(refreshed) => {
                    if res.meta().resource_version != refreshed.meta().resource_version {
                        res = refreshed;
                        continue 'patch;
                    }

                    return Err(eyre!("resource isn't changed after the refresh"));
                }
            }
        }
    }

    Ok(Some(res))
}

fn make_patch<K: Resource + Clone + Serialize>(
    res: &K,
    modify: impl Fn(&mut K) -> Result<()>,
) -> Result<Patch> {
    let before = serde_json::to_value(res).context("serialize")?;
    let after = {
        let mut modified = res.clone();
        modify(&mut modified).context("modify")?;
        serde_json::to_value(modified).context("serialize modified")?
    };

    let patch = json_patch::diff(&before, &after);
    Ok(patch)
}

fn prepend_uid_and_resource_version_test(mut patch: Patch, pod: &Pod) -> Result<Patch> {
    let uid = pod.uid().ok_or(eyre!("no uid"))?;
    let version = pod.resource_version().ok_or(eyre!("no resource version"))?;
    patch.0.insert(
        0,
        PatchOperation::Test(TestOperation {
            path: PointerBuf::from_tokens(["metadata", "uid"]),
            value: Value::String(uid),
        }),
    );
    patch.0.insert(
        1,
        PatchOperation::Test(TestOperation {
            path: PointerBuf::from_tokens(["metadata", "resourceVersion"]),
            value: Value::String(version),
        }),
    );

    Ok(patch)
}

pub async fn patch_pod_isolate(
    api_resolver: &ApiResolver,
    pod: &Pod,
    drain_until: DateTime<Utc>,
    eviction_delete_options: Option<&DeleteOptions>,
    loadbalancing: &LoadBalancingConfig,
) -> Result<Option<Pod>> {
    let res = apply_patch(
        api_resolver,
        pod,
        |pod| make_patch_pod_isolate(pod, drain_until, eviction_delete_options, loadbalancing),
        |pod| !matches!(get_pod_draining_info(pod), PodDrainingInfo::None),
    )
    .await?;
    Ok(res)
}

fn make_patch_pod_isolate(
    pod: &Pod,
    drain_until: DateTime<Utc>,
    eviction_delete_options: Option<&DeleteOptions>,
    loadbalancing: &LoadBalancingConfig,
) -> Result<Patch> {
    let patch = make_patch(pod, |pod| {
        backup_original_labels(pod).context("backup")?;
        set_draining_label(pod);
        set_drain_until_annotation(pod, drain_until);
        if let Some(eviction_delete_options) = eviction_delete_options {
            set_eviction_delete_options(pod, eviction_delete_options)?;
        }
        set_controller_annotation(pod, loadbalancing);
        remove_owner_reference(pod);
        Ok(())
    })?;
    return prepend_uid_and_resource_version_test(patch, pod);

    fn backup_original_labels(pod: &mut Pod) -> Result<()> {
        let labels = pod.labels_mut();
        let original_labels = serde_json::to_string(labels).context("serialize old labels")?;
        labels.clear();
        pod.annotations_mut().insert(
            String::from(ORIGINAL_LABELS_ANNOTATION_KEY),
            original_labels,
        );
        Ok(())
    }

    fn set_draining_label(pod: &mut Pod) {
        pod.labels_mut()
            .insert(String::from(DRAINING_LABEL_KEY), String::from("true"));
    }

    fn set_drain_until_annotation(pod: &mut Pod, drain_until: DateTime<Utc>) {
        let string = drain_until.to_rfc3339_opts(SecondsFormat::Secs, true);
        pod.annotations_mut()
            .insert(String::from(DRAIN_UNTIL_ANNOTATION_KEY), string);
    }

    fn set_eviction_delete_options(pod: &mut Pod, delete_options: &DeleteOptions) -> Result<()> {
        let annotation = serde_json::to_string(&DeleteOptions {
            // this is not dry-run
            dry_run: None,
            // preconditions.uid is this pod, so it is duplicate.
            // preconditions.resourceVersion will be voided by this patch.
            preconditions: None,
            kind: None,
            api_version: None,
            ..delete_options.clone()
        })
        .context("serialize old labels")?;

        pod.annotations_mut()
            .insert(String::from(DELETE_OPTIONS_ANNOTATION_KEY), annotation);
        Ok(())
    }

    fn set_controller_annotation(pod: &mut Pod, loadbalancing: &LoadBalancingConfig) {
        pod.annotations_mut().insert(
            String::from(DRAIN_CONTROLLER_ANNOTATION_KEY),
            loadbalancing.get_id(),
        );
    }

    /// To stop the pod controller's GC kicking in, we remove the OwnerReferences.
    fn remove_owner_reference(pod: &mut Pod) {
        for owner_ref in pod.owner_references_mut() {
            if owner_ref.api_version == "v1" && owner_ref.kind == "ReplicaSet" {
                owner_ref.controller = None;
            }
        }
    }
}

pub fn make_patch_eviction_to_dry_run(eviction: &Eviction) -> Result<Patch> {
    return make_patch(eviction, set_dry_run);

    fn set_dry_run(eviction: &mut Eviction) -> Result<()> {
        let delete_options = eviction.delete_options.clone().unwrap_or_default();
        eviction.delete_options = Some(DeleteOptions {
            dry_run: Some(vec![String::from("All")]),
            ..delete_options
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::DateTime;
    use serde_json::{json, Value};
    use uuid::Uuid;

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
    fn pod_patch_isolate() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
                "labels": {
                    "app": "test"
                },
                "ownerReferences": [{
                    "apiVersion": "v1",
                    "kind": "ReplicaSet",
                    "name": "owner",
                    "uid": "12345",
                    "controller": true,
                }]
            }
        });

        let drain_until = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::new(Uuid::nil());
        let patch = make_patch_pod_isolate(&pod, drain_until, None, &loadbalancing).unwrap();

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
    fn pod_patch_isolate_should_contain_test_resource_version() {
        let pod: Pod = from_json! ({
            "metadata": {
                "uid": "uid1234",
                "resourceVersion": "version1234",
            }
        });

        let drain_until = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::new(Uuid::nil());
        let patch = make_patch_pod_isolate(&pod, drain_until, None, &loadbalancing).unwrap();

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

    #[test]
    fn eviction_patch_none_delete_options() {
        let eviction: Eviction = from_json!({});

        let patch = make_patch_eviction_to_dry_run(&eviction).unwrap();

        let applied = apply(&eviction, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "policy/v1",
                "kind": "Eviction",
                "metadata": {},
                "deleteOptions": {
                    "dryRun": ["All"],
                },
            })
        );
    }

    #[test]
    fn eviction_patch_none_dry_run() {
        let eviction: Eviction = from_json!({
            "deleteOptions": {},
        });

        let patch = make_patch_eviction_to_dry_run(&eviction).unwrap();

        let applied = apply(&eviction, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "policy/v1",
                "kind": "Eviction",
                "metadata": {},
                "deleteOptions": {
                    "dryRun": ["All"],
                },
            })
        );
    }

    #[test]
    fn eviction_patch_empty_dry_run() {
        let eviction: Eviction = from_json!({
            "deleteOptions": {
                "dryRun": [],
            },
        });

        let patch = make_patch_eviction_to_dry_run(&eviction).unwrap();

        let applied = apply(&eviction, &patch).unwrap();
        assert_eq!(
            applied,
            json!({
                "apiVersion": "policy/v1",
                "kind": "Eviction",
                "metadata": {},
                "deleteOptions": {
                    "dryRun": ["All"],
                },
            })
        );
    }
}
