pub mod patch_eviction_admission;
pub mod patch_to_drain;
mod patch_to_evict;

use backoff::ExponentialBackoff;
use backoff::backoff::Backoff;
use eyre::{Context, Result, eyre};
use json_patch::{Patch, PatchOperation, TestOperation};
use jsonptr::PointerBuf;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::serde::Serialize;
use k8s_openapi::serde::de::DeserializeOwned;
use kube::api::PatchParams;
use kube::core::NamespaceResourceScope;
use kube::{Resource, ResourceExt};
use serde_json::Value;
use std::fmt::Debug;
use tracing::trace;

use crate::api_resolver::ApiResolver;
use crate::error_codes::{
    is_404_not_found_error, is_409_conflict_error, is_410_expired_error,
    is_generic_server_response_422_invalid_for_json_patch_error, is_transient_error,
};

async fn apply_patch<K, R>(
    api_resolver: &ApiResolver,
    res: &K,
    patch: impl Fn(&K) -> Result<Patch>,
    check: impl Fn(&K) -> bool,
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
            Err(err) if is_404_not_found_error(&err) || is_410_expired_error(&err) => {
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
                Err(err) if is_404_not_found_error(&err) || is_410_expired_error(&err) => {
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

fn build_diff<K: Resource + Clone + Serialize>(
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

fn prepend_uid_and_resource_version_test(mut patch: Patch, pod: &Pod) -> Patch {
    if let Some(uid) = pod.uid() {
        patch.0.insert(
            0,
            PatchOperation::Test(TestOperation {
                path: PointerBuf::from_tokens(["metadata", "uid"]),
                value: Value::String(uid),
            }),
        );
    }

    if let Some(version) = pod.resource_version() {
        patch.0.insert(
            1,
            PatchOperation::Test(TestOperation {
                path: PointerBuf::from_tokens(["metadata", "resourceVersion"]),
                value: Value::String(version),
            }),
        );
    }

    patch
}
