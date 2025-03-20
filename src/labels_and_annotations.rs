use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use crate::LoadBalancingConfig;
use crate::error_types::Bug;
use chrono::{DateTime, SecondsFormat, Utc};
use eyre::Result;
use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::ResourceExt;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum DrainingLabelValue {
    Draining,
    Evicting,
}

pub const DRAINING_LABEL_KEY: &str = "pod-graceful-drain/draining";
pub const DRAINING_LABEL_VALUE__DRAINING: &str = "true";
pub const DRAINING_LABEL_VALUE__EVICTING: &str = "evicting";

pub fn get_pod_draining_label_value(pod: &Pod) -> Result<Option<DrainingLabelValue>, String> {
    let label = pod.labels().get(DRAINING_LABEL_KEY).map(|x| x.as_str());
    match label {
        None => Ok(None),
        Some(DRAINING_LABEL_VALUE__DRAINING) => Ok(Some(DrainingLabelValue::Draining)),
        Some(DRAINING_LABEL_VALUE__EVICTING) => Ok(Some(DrainingLabelValue::Evicting)),
        Some(other) => Err(other.to_owned()),
    }
}

pub fn try_set_pod_draining_label_value(pod: &mut Pod, value: DrainingLabelValue) -> bool {
    if let Ok(Some(existing)) = get_pod_draining_label_value(pod) {
        if existing == value {
            // do not set if same
            return true;
        }

        if existing == DrainingLabelValue::Draining && value == DrainingLabelValue::Evicting {
            // do not regress to waiting state
            return false;
        }
    }

    let str = match value {
        DrainingLabelValue::Draining => DRAINING_LABEL_VALUE__DRAINING,
        DrainingLabelValue::Evicting => DRAINING_LABEL_VALUE__EVICTING,
    };

    pod.labels_mut()
        .insert(String::from(DRAINING_LABEL_KEY), String::from(str));

    true
}

const DRAIN_TIMESTAMP_ANNOTATION_KEY: &str = "pod-graceful-drain/drain-timestamp";

pub fn get_pod_drain_timestamp(pod: &Pod) -> Result<Option<DateTime<Utc>>, String> {
    let Some(str) = pod.annotations().get(DRAIN_TIMESTAMP_ANNOTATION_KEY) else {
        return Ok(None);
    };

    let result = DateTime::parse_from_rfc3339(str);
    match result {
        Ok(datetime) => Ok(Some(datetime.with_timezone(&Utc))),
        Err(_) => Err(str.to_owned()),
    }
}

pub fn try_set_pod_drain_timestamp(pod: &mut Pod, value: DateTime<Utc>) -> bool {
    let str = value.to_rfc3339_opts(SecondsFormat::Secs, true);
    match pod
        .annotations_mut()
        .entry(String::from(DRAIN_TIMESTAMP_ANNOTATION_KEY))
    {
        Entry::Vacant(entry) => {
            entry.insert(str);
            true
        }
        Entry::Occupied(mut entry) => {
            let existing = DateTime::parse_from_rfc3339(entry.get()).map(|x| x.with_timezone(&Utc));
            if existing.is_err() {
                // TODO: report error recovery
                entry.insert(str);
                true
            } else {
                // do not change timestamp after set
                false
            }
        }
    }
}

pub const EVICT_AFTER_ANNOTATION_KEY: &str = "pod-graceful-drain/evict-after";

pub fn get_pod_evict_after(pod: &Pod) -> Result<Option<DateTime<Utc>>, String> {
    let Some(str) = pod.annotations().get(EVICT_AFTER_ANNOTATION_KEY) else {
        return Ok(None);
    };

    let result = DateTime::parse_from_rfc3339(str);
    match result {
        Ok(datetime) => Ok(Some(datetime.with_timezone(&Utc))),
        Err(_) => Err(str.to_owned()),
    }
}

pub fn set_pod_evict_after(pod: &mut Pod, value: Option<DateTime<Utc>>) {
    if let Some(value) = value {
        let str = value.to_rfc3339_opts(SecondsFormat::Secs, true);
        pod.annotations_mut()
            .insert(String::from(EVICT_AFTER_ANNOTATION_KEY), str);
    } else if let Some(annotations) = &mut pod.metadata.annotations {
        annotations.remove(EVICT_AFTER_ANNOTATION_KEY);
    }
}

const ORIGINAL_LABELS_ANNOTATION_KEY: &str = "pod-graceful-drain/original-labels";
pub fn try_backup_pod_original_labels(pod: &mut Pod) -> Result<bool, Bug> {
    let mut to_backup = pod.labels().clone();
    let mut to_retain = BTreeMap::new();

    // remove previous draining labels from the backup
    match to_backup.get(DRAINING_LABEL_KEY) {
        Some(label)
            if label == DRAINING_LABEL_VALUE__DRAINING
                || label == DRAINING_LABEL_VALUE__EVICTING =>
        {
            to_retain.insert(DRAINING_LABEL_KEY.to_owned(), label.to_owned());
            to_backup.remove(DRAINING_LABEL_KEY);
        }
        _ => {}
    }

    if to_backup.is_empty() {
        return Ok(false);
    }

    let original_labels = serde_json::to_string(&to_backup).map_err(|err| Bug {
        message: "failed to serialize original labels".to_owned(),
        source: Some(err.into()),
    })?;

    let annotations = pod.annotations_mut();
    let annotation_keys = r#gen!({
        yield_!(String::from(ORIGINAL_LABELS_ANNOTATION_KEY));
        for i in 1..10 {
            yield_!(format!("{ORIGINAL_LABELS_ANNOTATION_KEY}_{i}"));
        }
    });

    for key in annotation_keys.into_iter() {
        match annotations.entry(key) {
            Entry::Occupied(_) => {
                continue;
            }
            Entry::Vacant(entry) => {
                entry.insert(original_labels);
                *pod.labels_mut() = to_retain;
                return Ok(true);
            }
        }
    }

    // give up after the key exhaustion
    Ok(false)
}

const DRAIN_CONTROLLER_ANNOTATION_KEY: &str = "pod-graceful-drain/controller";

pub fn am_i_pod_drain_controller(pod: &Pod, loadbalancing: &LoadBalancingConfig) -> bool {
    let Some(controller) = pod
        .annotations()
        .get(DRAIN_CONTROLLER_ANNOTATION_KEY)
        .map(String::as_str)
    else {
        return false;
    };

    controller == loadbalancing.get_id()
}

pub fn set_pod_drain_controller(pod: &mut Pod, loadbalancing: &LoadBalancingConfig) {
    pod.annotations_mut().insert(
        String::from(DRAIN_CONTROLLER_ANNOTATION_KEY),
        loadbalancing.get_id().to_owned(),
    );
}

const DELETE_OPTIONS_ANNOTATION_KEY: &str = "pod-graceful-drain/delete-options";

pub fn get_pod_delete_options(pod: &Pod) -> Result<Option<DeleteOptions>, String> {
    let Some(json) = pod.annotations().get(DELETE_OPTIONS_ANNOTATION_KEY) else {
        return Ok(None);
    };

    let result = serde_json::from_str(json);
    match result {
        Ok(delete_options) => Ok(delete_options),
        Err(_) => Err(json.to_owned()),
    }
}

pub fn set_pod_delete_options(pod: &mut Pod, config: Option<&DeleteOptions>) -> Result<(), Bug> {
    match config {
        Some(value) => {
            let json = serde_json::to_string(value).map_err(|err| Bug {
                message: "failed to serialize delete options".to_owned(),
                source: Some(err.into()),
            })?;
            pod.annotations_mut()
                .insert(String::from(DELETE_OPTIONS_ANNOTATION_KEY), json);
        }
        None => {
            pod.annotations_mut().remove(DELETE_OPTIONS_ANNOTATION_KEY);
        }
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Preconditions;

    use crate::from_json;

    #[test]
    fn test_get_pod_draining_label_value() {
        {
            let result = get_pod_draining_label_value(&from_json!({}));
            assert_matches!(result, Ok(None));
        }

        {
            let result = get_pod_draining_label_value(&from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "evicting"
                    }
                }
            }));
            assert_matches!(result, Ok(Some(DrainingLabelValue::Evicting)));
        }

        {
            let result = get_pod_draining_label_value(&from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true"
                    }
                }
            }));
            assert_matches!(result, Ok(Some(DrainingLabelValue::Draining)));
        }

        {
            let result = get_pod_draining_label_value(&from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "asdf"
                    }
                }
            }));
            assert_matches!(result, Err(str) if str == "asdf");
        }
    }

    #[test]
    fn test_set_pod_draining_label_value() {
        {
            let mut pod = Pod::default();
            assert!(try_set_pod_draining_label_value(
                &mut pod,
                DrainingLabelValue::Evicting,
            ));
            assert_eq!(
                get_pod_draining_label_value(&pod),
                Ok(Some(DrainingLabelValue::Evicting)),
                "should round-trip"
            );
        }

        {
            let mut pod = Pod::default();
            assert!(try_set_pod_draining_label_value(
                &mut pod,
                DrainingLabelValue::Draining
            ));
            assert_eq!(
                get_pod_draining_label_value(&pod),
                Ok(Some(DrainingLabelValue::Draining)),
                "should round-trip"
            );
        }

        {
            let mut pod = Pod::default();
            assert!(try_set_pod_draining_label_value(
                &mut pod,
                DrainingLabelValue::Evicting
            ));
            assert!(
                try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Draining),
                "should progress"
            );
            assert_eq!(
                get_pod_draining_label_value(&pod),
                Ok(Some(DrainingLabelValue::Draining)),
                "should progress"
            );
        }

        {
            let mut pod = Pod::default();
            assert!(try_set_pod_draining_label_value(
                &mut pod,
                DrainingLabelValue::Draining
            ));
            assert!(
                !try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Evicting),
                "should not regress progress"
            );

            assert!(
                try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Draining),
                "overwriting the same value should return true"
            );
            assert!(
                try_set_pod_draining_label_value(&mut pod, DrainingLabelValue::Draining),
                "should not regress progress"
            );
        }
    }

    #[test]
    fn test_get_pod_drain_timestamp() {
        let result = get_pod_drain_timestamp(&from_json!({}));
        assert_matches!(result, Ok(None));

        let result = get_pod_drain_timestamp(&from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/drain-timestamp": "2025-03-12T00:00:00Z"
                }
            }
        }));
        assert_matches!(result,
            Ok(Some(datetime)) if datetime == DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc));

        let result = get_pod_drain_timestamp(&from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/drain-timestamp": "invalid"
                }
            }
        }));
        assert_matches!(result, Err(str) if str == "invalid");
    }

    #[test]
    fn test_set_pod_drain_timestamp() {
        {
            let mut pod = Pod::default();
            let timestamp = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            assert!(
                try_set_pod_drain_timestamp(&mut pod, timestamp),
                "should set"
            );
            assert_eq!(
                get_pod_drain_timestamp(&pod),
                Ok(Some(timestamp)),
                "should round-trip"
            );
        }

        {
            let mut pod = Pod::default();

            let timestamp1 = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            let timestamp2 = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            try_set_pod_drain_timestamp(&mut pod, timestamp1);

            assert!(
                !try_set_pod_drain_timestamp(&mut pod, timestamp2),
                "should not update timestamp"
            );

            assert_eq!(
                get_pod_drain_timestamp(&pod),
                Ok(Some(timestamp1)),
                "should not update timestamp"
            );
        }

        {
            let mut pod = from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/drain-timestamp": "invalid"
                }
            }});

            let timestamp = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            assert!(
                try_set_pod_drain_timestamp(&mut pod, timestamp),
                "should recover from invalid timestamp"
            );
            assert_eq!(
                get_pod_drain_timestamp(&pod),
                Ok(Some(timestamp)),
                "should recover from invalid timestamp"
            );
        }
    }

    #[test]
    fn test_get_pod_evict_after() {
        let result = get_pod_evict_after(&from_json!({}));
        assert_matches!(result, Ok(None));

        let result = get_pod_evict_after(&from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/evict-after": "2025-03-12T00:00:00Z"
                }
            }
        }));
        assert_matches!(result,
            Ok(Some(datetime)) if datetime == DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc));

        let result = get_pod_evict_after(&from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/evict-after": "invalid"
                }
            }
        }));
        assert_matches!(result, Err(str) if str == "invalid");
    }

    #[test]
    fn test_set_pod_evict_after() {
        {
            let mut pod = Pod::default();
            let timestamp = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            set_pod_evict_after(&mut pod, Some(timestamp));
            assert_eq!(
                get_pod_evict_after(&pod),
                Ok(Some(timestamp)),
                "should round-trip"
            );
        }

        {
            let mut pod = Pod::default();

            let timestamp1 = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            let timestamp2 = DateTime::parse_from_rfc3339("2025-03-13T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            set_pod_evict_after(&mut pod, Some(timestamp1));
            set_pod_evict_after(&mut pod, Some(timestamp2));

            assert_eq!(
                get_pod_evict_after(&pod),
                Ok(Some(timestamp2)),
                "should update timestamp"
            );
        }

        {
            let mut pod = Pod::default();

            let timestamp = DateTime::parse_from_rfc3339("2025-03-12T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            set_pod_evict_after(&mut pod, Some(timestamp));
            set_pod_evict_after(&mut pod, None);

            assert_eq!(
                get_pod_evict_after(&pod),
                Ok(None),
                "should remove timestamp"
            );
        }
    }

    #[test]
    fn test_try_backup_pod_original_labels() {
        {
            let mut pod = from_json!({
                "metadata": {
                    "annotations": {
                        "some-annotation": "some-value"
                    }
                }
            });

            try_backup_pod_original_labels(&mut pod).unwrap();

            assert_eq!(
                pod,
                from_json!({
                    "metadata": {
                        "annotations": {
                            "some-annotation": "some-value"
                        }
                    }
                }),
                "nothing to backup"
            );
        }

        {
            let mut pod = from_json!({
                "metadata": {
                    "labels": {
                        "app": "test",
                    },
                    "annotations": {
                        "some-annotation": "some-value"
                    }
                }
            });

            try_backup_pod_original_labels(&mut pod).unwrap();

            assert_eq!(
                pod,
                from_json!({
                    "metadata": {
                        "labels": {},
                        "annotations": {
                            "pod-graceful-drain/original-labels": "{\"app\":\"test\"}",
                            "some-annotation": "some-value"
                        }
                    }
                }),
                "should backup labels"
            );
        }

        {
            let mut pod = from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                    },
                }
            });

            try_backup_pod_original_labels(&mut pod).unwrap();

            assert_eq!(
                pod,
                from_json!({
                    "metadata": {
                        "labels": {
                            "pod-graceful-drain/draining": "true",
                        },
                    }
                }),
                "should not backup pod-graceful-drain/draining label"
            );
        }

        {
            let mut pod = from_json!({
                "metadata": {
                    "labels": {
                        "pod-graceful-drain/draining": "true",
                        "app": "test",
                    },
                    "annotations": {
                        "some-annotation": "some-value",
                        "pod-graceful-drain/original-labels": "",
                        "pod-graceful-drain/original-labels_1": "",
                        "pod-graceful-drain/original-labels_2": "",
                        "pod-graceful-drain/original-labels_3": "",
                        "pod-graceful-drain/original-labels_4": "",
                        "pod-graceful-drain/original-labels_5": "",
                        "pod-graceful-drain/original-labels_6": "",
                        "pod-graceful-drain/original-labels_7": "",
                        "pod-graceful-drain/original-labels_8": "",
                    }
                }
            });

            try_backup_pod_original_labels(&mut pod).unwrap();

            assert_eq!(
                pod,
                from_json!({
                    "metadata": {
                        "labels": {
                            "pod-graceful-drain/draining": "true",
                        },
                        "annotations": {
                            "some-annotation": "some-value",
                            "pod-graceful-drain/original-labels": "",
                            "pod-graceful-drain/original-labels_1": "",
                            "pod-graceful-drain/original-labels_2": "",
                            "pod-graceful-drain/original-labels_3": "",
                            "pod-graceful-drain/original-labels_4": "",
                            "pod-graceful-drain/original-labels_5": "",
                            "pod-graceful-drain/original-labels_6": "",
                            "pod-graceful-drain/original-labels_7": "",
                            "pod-graceful-drain/original-labels_8": "",
                            "pod-graceful-drain/original-labels_9": "{\"app\":\"test\"}"
                        }
                    }
                }),
                "avoid name collision"
            );
        }
    }

    #[test]
    fn test_am_i_pod_drain_controller() {
        {
            let pod = from_json!({
                "metadata": {
                    "annotations": {
                        "pod-graceful-drain/controller": "test"
                    }
                }
            });
            let loadbalancing = LoadBalancingConfig::with_str("test");
            assert!(am_i_pod_drain_controller(&pod, &loadbalancing))
        }

        {
            let pod = from_json!({
                "metadata": {
                    "annotations": {
                        "pod-graceful-drain/controller": "test"
                    }
                }
            });
            let loadbalancing = LoadBalancingConfig::with_str("another");
            assert!(!am_i_pod_drain_controller(&pod, &loadbalancing))
        }

        {
            let pod = from_json!({});
            let loadbalancing = LoadBalancingConfig::with_str("test");
            assert!(!am_i_pod_drain_controller(&pod, &loadbalancing))
        }
    }

    #[test]
    fn test_set_pod_drain_controller() {
        {
            let mut pod = Pod::default();
            let loadbalancing = LoadBalancingConfig::with_str("test");
            set_pod_drain_controller(&mut pod, &loadbalancing);
            assert!(am_i_pod_drain_controller(&pod, &loadbalancing))
        }

        {
            let mut pod = Pod::default();
            let loadbalancing1 = LoadBalancingConfig::with_str("test");
            let loadbalancing2 = LoadBalancingConfig::with_str("another");
            set_pod_drain_controller(&mut pod, &loadbalancing1);
            set_pod_drain_controller(&mut pod, &loadbalancing2);
            assert!(
                am_i_pod_drain_controller(&pod, &loadbalancing2),
                "can overwrite"
            )
        }
    }

    #[test]
    fn test_get_delete_options() {
        {
            let pod = from_json!({});
            assert_eq!(get_pod_delete_options(&pod), Ok(None))
        }

        {
            let pod = from_json!({
                "metadata": {
                    "annotations": {
                        "pod-graceful-drain/delete-options": "{}"
                    }
                }
            });
            assert_eq!(
                get_pod_delete_options(&pod),
                Ok(Some(DeleteOptions::default()))
            )
        }

        {
            let pod = from_json!({
                "metadata": {
                    "annotations": {
                        "pod-graceful-drain/delete-options": "{\"dryRun\":[\"All\"],\"gracePeriodSeconds\":30,\"preconditions\":{\"resourceVersion\":\"version1234\",\"uid\":\"uid1234\"},\"propagationPolicy\":\"Foreground\"}"
                    }
                }
            });
            assert_eq!(
                get_pod_delete_options(&pod),
                Ok(Some(DeleteOptions {
                    dry_run: Some(vec!["All".to_owned()]),
                    propagation_policy: Some("Foreground".to_owned()),
                    preconditions: Some(Preconditions {
                        uid: Some("uid1234".to_owned()),
                        resource_version: Some("version1234".to_owned()),
                    }),
                    grace_period_seconds: Some(30),
                    ..DeleteOptions::default()
                }))
            );
        }
    }

    #[test]
    fn test_set_delete_options() {
        {
            let mut pod = from_json!({});
            set_pod_delete_options(&mut pod, Some(&DeleteOptions::default())).unwrap();

            assert_eq!(
                get_pod_delete_options(&pod),
                Ok(Some(DeleteOptions::default()))
            )
        }

        {
            let mut pod = from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/delete-options": "{\"dryRun\":[\"All\"]}"
                }
            }});
            set_pod_delete_options(&mut pod, Some(&DeleteOptions::default())).unwrap();
            assert_eq!(
                get_pod_delete_options(&pod),
                Ok(Some(DeleteOptions::default()))
            )
        }

        {
            let mut pod = from_json!({
            "metadata": {
                "annotations": {
                    "pod-graceful-drain/delete-options": "{}"
                }
            }});
            set_pod_delete_options(&mut pod, None).unwrap();
            assert_eq!(get_pod_delete_options(&pod), Ok(None))
        }
    }
}
