use std::borrow::Cow;
use std::fmt::Debug;
use std::time::Duration;

use backon::{BackoffBuilder, ExponentialBackoff, ExponentialBuilder};
use eyre::{Context, Result};
use json_patch::{Patch, PatchOperation, TestOperation};
use jsonptr::PointerBuf;
use k8s_openapi::serde::Serialize;
use k8s_openapi::serde::de::DeserializeOwned;
use kube::api::PatchParams;
use kube::core::NamespaceResourceScope;
use kube::{Api, Resource, ResourceExt};
use serde_json::Value;
use thiserror::Error;
use tracing::trace;

use crate::api_resolver::ApiResolver;
use crate::error_codes::{
    is_404_not_found_error, is_409_conflict_error,
    is_422_invalid_for_json_patch_test_error, is_transient_error,
};
use crate::error_types::Bug;

#[derive(Debug)]
pub enum MutationOutcome<T, R> {
    DesiredState(T),
    RequirePatch(R),
}

pub async fn patch<K, T, E1, E2>(
    api_resolver: &ApiResolver,
    res: &K,
    get_desired_state_or_mutated_res: impl Fn(Option<&K>) -> Result<MutationOutcome<T, K>, E1>,
) -> Result<T, E2>
where
    K: Resource<Scope = NamespaceResourceScope> + Clone + Serialize + DeserializeOwned + Debug,
    K::DynamicType: Default,
    K: ToOwned<Owned = K>,
    E2: From<E1> + From<ResourcePatchError>,
{
    let mut patcher = ResourcePatchUtil::new(api_resolver, res);
    loop {
        let outcome = get_desired_state_or_mutated_res(patcher.get())?;

        match outcome {
            MutationOutcome::DesiredState(desired) => {
                return Ok(desired);
            }
            MutationOutcome::RequirePatch(new_pod) => {
                patcher.try_patch(&new_pod).await?;
            }
        }
    }
}

pub struct ResourcePatchUtil<'a, K>
where
    K: ToOwned,
{
    api: Api<K>,
    name: String,
    last_known: Option<Cow<'a, K>>,
    backoff: ExponentialBackoff,
}

impl<'a, K> ResourcePatchUtil<'a, K>
where
    K: Resource<Scope = NamespaceResourceScope>,
    K::DynamicType: Default,
    K: ToOwned<Owned = K>,
{
    pub fn new(api_resolver: &ApiResolver, res: &'a K) -> Self {
        let api = api_resolver.api_for(res);
        let name = res.meta().name.clone().expect("pod should have name");

        let backoff = ExponentialBuilder::new()
            .with_jitter()
            .with_min_delay(Duration::from_millis(100))
            .with_max_times(5)
            .build();

        Self {
            api,
            name,
            last_known: Some(Cow::Borrowed(res)),
            backoff,
        }
    }

    pub fn get(&self) -> Option<&K> {
        self.last_known.as_ref().map(|x| x.as_ref())
    }
}

impl<K> ResourcePatchUtil<'_, K>
where
    K: Resource<Scope = NamespaceResourceScope> + Clone + Serialize + DeserializeOwned + Debug,
    K::DynamicType: Default,
    K: ToOwned<Owned = K>,
{
    pub async fn try_patch(&mut self, new_state: &K) -> Result<(), ResourcePatchError> {
        let Some(old_state) = self.last_known.as_ref().map(|x| x.as_ref()) else {
            return Err(Bug {
                message: String::from("tried to patch patch on non-existing resource"),
                source: None,
            }
            .into());
        };

        let patch = build_patch(old_state, new_state)
            .context("building patch")
            .map_err(|err| Bug {
                message: String::from("failed to build a patch"),
                source: Some(err),
            })?;

        if patch.0.is_empty() {
            return Err(Bug {
                message: String::from("tried to patch with empty patch"),
                source: None,
            }
            .into());
        }

        let patch = prepend_uid_and_resource_version_test(patch, old_state);

        trace!(?patch, "patching");
        let result = self
            .api
            .patch(
                &self.name,
                &PatchParams::default(),
                &kube::api::Patch::<K>::Json(patch),
            )
            .await;

        let err = match result {
            Ok(new_res) => {
                self.last_known = Some(Cow::Owned(new_res.clone()));
                return Ok(());
            }
            Err(err) if is_404_not_found_error(&err) => {
                self.last_known = None;
                return Ok(());
            }
            Err(err) => err,
        };

        if !(is_transient_error(&err)
            // kubernetes api server returns 422 when JsonPatch fails to test, not 409.
            // SEE: https://github.com/kubernetes/kubernetes/blob/2a1d4172e22abb6759b3d2ad21bb09a04eef596d/staging/src/k8s.io/apiserver/pkg/endpoints/handlers/patch.go#L394
            || is_422_invalid_for_json_patch_test_error(&err)
            // Conflict is to reduce future confusion.
            || is_409_conflict_error(&err))
        {
            return Err(err.into());
        }

        // transient errors, conflict errors
        'refresh: loop {
            let refreshed = self.api.get_opt(&self.name).await;
            match refreshed {
                Err(err) if is_404_not_found_error(&err) => {
                    self.last_known = None;
                    return Ok(());
                }
                Err(err) if is_transient_error(&err) => {
                    if let Some(backoff) = self.backoff.next() {
                        tokio::time::sleep(backoff).await;
                        continue 'refresh;
                    } else {
                        return Err(ResourcePatchError::KubeError(err));
                    }
                }
                Err(err) => {
                    return Err(err.into());
                }
                Ok(None) => {
                    self.last_known = None;
                    return Ok(());
                }
                Ok(Some(refreshed)) => {
                    if refreshed.meta().uid != old_state.meta().uid {
                        // uid changed, the resource that we know is gone
                        self.last_known = None;
                        return Ok(());
                    }

                    self.last_known = Some(Cow::Owned(refreshed.clone()));
                    return Ok(());
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ResourcePatchError {
    #[error("kube error")]
    KubeError(#[from] kube::Error),
    #[error(transparent)]
    Bug(#[from] Bug),
}

pub fn build_patch<K: Serialize>(before: &K, after: &K) -> Result<Patch> {
    let before = serde_json::to_value(before).context("before state")?;
    let after = serde_json::to_value(after).context("after state")?;

    let patch = json_patch::diff(&before, &after);
    Ok(patch)
}

fn prepend_uid_and_resource_version_test<K>(mut patch: Patch, resource: &K) -> Patch
where
    K: Resource,
{
    if let Some(uid) = resource.uid() {
        patch.0.insert(
            0,
            PatchOperation::Test(TestOperation {
                path: PointerBuf::from_tokens(["metadata", "uid"]),
                value: Value::String(uid),
            }),
        );
    }

    if let Some(version) = resource.resource_version() {
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
