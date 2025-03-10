use chrono::{DateTime, Utc};
use eyre::Result;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;

use crate::LoadBalancingConfig;
use crate::api_resolver::ApiResolver;
use crate::labels_and_annotations::{
    DrainingLabelValue, get_pod_draining_label_value, set_pod_delete_options,
    set_pod_drain_controller, set_pod_evict_after, try_set_pod_draining_label_value,
};
use crate::patch::resource_patch_util::{MutationOutcome, patch};

#[derive(Debug)]
pub enum PatchToEvictOutcome {
    /// pod is gone
    Gone,
    /// waiting for pod disruption budget
    WaitingForPodDisruptionBudget,
    /// didn't patched. keep draining.
    Draining,
}

pub async fn patch_to_evict(
    pod: &Pod,
    api_resolver: &ApiResolver,
    loadbalancing: &LoadBalancingConfig,
    delete_options: &DeleteOptions,
) -> Result<PatchToEvictOutcome> {
    patch(api_resolver, pod, |pod| {
        mutate_to_evict(pod, Utc::now(), loadbalancing, delete_options)
    })
    .await
}

pub(super) fn mutate_to_evict(
    pod: Option<&Pod>,
    timetstamp: DateTime<Utc>,
    loadbalancing: &LoadBalancingConfig,
    delete_options: &DeleteOptions,
) -> Result<MutationOutcome<PatchToEvictOutcome, Pod>> {
    let Some(pod) = pod else {
        return Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Gone));
    };

    let draining_state = get_pod_draining_label_value(pod);
    match draining_state {
        Ok(Some(DrainingLabelValue::Evicting)) => {
            return Ok(MutationOutcome::DesiredState(
                PatchToEvictOutcome::WaitingForPodDisruptionBudget,
            ));
        }
        Ok(Some(DrainingLabelValue::Draining)) => {
            return Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Draining));
        }
        _ => {}
    }

    let mut pod = pod.clone();

    try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Evicting);
    set_pod_drain_controller(&mut pod, loadbalancing);
    set_pod_evict_after(&mut pod, Some(timetstamp));
    set_pod_delete_options(&mut pod, Some(delete_options))?;

    Ok(MutationOutcome::RequirePatch(pod))
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, Utc};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Preconditions;

    use crate::from_json;
    use crate::patch::drain;

    #[test]
    fn smoke_test() {
        let pod: Pod = from_json!({
            "metadata": {
                "labels": {
                    "app": "test",
                },
            }
        });

        let timestamp = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let delete_options = DeleteOptions {
            dry_run: Some(vec!["All".to_owned()]),
            propagation_policy: Some("Foreground".to_owned()),
            preconditions: Some(Preconditions {
                uid: Some("uid1234".to_owned()),
                resource_version: Some("version1234".to_owned()),
            }),
            grace_period_seconds: Some(30),
            ..DeleteOptions::default()
        };

        let result = mutate_to_evict(Some(&pod), timestamp, &loadbalancing, &delete_options);
        assert_matches!(
            result,
            Ok(MutationOutcome::RequirePatch(pod)) if pod == from_json!({
                "metadata": {
                    "labels": {
                        "app": "test",
                        "pod-graceful-drain/draining": "evicting",
                    },
                    "annotations": {
                        "pod-graceful-drain/controller": "instance-id-1",
                        "pod-graceful-drain/delete-options": "{\"dryRun\":[\"All\"],\"gracePeriodSeconds\":30,\"preconditions\":{\"resourceVersion\":\"version1234\",\"uid\":\"uid1234\"},\"propagationPolicy\":\"Foreground\"}",
                        "pod-graceful-drain/evict-after": "2025-03-13T00:00:00Z",
                    },
                },
            })
        );
    }

    #[test]
    fn should_return_gone_if_pod_is_none() {
        let timestamp = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let delete_options = DeleteOptions::default();

        let result = mutate_to_evict(None, timestamp, &loadbalancing, &delete_options);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Gone))
        );
    }

    #[test]
    fn should_be_idempotent() {
        let pod: Pod = from_json!({});

        let timestamp = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let delete_options = DeleteOptions::default();
        let result = mutate_to_evict(Some(&pod), timestamp, &loadbalancing, &delete_options);

        let Ok(MutationOutcome::RequirePatch(pod)) = result else {
            panic!("should patch pod");
        };

        let loadbalancing = LoadBalancingConfig::with_str("instance-id-2");
        let delete_options = DeleteOptions::default();
        let result = mutate_to_evict(Some(&pod), timestamp, &loadbalancing, &delete_options);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(
                PatchToEvictOutcome::WaitingForPodDisruptionBudget
            ))
        );
    }

    #[test]
    fn should_not_regress_from_draining() {
        let pod: Pod = from_json!({});

        let loadbalancing = LoadBalancingConfig::with_str("instance-id-1");
        let timestamp = DateTime::parse_from_rfc3339("2023-02-08T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let result = drain::mutate_to_drain(Some(&pod), timestamp, &loadbalancing, true);
        let Ok(MutationOutcome::RequirePatch(pod)) = result else {
            panic!("should patch pod");
        };

        let delete_options = DeleteOptions::default();
        let result = mutate_to_evict(Some(&pod), timestamp, &loadbalancing, &delete_options);

        assert_matches!(
            result,
            Ok(MutationOutcome::DesiredState(PatchToEvictOutcome::Draining))
        );
    }
}
