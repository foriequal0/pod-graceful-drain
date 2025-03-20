use std::collections::{HashMap, HashSet};

use genawaiter::{rc::r#gen, yield_};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::Resource;
use kube::runtime::reflector::ObjectRef;

use crate::elbv2::TARGET_HEALTH_POD_CONDITION_TYPE_PREFIX;
use crate::elbv2::apis::TargetType;
use crate::reflector::Stores;
use crate::selector::matches_labels;
use crate::{Config, try_some};

pub fn is_pod_running(pod: &Pod) -> bool {
    mod pod_phase {
        pub const POD_PENDING: &str = "Pending";
        const _POD_RUNNING: &str = "Running";
        pub const POD_SUCCEEDED: &str = "Succeeded";
        pub const POD_FAILED: &str = "Failed";
    }

    if let Some(pod_phase::POD_PENDING | pod_phase::POD_SUCCEEDED | pod_phase::POD_FAILED) =
        try_some!(pod.status?.phase*?)
    {
        return false;
    }

    if pod.meta().deletion_timestamp.is_some() {
        return false;
    }

    true
}

pub fn is_pod_ready(pod: &Pod) -> bool {
    let readiness_gates = {
        let mut result = HashSet::new();
        // "Ready" is required even if not listed in readiness gate
        result.insert("Ready");

        // readiness_gates are reflected to "Ready" state, so this is unnecessary
        if let Some(readiness_gates) = try_some!(pod.spec?.readiness_gates?) {
            for readiness_gate in readiness_gates {
                result.insert(readiness_gate.condition_type.as_str());
            }
        }

        result
    };

    let conditions = {
        let mut result = HashMap::new();
        if let Some(conditions) = try_some!(pod.status?.conditions?) {
            for condition in conditions {
                result.insert(condition.type_.as_str(), condition.status.as_str());
            }
        }

        result
    };

    for readiness_gate in readiness_gates {
        if !matches!(conditions.get(readiness_gate), Some(&"True")) {
            return false;
        }
    }

    true
}

pub fn is_pod_exposed(config: &Config, stores: &Stores, pod: &Pod) -> bool {
    // TODO: Find better way to determine whether a pod is exposed.
    // e.g. Examine EndpointSlice, etc.
    if config.experimental_general_ingress {
        is_exposed_by_ingress(stores, pod)
    } else {
        is_exposed_by_target_group_binding(stores, pod)
    }
}

fn is_exposed_by_ingress(stores: &Stores, pod: &Pod) -> bool {
    // TODO: Build inverted index in reconciler incrementally?
    let ingress_exposed_services = r#gen!({
        let mut seen = HashSet::new();
        let pod_namespace = pod.metadata.namespace.as_deref().unwrap_or("default");
        for ingress in stores.ingresses(pod_namespace) {
            if let Some(default_service_name) =
                try_some!(&ingress.spec?.default_backend?.service?.name)
            {
                let service_ref = ObjectRef::new(default_service_name).within(pod_namespace);
                if !seen.insert(service_ref.clone()) {
                    continue;
                }
                yield_!(service_ref);
            }

            for rule in try_some!(ingress.spec?.rules?).unwrap_or(&vec![]) {
                for path in try_some!(&rule.http?.paths).unwrap_or(&vec![]) {
                    if let Some(service_name) = try_some!(&path.backend.service?.name) {
                        let service_ref = ObjectRef::new(service_name).within(pod_namespace);
                        if !seen.insert(service_ref.clone()) {
                            continue;
                        }
                        yield_!(service_ref);
                    }
                }
            }
        }
    });

    ingress_exposed_services
        .into_iter()
        .any(|service_ref| is_exposing_service(stores, pod, service_ref))
}

fn is_exposed_by_target_group_binding(stores: &Stores, pod: &Pod) -> bool {
    // TODO: Build inverted index in reconciler incrementally?
    let tgb_exposed_service = r#gen!({
        let mut seen = HashSet::new();
        let pod_namespace = pod.metadata.namespace.as_deref().unwrap_or("default");
        for tgb in stores.target_group_bindings(pod_namespace) {
            if try_some!(tgb.spec?.target_type?) != Some(&TargetType::Ip) {
                continue;
            }

            if let Some(service_name) = try_some!(&tgb.spec?.service_ref?.name) {
                let service_ref = ObjectRef::new(service_name).within(pod_namespace);
                if !seen.insert(service_ref.clone()) {
                    continue;
                }

                yield_!(service_ref);
            }
        }
    });

    let is_exposed_by_tgb = tgb_exposed_service
        .into_iter()
        .any(|service_ref| is_exposing_service(stores, pod, service_ref));
    if is_exposed_by_tgb {
        return true;
    }

    // The pod once had corresponding TargetGroupBinding, but it is somehow gone.
    // We don't know whether its TargetType was IP or not.
    // But, true is more conservative than false.
    try_some!(pod.spec?.readiness_gates?)
        .unwrap_or(&vec![])
        .iter()
        .any(|readiness_gate| {
            readiness_gate
                .condition_type
                .starts_with(TARGET_HEALTH_POD_CONDITION_TYPE_PREFIX)
        })
}

fn is_exposing_service(stores: &Stores, pod: &Pod, service_ref: ObjectRef<Service>) -> bool {
    let Some(service) = stores.get_service(&service_ref) else {
        return false;
    };

    let selector_labels = try_some!(service.spec?.selector?);
    matches_labels(pod, selector_labels)
}

#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::time::Duration;

    use kube::runtime::reflector::{Store, store};
    use kube::runtime::watcher::Event;

    use super::*;
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

    fn get_test_experimental_general_ingress_config() -> Config {
        Config {
            experimental_general_ingress: true,
            delete_after: Duration::from_secs(30),
        }
    }

    #[test]
    fn pod_is_ready() {
        assert!(is_pod_ready(&from_json!({
            "status": {
                "conditions": [
                    {
                        "status": "True",
                        "type": "Ready"
                    },
                ],
            }
        })));

        assert!(!is_pod_ready(&from_json!({
            "status": {
                "conditions": [
                    {
                        "status": "False",
                        "type": "Ready"
                    },
                ],
            }
        })));

        assert!(is_pod_ready(&from_json!({
            "status": {
                "conditions": [
                    {
                        "status": "False",
                        "type": "some-unknown-condition"
                    },
                    {
                        "status": "True",
                        "type": "Ready"
                    },
                ],
            }
        })));

        assert!(!is_pod_ready(&from_json!({
            "spec": {
                "readinessGates": [
                    {
                        "conditionType": "some-readiness-gate-condition"
                    },
                ],
            },
            "status": {
                "conditions": [
                    {
                        "status": "False",
                        "type": "some-readiness-gate-condition"
                    },
                    {
                        "status": "True",
                        "type": "Ready"
                    },
                ],
            }
        })));

        assert!(is_pod_ready(&from_json!({
            "spec": {
                "readinessGates": [
                    {
                        "conditionType": "some-readiness-gate-condition"
                    },
                ],
            },
            "status": {
                "conditions": [
                    {
                        "status": "True",
                        "type": "some-readiness-gate-condition"
                    },
                    {
                        "status": "True",
                        "type": "Ready"
                    },
                ],
            }
        })));
    }

    #[test]
    fn pod_is_exposed() {
        let pod: Pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns",
                "labels": {
                    "app": "test"
                }
            },
        });

        let service = from_json!({
            "metadata": {
                "name": "svc",
                "namespace": "ns",
            },
            "spec": {
                "selector": {
                    "app": "test",
                },
            },
        });

        let ingress = from_json!({
            "metadata": {
                "name": "ig",
                "namespace": "ns",
            },
            "spec": {
                "rules": [{
                    "http": {
                        "paths": [{
                            "backend": {
                                "service": {
                                    "name": "svc",
                                },
                            },
                        }],
                    },
                }],
            }
        });

        let stores = Stores::new(
            store_from([pod.clone()]),
            store_from([service]),
            store_from([ingress]),
            store_from([]),
            store_from([]),
        );

        assert!(is_pod_exposed(
            &get_test_experimental_general_ingress_config(),
            &stores,
            &pod
        ))
    }

    #[test]
    fn pod_is_exposed_by_tgb() {
        let pod: Pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns",
                "labels": {
                    "app": "test"
                }
            },
        });

        let service = from_json!({
            "metadata": {
                "name": "svc",
                "namespace": "ns",
            },
            "spec": {
                "selector": {
                    "app": "test",
                },
            },
        });

        let tgb = from_json!({
            "metadata": {
                "name": "tgb",
                "namespace": "ns",
            },
            "spec": {
                "networking": {
                    // snip
                },
                "serviceRef": {
                    "name": "svc",
                    "port": "http"
                },
                "targetGroupARN": "some-target-group-arn",
                "targetType": "ip"
            }
        });

        let stores = Stores::new(
            store_from([pod.clone()]),
            store_from([service]),
            store_from([]),
            store_from([]),
            store_from([tgb]),
        );

        assert!(is_pod_exposed(
            &Config {
                delete_after: Duration::from_secs(30),
                experimental_general_ingress: false,
            },
            &stores,
            &pod
        ))
    }

    #[test]
    fn pod_is_not_exposed_when_no_ingress() {
        let pod: Pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns",
                "labels": {
                    "app": "test"
                }
            },
        });

        let service = from_json!({
            "metadata": {
                "name": "svc",
                "namespace": "ns",
            },
            "spec": {
                "selector": {
                    "app": "test",
                },
            },
        });

        let stores = Stores::new(
            store_from([pod.clone()]),
            store_from([service]),
            store_from([]),
            store_from([]),
            store_from([]),
        );

        assert!(!is_pod_exposed(
            &get_test_experimental_general_ingress_config(),
            &stores,
            &pod
        ))
    }

    #[test]
    fn pod_is_not_exposed_when_selector_not_match() {
        let pod: Pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns",
                "labels": {
                    "app": "test"
                }
            },
        });

        let service = from_json!({
            "metadata": {
                "name": "svc",
                "namespace": "ns",
            },
            "spec": {
                "selector": {
                    "app": "test",
                    "another": "another",
                },
            },
        });

        let ingress = from_json!({
            "metadata": {
                "name": "ig",
                "namespace": "ns",
            },
            "spec": {
                "rules": [{
                    "http": {
                        "paths": [{
                            "backend": {
                                "service": {
                                    "name": "svc",
                                },
                            },
                        }],
                    },
                }],
            }
        });

        let stores = Stores::new(
            store_from([pod.clone()]),
            store_from([service]),
            store_from([ingress]),
            store_from([]),
            store_from([]),
        );

        assert!(!is_pod_exposed(
            &get_test_experimental_general_ingress_config(),
            &stores,
            &pod
        ))
    }

    #[test]
    fn pod_is_not_exposed_namespace_differ() {
        let pod: Pod = from_json!({
            "metadata": {
                "name": "pod",
                "namespace": "ns2",
                "labels": {
                    "app": "test"
                }
            },
        });

        let service = from_json!({
            "metadata": {
                "name": "svc",
                "namespace": "ns",
            },
            "spec": {
                "selector": {
                    "app": "test",
                },
            },
        });

        let ingress = from_json!({
            "metadata": {
                "name": "ig",
                "namespace": "ns",
            },
            "spec": {
                "rules": [{
                    "http": {
                        "paths": [{
                            "backend": {
                                "service": {
                                    "name": "svc",
                                },
                            },
                        }],
                    },
                }],
            }
        });

        let stores = Stores::new(
            store_from([pod.clone()]),
            store_from([service]),
            store_from([ingress]),
            store_from([]),
            store_from([]),
        );

        assert!(!is_pod_exposed(
            &get_test_experimental_general_ingress_config(),
            &stores,
            &pod
        ))
    }
}
