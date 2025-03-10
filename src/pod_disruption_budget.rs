use std::fmt::Display;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use eyre::Result;
use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::api::PostParams;
use kube::runtime::reflector::Lookup;
use kube::{Api, Resource};
use thiserror::Error;
use tracing::error;

use crate::error_codes::is_404_not_found_error;
use crate::error_types::{Bug, NotMyFault};
use crate::pod_state::is_pod_ready;
use crate::selector::matches_selector;
use crate::{ApiResolver, Stores, try_some};

#[derive(Debug, Error)]
pub enum DecreasePodDisruptionBudgetError {
    #[error("too many requests")]
    TooManyRequests(#[from] TooManyRequestsError),
    #[error("kube error")]
    Kube(#[from] kube::Error),
    #[error(transparent)]
    Bug(#[from] Bug),
    #[error(transparent)]
    NotMyFault(#[from] NotMyFault),
}

#[derive(Debug, Error)]
pub struct TooManyRequestsError {
    pub message: String,
    pub causes: Vec<String>,
    pub retry_after_seconds: u32,
}

impl Display for TooManyRequestsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        for cause in &self.causes {
            write!(f, "\n{}", cause)?;
        }
        Ok(())
    }
}

/// defined in `kubernetes/pkg/registry/core/pod/storage/eviction.go`
const MAX_DISRUPTED_POD_SIZE: usize = 2000;

/// roughly follows `kubernetes/pkg/registry/core/pod/storage/eviction.go:Create`
pub async fn decrease_pod_disruption_budget(
    pod: &Pod,
    stores: &Stores,
    api_resolver: &ApiResolver,
) -> Result<(), DecreasePodDisruptionBudgetError> {
    let pod_namespace = pod.meta().namespace.clone().ok_or_else(|| Bug {
        message: "pod should have namespace".to_owned(),
        source: None,
    })?;
    let pod_name = pod.meta().name.clone().ok_or_else(|| Bug {
        message: "pod should have name".to_owned(),
        source: None,
    })?;

    let Some(pdb) = get_pdb(stores, pod)? else {
        // no pdb, so allowed to disrupt
        return Ok(());
    };

    let pdb_name = pdb.meta().name.clone().ok_or_else(|| NotMyFault {
        message: "pdb should have name".to_owned(),
        source: None,
    })?;

    match check_pod_disruption_policy(pod, &pdb)? {
        PodDisruptionPolicyResult::Evict => {
            return Ok(());
        }
        PodDisruptionPolicyResult::EvictIfBudgetAllows => {}
    }

    let mut pdb = pdb.as_ref().clone();
    check_and_decrease(&pod_name, &mut pdb)?;

    // replace(PUT) will take care of resourceVersion
    // https://kubernetes.io/docs/reference/using-api/api-concepts/#patch-and-apply
    let api: Api<PodDisruptionBudget> = api_resolver.namespaced(&pod_namespace);
    let data = serde_json::to_vec(&pdb).map_err(|err| NotMyFault {
        message: "failed to serialize pdb".to_owned(),
        source: Some(err.into()),
    })?;
    let result = api
        .replace_status(&pdb_name, &PostParams::default(), data)
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_404_not_found_error(&err) => {
            // PDB is gone anyway, allowed to disrupt
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

fn get_pdb(stores: &Stores, pod: &Pod) -> Result<Option<Arc<PodDisruptionBudget>>, NotMyFault> {
    let pod_disruption_budgets = r#gen!({
        let pod_namespace = pod.metadata.namespace.as_deref();
        for pdb in stores.pod_disruption_budgets() {
            if pdb.meta().namespace.as_deref() != pod_namespace {
                continue;
            }

            if matches_selector(pod, try_some!(pdb.spec?.selector?)) {
                yield_!(pdb);
            }
        }
    });

    let pdbs: Vec<_> = pod_disruption_budgets.into_iter().collect();
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

#[derive(Debug)]
enum PodDisruptionPolicyResult {
    Evict,
    EvictIfBudgetAllows,
}

mod unhealthy_pod_eviction_policy_type {
    pub const ALWAYS_ALLOW: &str = "AlwaysAllow";
    pub const IF_HEALTHY_BUDGET: &str = "IfHealthyBudget";
}

fn check_pod_disruption_policy(
    pod: &Pod,
    pdb: &PodDisruptionBudget,
) -> Result<PodDisruptionPolicyResult, NotMyFault> {
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
            if let Some(status) = pdb.status.as_ref() {
                if status.current_healthy >= status.desired_healthy && status.desired_healthy > 0 {
                    return Ok(PodDisruptionPolicyResult::Evict);
                }
            };

            Ok(PodDisruptionPolicyResult::EvictIfBudgetAllows)
        }
        other => Err(NotMyFault {
            message: format!(
                "kubernetes bug: PodDisruptionBudget.spec.unhealthyPodEvictionPolicy '{}' is invalid: {}",
                other.unwrap_or(""),
                pdb.to_object_ref(())
            ),
            source: None,
        }),
    }
}

fn check_and_decrease(
    pod_name: &str,
    pdb: &mut PodDisruptionBudget,
) -> Result<(), DecreasePodDisruptionBudgetError> {
    check_and_decrease_impl(pod_name, pdb, Utc::now())
}

fn check_and_decrease_impl(
    pod_name: &str,
    pdb: &mut PodDisruptionBudget,
    now: DateTime<Utc>,
) -> Result<(), DecreasePodDisruptionBudgetError> {
    let status = pdb.status.get_or_insert_default();

    if status.observed_generation.unwrap_or(0) < pdb.metadata.generation.unwrap_or(0) {
        return Err(DecreasePodDisruptionBudgetError::TooManyRequests(
            TooManyRequestsError {
                message: String::from(
                    "Cannot evict pod as it would violate the pod's disruption budget.",
                ),
                retry_after_seconds: 10,
                causes: vec![format!(
                    "The disruption budget {} is still being processed by the server.",
                    pdb.metadata.name.as_deref().unwrap_or_default(),
                )],
            },
        ));
    }

    if status.disruptions_allowed < 0 {
        return Err(NotMyFault {
            message: String::from("pdb disruptions allowed is negative"),
            source: None,
        }
        .into());
    }

    if status
        .disrupted_pods
        .as_ref()
        .map(|x| x.len())
        .unwrap_or_default()
        > MAX_DISRUPTED_POD_SIZE
    {
        return Err(NotMyFault {
            message: String::from(
                "DisruptedPods map too big - too many evictions not confirmed by PDB controller",
            ),
            source: None,
        }
        .into());
    }

    if status.disruptions_allowed == 0 {
        return Err(DecreasePodDisruptionBudgetError::TooManyRequests(
            TooManyRequestsError {
                message: String::from(
                    "Cannot evict pod as it would violate the pod's disruption budget.",
                ),
                retry_after_seconds: 0,
                causes: vec![format!(
                    "The disruption budget {} needs {} healthy pods and has {} currently",
                    pdb.metadata.name.as_deref().unwrap_or_default(),
                    status.desired_healthy,
                    status.current_healthy,
                )],
            },
        ));
    }

    status.disruptions_allowed -= 1;
    if status.disruptions_allowed == 0 {
        update_disruption_allowed_condition(status, now);
    }

    let disrupted_pods = status.disrupted_pods.get_or_insert_default();
    disrupted_pods.insert(pod_name.to_owned(), Time(now));

    Ok(())
}

const DISRUPTION_ALLOWED_CONDITION: &str = "DisruptionAllowed";

const SUFFICIENT_PODS_REASON: &str = "SufficientPods";
const INSUFFICIENT_PODS_REASON: &str = "InsufficientPods";

const CONDITION_TRUE: &str = "True";
const CONDITION_FALSE: &str = "False";

fn update_disruption_allowed_condition(status: &mut PodDisruptionBudgetStatus, now: DateTime<Utc>) {
    let conditions = status.conditions.get_or_insert_default();

    if status.disruptions_allowed > 0 {
        set_status_condition(
            conditions,
            Condition {
                type_: DISRUPTION_ALLOWED_CONDITION.to_owned(),
                reason: SUFFICIENT_PODS_REASON.to_owned(),
                status: CONDITION_TRUE.to_owned(),
                observed_generation: status.observed_generation,

                // default value
                last_transition_time: TIME_ZERO,
                message: String::new(),
            },
            now,
        );
    } else {
        set_status_condition(
            conditions,
            Condition {
                type_: DISRUPTION_ALLOWED_CONDITION.to_owned(),
                reason: INSUFFICIENT_PODS_REASON.to_owned(),
                status: CONDITION_FALSE.to_owned(),
                observed_generation: status.observed_generation,

                // default value
                last_transition_time: TIME_ZERO,
                message: String::new(),
            },
            now,
        );
    }
}

const TIME_ZERO: Time = Time(
    NaiveDateTime::new(
        NaiveDate::from_ymd_opt(1, 1, 1).expect("valid NaiveDate"),
        NaiveTime::from_hms_opt(0, 0, 0).expect("valid NaiveTime"),
    )
    .and_utc(),
);

fn set_status_condition(
    conditions: &mut Vec<Condition>,
    mut new_condition: Condition,
    now: DateTime<Utc>,
) -> bool {
    let existing_condition = conditions
        .iter_mut()
        .find(|c| c.type_ == new_condition.type_);
    let existing_condition = if let Some(existing_condition) = existing_condition {
        existing_condition
    } else {
        if new_condition.last_transition_time == TIME_ZERO {
            new_condition.last_transition_time = Time(now);
        }
        conditions.push(new_condition);
        return true;
    };

    let mut changed = false;
    if existing_condition.status != new_condition.status {
        existing_condition.status = new_condition.status;
        if new_condition.last_transition_time != TIME_ZERO {
            existing_condition.last_transition_time = new_condition.last_transition_time;
        } else {
            existing_condition.last_transition_time = Time(now);
        }
        changed = true;
    }

    if existing_condition.reason != new_condition.reason {
        existing_condition.reason = new_condition.reason;
        changed = true;
    }

    if existing_condition.message != new_condition.message {
        existing_condition.message = new_condition.message;
        changed = true;
    }

    if existing_condition.observed_generation != new_condition.observed_generation {
        existing_condition.observed_generation = new_condition.observed_generation;
        changed = true;
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::hash::Hash;

    use kube::runtime::reflector::{Store, store};
    use kube::runtime::watcher::Event;

    use crate::from_json;

    fn store_from<K>(iter: impl IntoIterator<Item = K>) -> Store<K>
    where
        K: 'static + Resource + Clone,
        K::DynamicType: Hash + Eq + Clone + Default,
    {
        let (reader, mut writer) = store();
        writer.apply_watcher_event(&Event::Init);
        for item in iter.into_iter() {
            writer.apply_watcher_event(&Event::InitApply(item));
        }
        writer.apply_watcher_event(&Event::InitDone);
        reader
    }

    #[test]
    fn test_get_pdb() {
        let pdb1: PodDisruptionBudget = from_json!({
            "metadata": {
                "name": "pdb1",
                "namespace": "ns1",
            },
            "spec": {
                "selector": {
                    "matchLabels": {
                        "app": "app1",
                    },
                },
            },
        });
        let pdb2: PodDisruptionBudget = from_json!({
            "metadata": {
                "name": "pdb2",
                "namespace": "ns1",
            },
            "spec": {
                "selector": {
                    "matchLabels": {
                        "app": "app2",
                    },
                },
            },
        });
        let pdb3: PodDisruptionBudget = from_json!({
            "metadata": {
                "name": "pdb1",
                "namespace": "ns2",
            },
            "spec": {
                "selector": {
                    "matchLabels": {
                        "app": "app1",
                    },
                },
            },
        });
        let stores = Stores::new(
            store_from([]),
            store_from([]),
            store_from([]),
            store_from([pdb1.clone(), pdb2.clone(), pdb3.clone()]),
            store_from([]),
        );

        let pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns1",
                "labels": {
                    "app": "app1",
                }
            },
        });

        let result = get_pdb(&stores, &pod);
        assert_matches!(result, Ok(Some(pdb)) if pdb.as_ref() == &pdb1);
    }

    #[test]
    fn test_check_pod_disruption_policy() {
        let always_allow: PodDisruptionBudget = from_json!({
            "spec": {
                "unhealthyPodEvictionPolicy": "AlwaysAllow",
            },
        });
        let if_healthy_budget_enough: PodDisruptionBudget = from_json!({
            "spec": {
                "unhealthyPodEvictionPolicy": "IfHealthyBudget",
            },
            "status": {
                "currentHealthy": 1,
                "desiredHealthy": 1,
            },
        });
        let if_healthy_budget_short: PodDisruptionBudget = from_json!({
            "spec": {
                "unhealthyPodEvictionPolicy": "IfHealthyBudget",
            },
            "status": {
                "currentHealthy": 0,
                "desiredHealthy": 1,
            },
        });

        {
            let healthy_pod: Pod = from_json!({
                "status": {
                    "conditions": [
                        {
                            "type": "Ready",
                            "status": "True",
                        },
                    ],
                },
            });

            for pdb in [
                &always_allow,
                &if_healthy_budget_enough,
                &if_healthy_budget_short,
            ] {
                let result = check_pod_disruption_policy(&healthy_pod, pdb);
                assert_matches!(
                    result,
                    Ok(PodDisruptionPolicyResult::EvictIfBudgetAllows),
                    "healthy pod should respect PDB"
                );
            }
        }

        let unhealthy_pod: Pod = from_json!({});
        {
            let result = check_pod_disruption_policy(&unhealthy_pod, &always_allow);
            assert_matches!(
                result,
                Ok(PodDisruptionPolicyResult::Evict),
                "unhealthy pod should always be evicted"
            );
        }

        {
            let result = check_pod_disruption_policy(&unhealthy_pod, &if_healthy_budget_enough);
            assert_matches!(
                result,
                Ok(PodDisruptionPolicyResult::Evict),
                "unhealthy pod should be evicted if budget is enough"
            );
        }

        {
            let result = check_pod_disruption_policy(&unhealthy_pod, &if_healthy_budget_short);
            assert_matches!(
                result,
                Ok(PodDisruptionPolicyResult::EvictIfBudgetAllows),
                "unhealthy pod should respect PDB if budget is short"
            );
        }
    }

    #[test]
    fn test_check_and_decrement() {
        {
            let mut pdb: PodDisruptionBudget = from_json!({
                "metadata": {
                    "generation": 2,
                },
                "status": {
                    "observedGeneration": 1,
                },
            });
            assert_matches!(
                check_and_decrease("test", &mut pdb),
                Err(DecreasePodDisruptionBudgetError::TooManyRequests(_)),
                "should return TooManyRequests if pdb controller is lagging, "
            );
        }

        {
            let mut pdb: PodDisruptionBudget = from_json!({
                "status": {
                    "disruptionsAllowed": -1,
                },
            });
            assert_matches!(
                check_and_decrease("test", &mut pdb),
                Err(_),
                "disruptionAllowed should not be negative"
            );
            assert_matches!(
                try_some!(pdb.status?.disruptions_allowed),
                Some(-1),
                "should not decrease disruption budget if it's already 0"
            )
        }

        {
            let mut pdb: PodDisruptionBudget = from_json!({
                "status": {
                    "disruptionsAllowed": 0,
                },
            });
            assert_matches!(
                check_and_decrease("test", &mut pdb),
                Err(DecreasePodDisruptionBudgetError::TooManyRequests(_)),
                "should not decrease disruption budget if it's already 0"
            );
            assert_matches!(
                try_some!(pdb.status?.disruptions_allowed),
                Some(0),
                "should not decrease disruption budget if it's already 0"
            )
        }

        {
            let mut pdb: PodDisruptionBudget = from_json!({
                "status": {
                    "disruptionsAllowed": 2,
                },
            });
            let now = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            assert_matches!(
                check_and_decrease_impl("test", &mut pdb, now),
                Ok(()),
                "should decrease disruption budget"
            );
            assert_eq!(
                pdb,
                from_json!({
                    "status": {
                        "disruptionsAllowed": 1,
                        "disruptedPods": {
                            "test": "2025-03-13T00:00:00Z",
                        },
                    },
                }),
                "should decrease disruption budget"
            )
        }

        {
            let mut pdb: PodDisruptionBudget = from_json!({
                "status": {
                    "observedGeneration": 123,
                    "disruptionsAllowed": 1,
                },
            });
            let now = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            assert_matches!(
                check_and_decrease_impl("test", &mut pdb, now),
                Ok(()),
                "should decrease disruption budget"
            );
            assert_eq!(
                pdb,
                from_json!({
                    "status": {
                        "observedGeneration": 123,
                        "disruptionsAllowed": 0,
                        "disruptedPods": {
                            "test": "2025-03-13T00:00:00Z",
                        },
                        "conditions": [
                            {
                                "type": "DisruptionAllowed",
                                "reason": "InsufficientPods",
                                "status": "False",
                                "observedGeneration": 123,
                                "lastTransitionTime": "2025-03-13T00:00:00Z",
                                "message": "",
                            },
                        ],
                    },
                }),
                "should decrease disruption budget and set conditions"
            )
        }
    }
}
