use std::sync::Arc;

use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;

use crate::error_types::NotMyFault;
use crate::selector::matches_selector;
use crate::{Stores, try_some};

pub mod budget;
pub mod fixup;

pub fn get_pdbs_for(stores: &Stores, pod: &Pod) -> Vec<Arc<PodDisruptionBudget>> {
    let pod_disruption_budgets = r#gen!({
        let pod_namespace = pod.metadata.namespace.as_deref().unwrap_or("default");
        for pdb in stores.pod_disruption_budgets(pod_namespace) {
            if matches_selector(pod, try_some!(pdb.spec?.selector?)) {
                yield_!(pdb);
            }
        }
    });

    pod_disruption_budgets.into_iter().collect()
}

pub fn get_pdb_for(
    stores: &Stores,
    pod: &Pod,
) -> Result<Option<Arc<PodDisruptionBudget>>, NotMyFault> {
    let pdbs = get_pdbs_for(stores, pod);
    if pdbs.is_empty() {
        return Ok(None);
    }

    if pdbs.len() > 1 {
        return Err(NotMyFault {
            message: String::from("does not support more than one pod disruption budget"),
            source: None,
        });
    }

    let pdb = pdbs[0].clone();
    Ok(Some(pdb))
}
