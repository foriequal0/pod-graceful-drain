use std::collections::btree_map::Entry;

use crate::LoadBalancingConfig;
use chrono::{DateTime, SecondsFormat, Utc};
use eyre::{Context, Result};
use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::ResourceExt;

pub const CONTROLLER_NAME: &str = "pod-graceful-drain";

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum DrainingLabelValue {
    Draining,
    WaitingForPodDisruptionBudget,
}

pub const DRAINING_LABEL_KEY: &str = "pod-graceful-drain/draining";
pub const DRAINING_LABEL_VALUE__DRAINING: &str = "true";
pub const DRAINING_LABEL_VALUE__WAITING: &str = "waiting";

pub fn get_pod_draining_label_value(pod: &Pod) -> Result<Option<DrainingLabelValue>, String> {
    let label = pod.labels().get(DRAINING_LABEL_KEY).map(|x| x.as_str());
    match label {
        None => Ok(None),
        Some(DRAINING_LABEL_VALUE__DRAINING) => Ok(Some(DrainingLabelValue::Draining)),
        Some(DRAINING_LABEL_VALUE__WAITING) => {
            Ok(Some(DrainingLabelValue::WaitingForPodDisruptionBudget))
        }
        Some(other) => Err(other.to_owned()),
    }
}

pub fn try_set_pod_draining_label_value(pod: &mut Pod, value: DrainingLabelValue) -> bool {
    if let Ok(Some(existing)) = get_pod_draining_label_value(pod) {
        if existing == value {
            // do not set if same
            return false;
        }
        if existing == DrainingLabelValue::Draining {
            // do not regress to waiting state
            return false;
        }
    }

    let str = match value {
        DrainingLabelValue::Draining => DRAINING_LABEL_VALUE__DRAINING,
        DrainingLabelValue::WaitingForPodDisruptionBudget => DRAINING_LABEL_VALUE__WAITING,
    };

    pod.labels_mut()
        .insert(String::from(DRAINING_LABEL_KEY), String::from(str));

    true
}

pub enum DrainTimestampValue {
    Value(DateTime<Utc>),
    Other(String),
    None,
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

const ORIGINAL_LABELS_ANNOTATION_KEY: &str = "pod-graceful-drain/original-labels";
pub fn try_backup_pod_original_labels(pod: &mut Pod) -> Result<bool> {
    let mut labels = pod.labels().clone();

    // remove previous draining labels from the backup
    match labels.get(DRAINING_LABEL_KEY) {
        Some(label) if label == "true" => {
            labels.remove(DRAINING_LABEL_KEY);
        }
        _ => {}
    }

    if labels.len() == 0 {
        return Ok(false);
    }

    let original_labels = serde_json::to_string(&labels).context("serializing labels")?;

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
                pod.labels_mut().clear();
                return Ok(true);
            }
        }
    }

    // give up after the key exhaustion
    Ok(false)
}

const DRAIN_CONTROLLER_ANNOTATION_KEY: &str = "pod-graceful-drain/controller";

pub fn get_pod_drain_controller(pod: &Pod) -> Option<&str> {
    pod.annotations()
        .get(DRAIN_CONTROLLER_ANNOTATION_KEY)
        .map(String::as_str)
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

pub enum PodDeleteOptionsValue {
    Overwrite(Option<DeleteOptions>),
    Preserve(DeleteOptions),
}

pub fn set_pod_delete_options(pod: &mut Pod, value: Option<&DeleteOptions>) -> Result<()> {
    match value {
        Some(value) => {
            let json = serde_json::to_string(value).context("serializing DeleteOptions")?;
            pod.annotations_mut()
                .insert(String::from(DELETE_OPTIONS_ANNOTATION_KEY), json);
        }

        None => {
            // preserve delete options if any
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
}
