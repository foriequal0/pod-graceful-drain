use std::collections::btree_map::Entry;

use eyre::Result;
use jiff::Timestamp;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use kube::ResourceExt;
use thiserror::Error;

use crate::ApiResolver;
use crate::error_types::NotMyFault;
use crate::patch::resource_patch_util::{MutationOutcome, ResourcePatchError, patch};

#[derive(Debug, Error)]
pub enum ForceTriggerSyncError {
    #[error("failed to patch")]
    PatchError(#[from] ResourcePatchError),
    #[error(transparent)]
    NotMyFault(#[from] NotMyFault),
}

/// ISSUE: https://github.com/foriequal0/pod-graceful-drain/issues/7#issuecomment-4202371201
/// Kubernetes' PDB controller doesn't update PDB if pod's labels are no more selected by the selector
/// REF: https://github.com/kubernetes/kubernetes/blob/23be9587a0f8677eb8091464098881df939c44a9/pkg/controller/disruption/disruption.go#L336-L346
/// So attach an annotation to the PDB to forcefully trigger sync.
/// (the PDB controller updates the PDB when it is modified)
pub async fn force_trigger_sync(
    pdb: &PodDisruptionBudget,
    api_resolver: &ApiResolver,
) -> Result<(), ForceTriggerSyncError> {
    let now = Timestamp::now();
    patch::<_, _, _, ForceTriggerSyncError>(api_resolver, pdb, |pdb| {
        mutate_to_trigger_force_sync(pdb, now)
    })
    .await?;
    Ok(())
}

fn mutate_to_trigger_force_sync(
    pdb: Option<&PodDisruptionBudget>,
    now: Timestamp,
) -> Result<MutationOutcome<(), PodDisruptionBudget>, NotMyFault> {
    let Some(pdb) = pdb else {
        // pdb is gone. no need to patch
        return Ok(MutationOutcome::DesiredState(()));
    };

    let mut pdb = pdb.clone();
    if !try_set_pdb_force_sync_annotation(&mut pdb, now) {
        return Ok(MutationOutcome::DesiredState(()));
    }

    Ok(MutationOutcome::RequirePatch(pdb))
}

const DRAIN_FORCE_SYNC_NONCE: &str = "pod-graceful-drain/force-sync-nonce";

fn try_set_pdb_force_sync_annotation(pdb: &mut PodDisruptionBudget, value: Timestamp) -> bool {
    let str = value.to_string();
    match pdb
        .annotations_mut()
        .entry(String::from(DRAIN_FORCE_SYNC_NONCE))
    {
        Entry::Vacant(entry) => {
            entry.insert(str);
            true
        }
        Entry::Occupied(mut entry) => {
            let existing = entry.get();
            if existing.as_str() == str.as_str() {
                // do not need to change
                return false;
            }

            entry.insert(str);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::from_json;

    #[test]
    fn smoke_test() {
        let pod: PodDisruptionBudget = from_json!({});

        let timestamp = Timestamp::from_str("2025-03-13T00:00:00.123456789Z").unwrap();

        let result = mutate_to_trigger_force_sync(Some(&pod), timestamp);
        assert_matches!(
            result,
            Ok(MutationOutcome::RequirePatch(pod)) if pod == from_json!({
                "metadata": {
                    "annotations": {
                        "pod-graceful-drain/force-sync-nonce": "2025-03-13T00:00:00.123456789Z",
                    },
                },
            })
        );
    }
}
