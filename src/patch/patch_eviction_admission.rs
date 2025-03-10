use crate::patch::build_diff;
use eyre::Result;
use json_patch::Patch;
use k8s_openapi::api::policy::v1::Eviction;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;

pub fn make_patch_eviction_to_dry_run(eviction: &Eviction) -> Result<Patch> {
    return build_diff(eviction, set_dry_run);

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
