use std::hash::Hasher;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use eyre::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{DeleteParams, EvictParams, Preconditions};
use kube::runtime::controller::Action;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher::Config;
use kube::runtime::{controller, watcher, Controller};
use kube::{Api, ResourceExt};
use rand::{Rng, SeedableRng};
use thiserror::Error;
use tracing::{debug, error, info, span, trace, Level};

use crate::api_resolver::ApiResolver;
use crate::consts::DRAINING_LABEL_KEY;
use crate::error_codes::{
    is_404_not_found_error, is_409_conflict_error, is_410_expired_error, is_transient_error,
};
use crate::loadbalancing::LoadBalancingConfig;
use crate::pod_draining_info::{get_pod_draining_info, PodDrainingInfo};
use crate::pod_evict_params::get_pod_evict_params;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::{instrumented, try_some, ServiceRegistry};

/// Start a controller that deletes deregistered pods.
pub fn start_controller(
    api_resolver: &ApiResolver,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
    shutdown: &Shutdown,
) -> Result<()> {
    let api_resolver = api_resolver.clone();

    let context = Arc::new(ReconcilerContext {
        api_resolver: api_resolver.clone(),
        loadbalancing: loadbalancing.clone(),
    });

    let pods: Api<Pod> = api_resolver.all();
    let controller = Controller::new(pods, Config::default().labels(DRAINING_LABEL_KEY))
        .graceful_shutdown_on(shutdown.wait_shutdown_triggered());

    let signal = service_registry.register("controller");
    spawn_service(shutdown, "controller", {
        let shutdown = shutdown.clone();
        async move {
            signal.ready();
            controller
                .run(reconcile, error_policy, context)
                .take_until(shutdown.wait_shutdown_triggered())
                .for_each(log_reconcile_result)
                .await
        }
    })?;

    Ok(())
}

struct ReconcilerContext {
    api_resolver: ApiResolver,
    loadbalancing: LoadBalancingConfig,
}

#[derive(Error, Debug)]
enum ReconcileError {
    #[error("kube error: {0}")]
    KubeError(#[from] kube::Error),
}

const CONTROLLER_EXCLUSIVE_DURATION: Duration = Duration::from_secs(10);
const CONTROLLER_TIMEOUT_JITTER: Duration = Duration::from_secs(10);
const DEFAULT_TRANSIENT_ERROR_RECONCILE: Duration = Duration::from_secs(5);
const DEFAULT_RECONCILE_DURATION: Duration = Duration::from_secs(3600);

async fn reconcile(
    pod: Arc<Pod>,
    context: Arc<ReconcilerContext>,
) -> Result<Action, ReconcileError> {
    let span = span!(Level::ERROR, "reconciler");
    instrumented!(span, async move {
        if pod.metadata.deletion_timestamp.is_some() {
            return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
        }

        if let PodDrainingInfo::DrainUntil(drain_until) = get_pod_draining_info(&pod) {
            if context.loadbalancing.controls(&pod) {
                let remaining = drain_until - Utc::now();
                if let Ok(remaining) = remaining.to_std() {
                    return Ok(Action::requeue(remaining));
                }
            } else {
                // Let the original controller handle first.
                let controller_exclusive_until = drain_until + CONTROLLER_EXCLUSIVE_DURATION;
                let jitter = get_stable_jitter(&pod, &context);
                let jittered = controller_exclusive_until + jitter;
                let remaining = jittered - Utc::now();
                if let Ok(remaining) = remaining.to_std() {
                    return Ok(Action::requeue(remaining));
                }
            }

            // TODO: possible bottleneck of the reconciler.
            if let Some(evict_params) = get_pod_evict_params(&pod) {
                evict_pod(&context.api_resolver, &pod, &evict_params).await?
            } else {
                delete_pod(&context.api_resolver, &pod).await?
            };
        };

        Ok(Action::requeue(DEFAULT_RECONCILE_DURATION))
    })
}

fn get_stable_jitter(pod: &Pod, context: &ReconcilerContext) -> Duration {
    let instance_id = context.loadbalancing.get_id();
    let pod_namespace = try_some!(pod.metadata.namespace?).map(|x| x.as_str());
    let pod_name = try_some!(pod.metadata.name?).map(|x| x.as_str());

    get_stable_jitter_impl(instance_id, pod_namespace, pod_name)
}

fn get_stable_jitter_impl(
    instance_id: &str,
    pod_namespace: Option<&str>,
    pod_name: Option<&str>,
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
    rng.random_range(Default::default()..CONTROLLER_TIMEOUT_JITTER)
}

fn error_policy(pod: Arc<Pod>, err: &ReconcileError, _context: Arc<ReconcilerContext>) -> Action {
    let span = span!(Level::ERROR, "reconciler::error_policy");
    let _ = span.enter();
    match err {
        ReconcileError::KubeError(err) => {
            if is_409_conflict_error(err) {
                return Action::requeue(Duration::from_secs(1));
            }

            if is_transient_error(err) {
                let object_ref = ObjectRef::from_obj(pod.as_ref());
                info!(%object_ref, ?err, "retry transient error");
                return Action::requeue(DEFAULT_TRANSIENT_ERROR_RECONCILE);
            }
        }
    }

    Action::requeue(Duration::from_secs(5))
}

async fn log_reconcile_result(
    result: Result<(ObjectRef<Pod>, Action), controller::Error<ReconcileError, watcher::Error>>,
) {
    let span = span!(Level::ERROR, "reconciler");
    instrumented!(span, async move {
        match result {
            Ok((object_ref, action)) => {
                trace!(%object_ref, ?action, "success");
            }
            Err(controller::Error::ReconcilerFailed(err, object_ref)) => match err {
                ReconcileError::KubeError(err) if is_409_conflict_error(&err) => {
                    debug!(%object_ref, ?err, "conflict");
                }
                ReconcileError::KubeError(err)
                    if is_404_not_found_error(&err) || is_410_expired_error(&err) =>
                {
                    // reconciler is late
                    debug!(%object_ref, ?err, "gone");
                }
                _ => error!(%object_ref, ?err, "error"),
            },
            Err(controller::Error::ObjectNotFound(object_ref)) => {
                // reconciler is late
                debug!(%object_ref, "gone");
            }
            Err(err) => {
                error!(?err, "error");
            }
        }
    })
}

async fn delete_pod(api_resolver: &ApiResolver, pod: &Pod) -> kube::Result<()> {
    let api = api_resolver.api_for(pod);
    let name = pod.name_any();

    let delete_params = DeleteParams {
        preconditions: Some(Preconditions {
            uid: pod.uid(),
            ..Preconditions::default()
        }),
        ..DeleteParams::default()
    };

    debug!("deleting pod");
    let result = api.delete(&name, &delete_params).await;
    match result {
        Ok(_) => {
            info!("pod is deleted");
            Ok(())
        }
        Err(err) if is_404_not_found_error(&err) || is_410_expired_error(&err) => {
            debug!("pod is gone anyway"); // This is what we desired.
            Ok(())
        }
        Err(err) => Err(err),
    }
}

async fn evict_pod(
    api_resolver: &ApiResolver,
    pod: &Pod,
    evict_params: &EvictParams,
) -> kube::Result<()> {
    let api = api_resolver.api_for(pod);
    let name = pod.name_any();

    debug!("evicting pod");
    let result = api.evict(&name, evict_params).await;
    match result {
        Ok(_) => {
            info!("pod is evicted");
            Ok(())
        }
        Err(err) if is_404_not_found_error(&err) || is_410_expired_error(&err) => {
            debug!("pod is gone anyway"); // This is what we desired.
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_jitter_should_stable() {
        let first = get_stable_jitter_impl("instance_id", Some("namespace"), Some("name"));
        let second = get_stable_jitter_impl("instance_id", Some("namespace"), Some("name"));

        assert_eq!(first, second);
    }
}
