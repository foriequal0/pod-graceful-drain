use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::ResourceExt;
use thiserror::Error;

use crate::labels_and_annotations;
use crate::labels_and_annotations::{
    DELETE_OPTIONS_ANNOTATION_KEY, DRAIN_TIMESTAMP_ANNOTATION_KEY, DRAINING_LABEL_KEY,
};

#[derive(Debug)]
pub enum PodDrainingState {
    None,
    WaitingForPodDisruptionBudget {
        controller: String,
        delete_options: DeleteOptions,
    },
    Draining {
        controller: String,
        drain_timestamp: DateTime<Utc>,
        delete_options: DeleteOptions,
    },
    DrainDisabled,
}

#[derive(Debug, Error)]
pub enum PodDrainingStateError {
    #[error("failed to parse pod draining state: {message}")]
    AnnotationParseError { message: String },
}

pub fn get_pod_draining_state(pod: &Pod) -> Result<PodDrainingState> {
    let Some(draining_label) = pod.labels().get(DRAINING_LABEL_KEY).map(|x| x.as_str()) else {
        return Ok(PodDrainingState::None);
    };

    if draining_label.eq_ignore_ascii_case("false")
        || draining_label == "0"
        || draining_label.is_empty()
    {
        return Ok(PodDrainingState::DrainDisabled);
    }

    if draining_label.eq_ignore_ascii_case("WaitingForPodDisruptionBudget") {
        let controller = pod
            .annotations()
            .get(labels_and_annotations::DRAIN_CONTROLLER_ANNOTATION_KEY);
    }

    if draining_label.eq_ignore_ascii_case("true")
        || draining_label.eq_ignore_ascii_case("draining")
    {
        let Some(str) = pod.annotations().get(DRAIN_TIMESTAMP_ANNOTATION_KEY) else {
            return Err(PodDrainingStateError::AnnotationParseError {
                message: format!("annotation '{DRAIN_TIMESTAMP_ANNOTATION_KEY}' not exists"),
            });
        };

        match DateTime::parse_from_rfc3339(str) {
            Ok(datetime) => {
                let utc = datetime.with_timezone(&Utc);
                PodDrainingState::DrainUntil(utc)
            }
            Err(err) => PodDrainingState::AnnotationParseError {
                message: format!(
                    "annotation '{DRAIN_TIMESTAMP_ANNOTATION_KEY}' has invalid format: {}",
                    err
                ),
            },
        }
    }
}

struct RawPodDrainingState {
    draining: DrainingLabelValue,
    controller: Option<String>,
    drain_until: Option<Result<DateTime<Utc>, chrono::ParseError>>,
    delete_options: Option<Result<DeleteOptions, serde_json::Error>>,
}

fn get_raw_pod_draining_state(pod: &Pod) -> RawPodDrainingState {
    let draining = get_draining_label(pod);
    let controller = pod
        .annotations()
        .get(labels_and_annotations::DRAIN_CONTROLLER_ANNOTATION_KEY)
        .map(|x| x.to_owned());
    let drain_until = get_drain_until_annotation(pod);
    let delete_options = get_delete_options(pod);

    return RawPodDrainingState {
        draining,
        controller,
        drain_until,
        delete_options,
    };

    fn get_draining_label(pod: &Pod) -> DrainingLabelValue {
        let Some(draining_label) = pod.labels().get(DRAINING_LABEL_KEY).map(|x| x.as_str()) else {
            return DrainingLabelValue::None;
        };

        if draining_label.eq_ignore_ascii_case("waiting") {
            return DrainingLabelValue::WaitingForPodDisruptionBudget;
        }

        if draining_label.eq_ignore_ascii_case("true") {
            return DrainingLabelValue::Draining;
        }

        DrainingLabelValue::Other(draining_label.to_owned())
    }

    fn get_drain_until_annotation(pod: &Pod) -> Option<Result<DateTime<Utc>, chrono::ParseError>> {
        let str = pod.annotations().get(DRAIN_TIMESTAMP_ANNOTATION_KEY)?;
        match DateTime::parse_from_rfc3339(str) {
            Ok(datetime) => {
                let utc = datetime.with_timezone(&Utc);
                Some(Ok(utc))
            }
            Err(err) => Some(Err(err)),
        }
    }

    fn get_delete_options(pod: &Pod) -> Option<Result<DeleteOptions, serde_json::Error>> {
        let annotation = pod.annotations().get(DELETE_OPTIONS_ANNOTATION_KEY)?;
        Some(serde_json::from_str(annotation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! from_json {
        ($($json:tt)+) => {
            ::serde_json::from_value(::serde_json::json!($($json)+)).expect("Invalid json")
        };
    }

    #[test]
    fn should_return_some_drain_until() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {
                    "pod-graceful-drain/drain-until": "2023-02-09T15:30:45Z",
                },
            }
        });

        let info = get_pod_draining_state(&pod);
        let expected = DateTime::parse_from_rfc3339("2023-02-09T15:30:45Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_matches!(info, PodDrainingState::DrainUntil(value) if value == expected);
    }

    #[test]
    fn should_return_none_with_draining_false() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "false",
                },
                "annotations": {
                    "pod-graceful-drain/drain-until": "2023-02-09T15:30:45Z",
                },
            }
        });

        let info = get_pod_draining_state(&pod);
        assert_matches!(info, PodDrainingState::DrainDisabled);
    }

    #[test]
    fn should_return_none_when_no_label() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {},
            }
        });

        let info = get_pod_draining_state(&pod);
        assert_matches!(info, PodDrainingState::None);
    }

    #[test]
    fn should_return_none_when_no_annotation() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {},
            }
        });

        let info = get_pod_draining_state(&pod);
        assert_matches!(info, PodDrainingState::AnnotationParseError { message: _ });
    }

    #[test]
    fn should_return_some_error_when_invalid_annotation() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {
                    "pod-graceful-drain/draining": "true",
                },
                "annotations": {
                    "pod-graceful-drain/drain-until": "INVALID",
                },
            }
        });

        let info = get_pod_draining_state(&pod);
        assert_matches!(info, PodDrainingState::AnnotationParseError { message: _ });
    }
}
