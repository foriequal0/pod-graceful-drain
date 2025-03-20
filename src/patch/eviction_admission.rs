use eyre::Result;
use json_patch::Patch;
use k8s_openapi::api::policy::v1::Eviction;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;

use crate::patch::resource_patch_util::build_patch;

pub fn make_patch_eviction_to_dry_run(eviction: &Eviction) -> Result<Patch> {
    let mut modified = eviction.clone();
    set_dry_run(&mut modified);
    return build_patch(eviction, &modified);

    fn set_dry_run(eviction: &mut Eviction) {
        let delete_options = eviction.delete_options.clone().unwrap_or_default();
        eviction.delete_options = Some(DeleteOptions {
            dry_run: Some(vec![String::from("All")]),
            ..delete_options
        });
    }
}

#[cfg(test)]
mod tests {
    use eyre::Result;
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::from_json;

    fn apply<K>(res: &K, patch: &Patch) -> Result<Value>
    where
        K: Serialize,
    {
        let mut modified = serde_json::to_value(res)?;
        json_patch::patch(&mut modified, &patch.0)?;
        Ok(modified)
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
