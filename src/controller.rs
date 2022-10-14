use std::ops::Add;
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
use rand::Rng;
use thiserror::Error;
use tracing::{debug, error, info, span, trace, Level};

use crate::api_resolver::ApiResolver;
use crate::consts::DRAINING_LABEL_KEY;
use crate::loadbalancing::LoadBalancingConfig;
use crate::pod_draining_info::{get_pod_draining_info, PodDrainingInfo};
use crate::pod_evict_params::get_pod_evict_params;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::status::{
    is_404_not_found_error, is_409_conflict_error, is_410_gone_error, is_transient_error,
};
use crate::{instrumented, ServiceRegistry};

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
    let span = span!(Level::ERROR, "reconciler", object_ref = %ObjectRef::from_obj(pod.as_ref()));
    instrumented!(span, async move {
        if let PodDrainingInfo::DrainUntil(drain_until) = get_pod_draining_info(&pod) {
            let remaining = drain_until - Utc::now();
            if let Ok(remaining) = remaining.to_std() {
                return Ok(Action::requeue(remaining));
            }

            let expire = (-remaining).to_std().expect("should be expired");
            if expire < CONTROLLER_EXCLUSIVE_DURATION && !context.loadbalancing.controls(&pod) {
                // Let the original controller handle first.
                let requeue_duration = rand::thread_rng().gen_range(
                    CONTROLLER_EXCLUSIVE_DURATION
                        ..CONTROLLER_EXCLUSIVE_DURATION.add(CONTROLLER_TIMEOUT_JITTER),
                );

                return Ok(Action::requeue(requeue_duration));
            }

            // TODO: possible bottleneck of the reconciler.
            let result = if let Some(evict_params) = get_pod_evict_params(&pod) {
                evict_pod(&context.api_resolver, &pod, &evict_params).await
            } else {
                delete_pod(&context.api_resolver, &pod).await
            };

            if let Err(err) = result {
                if is_transient_error(&err) {
                    return Ok(Action::requeue(DEFAULT_TRANSIENT_ERROR_RECONCILE));
                }
            }
        };

        Ok(Action::requeue(DEFAULT_RECONCILE_DURATION))
    })
}

fn error_policy(_pod: Arc<Pod>, err: &ReconcileError, _context: Arc<ReconcilerContext>) -> Action {
    match err {
        ReconcileError::KubeError(err) => {
            if is_409_conflict_error(err) {
                return Action::requeue(Duration::from_secs(1));
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
                _ => error!(%object_ref, ?err, "error"),
            },
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

    info!("deleting pod");
    let result = api.delete(&name, &delete_params).await;
    match result {
        Ok(_) => {
            debug!("pod is deleted");
            Ok(())
        }
        Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
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

    info!("evicting pod");
    let result = api.evict(&name, evict_params).await;
    match result {
        Ok(_) => {
            debug!("pod is evicted");
            Ok(())
        }
        Err(err) if is_404_not_found_error(&err) || is_410_gone_error(&err) => {
            debug!("pod is gone anyway"); // This is what we desired.
            Ok(())
        }
        Err(err) => Err(err),
    }
}
