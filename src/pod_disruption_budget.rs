use std::collections::BTreeMap;
use std::sync::Arc;

use eyre::{Report, Result, eyre};
use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, LabelSelectorRequirement};
use kube::core::response::StatusCause;
use kube::runtime::reflector::Lookup;
use kube::{Resource, ResourceExt};
use thiserror::Error;
use tracing::error;

use crate::pod_state::is_pod_ready;
use crate::{ApiResolver, Stores, try_some};

#[derive(Error, Debug)]
pub enum DecreasePodDisruptionBudgetError {
    #[error("too many requests: {0:?}")]
    TooManyRequests(TooManyRequestsError),
    #[error(transparent)]
    Other(#[from] Report),
}

#[derive(Debug)]
pub struct TooManyRequestsError {
    message: String,
    causes: Vec<StatusCause>,
    retry_after_seconds: u32,
}

const DISRUPTION_BUDGET_CAUSE: &str = "DisruptionBudget";

/// defined in `kubernetes/pkg/registry/core/pod/storage/eviction.go`
const MAX_DISRUPTED_POD_SIZE: usize = 2000;

/// roughly follows `kubernetes/pkg/registry/core/pod/storage/eviction.go:Create`
pub async fn decrease_pod_disruption_budget(
    pod: &Pod,
    stores: &Stores,
    api_resolver: &ApiResolver,
) -> Result<(), DecreasePodDisruptionBudgetError> {
    let Some(mut pdb) = get_pdb(stores, pod)? else {
        // no pdb, so allowed to disrupt
        return Ok(());
    };

    match check_pod_disruption_policy(pod, &pdb)? {
        PodDisruptionPolicyResult::Evict => {
            return Ok(());
        }
        PodDisruptionPolicyResult::EvictIfBudgetAllows => {}
    }

    let Some(status) = &pdb.status else {
        return Err(eyre!("kubernetes bug: PodDisruptionBudget.status is empty").into());
    };

    if status.observed_generation.unwrap_or(0) < pdb.metadata.generation.unwrap_or(0) {
        return Err(DecreasePodDisruptionBudgetError::TooManyRequests(
            TooManyRequestsError {
                message: String::from(
                    "Cannot evict pod as it would violate the pod's disruption budget.",
                ),
                retry_after_seconds: 10,
                causes: vec![StatusCause {
                    reason: String::from(DISRUPTION_BUDGET_CAUSE),
                    message: format!(
                        "The disruption budget {} is still being processed by the server.",
                        pdb.metadata.name.as_deref().unwrap_or_default()
                    ),
                    field: String::new(),
                }],
            },
        ));
    }

    if status.disruptions_allowed < 0 {
        return Err(eyre!("pdb disruptions allowed is negative").into());
    }

    if status
        .disrupted_pods
        .as_ref()
        .map(|x| x.len())
        .unwrap_or_default()
        > MAX_DISRUPTED_POD_SIZE
    {
        return Err(eyre!(
            "DisruptedPods map too big - too many evictions not confirmed by PDB controller"
        )
        .into());
    }

    if status.disruptions_allowed == 0 {
        return Err(DecreasePodDisruptionBudgetError::TooManyRequests(
            TooManyRequestsError {
                message: String::from(
                    "Cannot evict pod as it would violate the pod's disruption budget.",
                ),
                retry_after_seconds: 1,
                causes: vec![StatusCause {
                    reason: String::from(DISRUPTION_BUDGET_CAUSE),
                    message: format!(
                        "The disruption budget {} needs {} healthy pods and has {} currently"
                        pdb.metadata.name.as_deref().unwrap_or_default(),
                        status.desired_healthy,
                        status.current_healthy,
                    ),
                    field: String::new(),
                }],
            },
        ));
    }

    Ok(())
}

fn get_pdb(stores: &Stores, pod: &Pod) -> Result<Option<Arc<PodDisruptionBudget>>> {
    let pod_disruption_budgets = r#gen!({
        let pod_namespace = pod.metadata.namespace.as_ref();
        for pdb in stores.pod_disruption_budgets() {
            if pdb.meta().namespace.as_ref() != pod_namespace {
                continue;
            }

            if matches_selector(pod, try_some!(pdb.spec?.selector?)) {
                yield_!(pdb);
            }
        }
    });

    let pdbs: Vec<_> = pod_disruption_budgets.into_iter().collect();
    if pdbs.len() == 0 {
        return Ok(None);
    }

    if pdbs.len() > 1 {
        return Err(eyre!(
            "does not support more than one pod disruption budget"
        ));
    }

    let pdb = pdbs[0].clone();
    Ok(Some(pdb))
}

fn matches_selector(pod: &Pod, selector: Option<&LabelSelector>) -> bool {
    let Some(selector) = selector else {
        return false;
    };

    if matches_labels(pod, selector.match_labels.as_ref()) {
        return true;
    }

    if matches_expressions(pod, selector.match_expressions.as_deref()) {
        return true;
    }

    false
}

fn matches_labels(pod: &Pod, match_labels: Option<&BTreeMap<String, String>>) -> bool {
    let labels = pod.labels();

    let Some(match_labels) = match_labels else {
        return false;
    };

    for (key, value) in match_labels.iter() {
        if labels.get(key) != Some(value) {
            return false;
        }
    }

    true
}

fn matches_expressions(pod: &Pod, match_expressions: Option<&[LabelSelectorRequirement]>) -> bool {
    let labels = pod.labels();

    let Some(match_expressions) = match_expressions else {
        return false;
    };

    for requirement in match_expressions {
        let key = requirement.key.as_str();
        let values = requirement.values.as_deref();
        match requirement.operator.as_str() {
            "In" => {
                let Some(value) = labels.get(key) else {
                    return false;
                };

                let Some(values) = values else {
                    error!(
                        "kubernetes bug: 'selector.matchExpressions[*].values' cannot be empty when 'operator' is 'In' or 'NotIn'"
                    );
                    continue;
                };

                let contains = values.contains(value);
                if !contains {
                    return false;
                }
            }
            "NotIn" => {
                let Some(value) = labels.get(key) else {
                    continue;
                };

                let Some(values) = values else {
                    error!(
                        "kubernetes bug: 'selector.matchExpressions[*].values' cannot be empty when 'operator' is 'In' or 'NotIn'"
                    );
                    continue;
                };

                let contains = values.contains(value);
                if contains {
                    return false;
                }
            }
            "Exists" => {
                if labels.get(key).is_none() {
                    return false;
                }
            }
            "DoesNotExist" => {
                if labels.get(key).is_some() {
                    return false;
                }
            }
            op => {
                error!("kubernetes bug: unexpected labelSelector operator '{}'", op);
            }
        }
    }

    true
}

enum PodDisruptionPolicyResult {
    Evict,
    EvictIfBudgetAllows,
}

fn check_pod_disruption_policy(
    pod: &Pod,
    pdb: &PodDisruptionBudget,
) -> Result<PodDisruptionPolicyResult> {
    mod unhealthy_pod_eviction_policy_type {
        pub const ALWAYS_ALLOW: &str = "AlwaysAllow";
        pub const IF_HEALTHY_BUDGET: &str = "IfHealthyBudget";
    }

    if is_pod_ready(pod) {
        return Ok(PodDisruptionPolicyResult::EvictIfBudgetAllows);
    }

    match try_some!(pdb.spec?.unhealthy_pod_eviction_policy*?) {
        Some(unhealthy_pod_eviction_policy_type::ALWAYS_ALLOW) => {
            // unhealthy pod can be disrupted
            Ok(PodDisruptionPolicyResult::Evict)
        }
        Some(unhealthy_pod_eviction_policy_type::IF_HEALTHY_BUDGET) => {
            // unhealthy pod can be disrupted when healthy budget allows
            let Some(status) = pdb.status.as_ref() else {
                return Err(eyre!(
                    "kubernetes bug: PodDisruptionBudget.status is not exist: {}",
                    pdb.to_object_ref(())
                ));
            };

            if status.current_healthy >= status.desired_healthy && status.desired_healthy > 0 {
                return Ok(PodDisruptionPolicyResult::Evict);
            }

            Ok(PodDisruptionPolicyResult::EvictIfBudgetAllows)
        }
        other => Err(eyre!(
            "kubernetes bug: PodDisruptionBudget.spec.unhealthyPodEvictionPolicy '{}' is invalid: {}",
            other.unwrap_or(""),
            pdb.to_object_ref(())
        )),
    }
}
