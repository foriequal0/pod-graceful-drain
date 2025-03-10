use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, LabelSelectorRequirement};
use kube::{Resource, ResourceExt};
use tracing::error;

pub fn matches_selector(res: &impl Resource, selector: Option<&LabelSelector>) -> bool {
    // "A null label selector matches no objects."
    let Some(selector) = selector else {
        // null label selector
        return false;
    };

    // "The result of matchLabels and matchExpressions are ANDed."
    if !matches_labels(res, selector.match_labels.as_ref()) {
        return false;
    }

    if !matches_expressions(res, selector.match_expressions.as_deref()) {
        return false;
    }

    true
}

pub fn matches_labels(
    res: &impl Resource,
    match_labels: Option<&BTreeMap<String, String>>,
) -> bool {
    let labels = res.labels();

    // "An empty label selector matches all objects"
    let Some(match_labels) = match_labels else {
        return true;
    };

    for (key, value) in match_labels.iter() {
        if labels.get(key) != Some(value) {
            return false;
        }
    }

    true
}

pub fn matches_expressions(
    res: &impl Resource,
    match_expressions: Option<&[LabelSelectorRequirement]>,
) -> bool {
    let labels = res.labels();

    // "An empty label selector matches all objects"
    let Some(match_expressions) = match_expressions else {
        return true;
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

#[cfg(test)]
mod tests {
    use super::*;

    use k8s_openapi::api::core::v1::Pod;

    use crate::from_json;

    macro_rules! requirement {
        ($key:literal, $op:literal) => {
            LabelSelectorRequirement {
                key: String::from($key),
                operator: String::from($op),
                values: None,
            }
        };
        ($key:literal, $op:literal, $values:expr) => {
            LabelSelectorRequirement {
                key: String::from($key),
                operator: String::from($op),
                values: Some($values.into_iter().map(String::from).collect()),
            }
        };
    }

    #[test]
    fn test_matches_selector() {
        let empty_pod: Pod = from_json!({});
        let pod: Pod = from_json!({
            "metadata": {
                "labels": {
                    "app": "test",
                    "env": "dev",
                }
            }
        });

        {
            let selector = None;
            assert!(
                !matches_selector(&empty_pod, selector.as_ref()),
                "null label selector matches no objects"
            );
            assert!(
                !matches_selector(&pod, selector.as_ref()),
                "null label selector matches no objects"
            );
        }

        {
            let selector = Some(LabelSelector::default());
            assert!(
                matches_selector(&empty_pod, selector.as_ref()),
                "empty label selector should match all objects"
            );
            assert!(
                matches_selector(&pod, selector.as_ref()),
                "empty label selector should match all objects"
            );
        }

        {
            let selector = Some(LabelSelector {
                match_labels: Some(BTreeMap::from([("app".to_owned(), "test".to_owned())])),
                match_expressions: Some(vec![requirement!("env", "In", ["dev"])]),
            });
            assert!(
                matches_selector(&pod, selector.as_ref()),
                "The result of matchLabels and matchExpressions are ANDed."
            );
        }
    }

    #[test]
    fn test_matches_labels() {
        let empty_pod: Pod = from_json!({});
        let pod: Pod = from_json!({
            "metadata": {
                "labels": {
                    "app": "test",
                    "env": "dev",
                }
            }
        });

        {
            let match_labels = None;
            assert!(
                matches_labels(&empty_pod, match_labels.as_ref()),
                "empty label selector should match all objects"
            );
            assert!(
                matches_labels(&pod, match_labels.as_ref()),
                "empty label selector should match all objects"
            );
        }

        {
            let match_labels = Some(BTreeMap::new());
            assert!(
                matches_labels(&empty_pod, match_labels.as_ref()),
                "empty label selector"
            );
            assert!(
                matches_labels(&pod, match_labels.as_ref()),
                "empty label selector should match all objects"
            );
        }

        {
            let match_labels = Some(BTreeMap::from([("app".to_string(), "test".to_string())]));
            assert!(
                !matches_labels(&empty_pod, match_labels.as_ref()),
                "empty pod isn't selected by label selector"
            );
        }

        {
            let match_labels = Some(BTreeMap::from([("app".to_string(), "test".to_string())]));
            assert!(matches_labels(&pod, match_labels.as_ref()));
        }

        {
            let match_labels = Some(BTreeMap::from([
                ("app".to_string(), "test".to_string()),
                ("env".to_string(), "dev".to_string()),
            ]));
            assert!(
                matches_labels(&pod, match_labels.as_ref()),
                "requirements are ANDed"
            );
        }

        {
            let match_labels = Some(BTreeMap::from([("app".to_string(), "another".to_string())]));
            assert!(!matches_labels(&pod, match_labels.as_ref()));
        }

        {
            let match_labels = Some(BTreeMap::from([
                ("app".to_string(), "another".to_string()),
                ("env".to_string(), "dev".to_string()),
            ]));
            assert!(
                !matches_labels(&pod, match_labels.as_ref()),
                "requirements are ANDed"
            );
        }
    }

    #[test]
    fn test_matches_expressions() {
        let empty_pod: Pod = from_json!({});
        let pod: Pod = from_json!({
            "metadata": {
                "labels": {
                    "app": "test",
                    "env": "dev",
                }
            }
        });

        {
            let requirements = None;
            assert!(
                matches_expressions(&empty_pod, requirements),
                "empty label selector should match all objects"
            );
            assert!(
                matches_expressions(&pod, requirements),
                "empty label selector should match all objects"
            );
        }

        {
            let requirements: Option<&[LabelSelectorRequirement]> = Some(&[]);
            assert!(
                matches_expressions(&empty_pod, requirements),
                "empty label selector"
            );
            assert!(
                matches_expressions(&pod, requirements),
                "empty label selector should match all objects"
            );
        }

        {
            let requirements: Option<&[LabelSelectorRequirement]> = Some(&[]);
            assert!(
                matches_expressions(&empty_pod, requirements),
                "empty label selector"
            );
        }

        {
            let requirements = &[requirement!("app", "In", ["test"])];
            assert!(matches_expressions(&pod, Some(requirements)), "In - true");
        }

        {
            let requirements = &[requirement!("app", "In", ["another"])];
            assert!(!matches_expressions(&pod, Some(requirements)), "In - false");
        }

        {
            let requirements = &[requirement!("app", "NotIn", ["another"])];
            assert!(
                matches_expressions(&pod, Some(requirements)),
                "NotIn - true"
            );
        }

        {
            let requirements = &[requirement!("app", "NotIn", ["test"])];
            assert!(
                !matches_expressions(&pod, Some(requirements)),
                "NotIn - false"
            );
        }

        {
            let requirements = &[requirement!("app", "Exists")];
            assert!(
                matches_expressions(&pod, Some(requirements)),
                "Exists - true"
            );
        }

        {
            let requirements = &[requirement!("name", "Exists")];
            assert!(
                !matches_expressions(&pod, Some(requirements)),
                "Exists - false"
            );
        }

        {
            let requirements = &[requirement!("name", "DoesNotExist")];
            assert!(
                matches_expressions(&pod, Some(requirements)),
                "DoesNotExist - true"
            );
        }

        {
            let requirements = &[requirement!("app", "DoesNotExist")];
            assert!(
                !matches_expressions(&pod, Some(requirements)),
                "DoesNotExist - false"
            );
        }

        {
            let requirements = &[
                requirement!("app", "In", ["test"]),
                requirement!("env", "NotIn", ["prod"]),
            ];
            assert!(
                matches_expressions(&pod, Some(requirements)),
                "ANDed - true"
            );
        }

        {
            let requirements = &[
                requirement!("app", "In", ["test"]),
                requirement!("env", "NotIn", ["dev"]),
            ];
            assert!(
                !matches_expressions(&pod, Some(requirements)),
                "ANDed - false"
            );
        }
    }
}
