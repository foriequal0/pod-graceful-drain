use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;

use crate::consts::{DRAINING_LABEL_KEY, DRAIN_UNTIL_ANNOTATION_KEY};

#[derive(Debug)]
pub enum PodDrainingInfo {
    None,
    DrainUntil(DateTime<Utc>),
    DrainDisabled,
    AnnotationParseError { message: String },
}

pub fn get_pod_draining_info(pod: &Pod) -> PodDrainingInfo {
    if let Some(label) = pod.labels().get(DRAINING_LABEL_KEY) {
        if !label.eq_ignore_ascii_case("true") || label == "0" || label.is_empty() {
            return PodDrainingInfo::DrainDisabled;
        }
    } else {
        return PodDrainingInfo::None;
    }

    let Some(str) = pod.annotations().get(DRAIN_UNTIL_ANNOTATION_KEY) else {
        return PodDrainingInfo::AnnotationParseError {
            message: format!("annotation '{DRAIN_UNTIL_ANNOTATION_KEY}' not exists"),
        };
    };

    match DateTime::parse_from_rfc3339(str) {
        Ok(datetime) => {
            let utc = datetime.with_timezone(&Utc);
            PodDrainingInfo::DrainUntil(utc)
        }
        Err(err) => PodDrainingInfo::AnnotationParseError {
            message: format!(
                "annotation '{DRAIN_UNTIL_ANNOTATION_KEY}' has invalid format: {}",
                err
            ),
        },
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

        let info = get_pod_draining_info(&pod);
        let expected = DateTime::parse_from_rfc3339("2023-02-09T15:30:45Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_matches!(info, PodDrainingInfo::DrainUntil(value) if value == expected);
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

        let info = get_pod_draining_info(&pod);
        assert_matches!(info, PodDrainingInfo::DrainDisabled);
    }

    #[test]
    fn should_return_none_when_no_label() {
        let pod: Pod = from_json! ({
            "metadata": {
                "labels": {},
            }
        });

        let info = get_pod_draining_info(&pod);
        assert_matches!(info, PodDrainingInfo::None);
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

        let info = get_pod_draining_info(&pod);
        assert_matches!(info, PodDrainingInfo::AnnotationParseError { message: _ });
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

        let info = get_pod_draining_info(&pod);
        assert_matches!(info, PodDrainingInfo::AnnotationParseError { message: _ });
    }
}
