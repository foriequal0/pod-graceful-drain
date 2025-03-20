use std::fmt::Debug;
use std::hash::Hasher;
use std::ops::Range;
use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use kube::api::DynamicObject;
use kube::runtime::controller::Action;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{controller, watcher};
use rand::{Rng, SeedableRng};
use tracing::{debug, error, trace};

use crate::error_codes::{
    is_404_not_found_error, is_409_conflict_error, is_410_expired_error,
    is_410_expired_error_response, is_transient_error,
};
use crate::{LoadBalancingConfig, try_some};

pub fn get_stable_jitter(
    pod: &Pod,
    loadbalancing: &LoadBalancingConfig,
    range: Range<Duration>,
) -> Duration {
    let instance_id = loadbalancing.get_id();
    let pod_namespace = try_some!(pod.metadata.namespace?).map(|x| x.as_str());
    let pod_name = try_some!(pod.metadata.name?).map(|x| x.as_str());

    get_stable_jitter_impl(instance_id, pod_namespace, pod_name, range)
}

fn get_stable_jitter_impl(
    instance_id: &str,
    pod_namespace: Option<&str>,
    pod_name: Option<&str>,
    range: Range<Duration>,
) -> Duration {
    let mut hasher = std::hash::DefaultHasher::default();
    hasher.write(instance_id.as_bytes());
    if let Some(namespace) = pod_namespace {
        hasher.write(namespace.as_bytes());
    }
    if let Some(name) = pod_name {
        hasher.write(name.as_bytes());
    }

    let mut rng = rand::rngs::StdRng::seed_from_u64(hasher.finish());
    rng.random_range(range)
}

pub fn log_reconcile_result_common<E>(
    result: Result<(ObjectRef<Pod>, Action), controller::Error<E, watcher::Error>>,
    reconciler_failed_handler: impl Fn(E, ObjectRef<DynamicObject>),
) where
    E: Debug,
{
    match result {
        Ok((object_ref, action)) => {
            trace!(%object_ref, ?action, "apply");
        }
        Err(controller::Error::ReconcilerFailed(reconciler_err, object_ref)) => {
            reconciler_failed_handler(reconciler_err, object_ref);
        }
        Err(controller::Error::QueueError(queue_err)) => {
            match queue_err {
                watcher::Error::WatchFailed(err) => {
                    // restarting
                    trace!(?err, "watch fail on queue");
                }
                watcher::Error::WatchError(resp) if is_410_expired_error_response(&resp) => {
                    // reconciler is late
                    trace!(?resp, "expired on queue");
                }
                _ => error!(?queue_err, "error on queue"),
            }
        }
        Err(controller::Error::ObjectNotFound(object_ref)) => {
            // reconciler is late
            trace!(%object_ref, "not found");
        }
        Err(err) => {
            error!(?err, "error on controller");
        }
    }
}

pub fn log_reconcile_kube_err_common(err: kube::Error) {
    if is_409_conflict_error(&err) {
        debug!(%err, "conflict on reconcile");
    } else if is_404_not_found_error(&err) || is_410_expired_error(&err) {
        // reconcile function is late
        debug!(%err, "expired on reconcile");
    } else if is_transient_error(&err) {
        debug!(%err, "transient error");
    } else {
        error!(%err, "error on reconcile")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_jitter_should_stable() {
        let call = || {
            get_stable_jitter_impl(
                "instance_id",
                Some("namespace"),
                Some("name"),
                Default::default()..Duration::from_secs(10),
            )
        };

        let first = call();
        let second = call();

        assert_eq!(first, second);
    }
}
