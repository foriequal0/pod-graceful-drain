use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use eyre::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::controller::Action;
use kube::runtime::events::{EventType, Recorder};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{Controller, controller, watcher};
use kube::{Api, Resource};
use thiserror::Error;
use tracing::{Level, debug, error, span};

use crate::api_resolver::ApiResolver;
use crate::controllers::utils::{
    get_stable_jitter, log_reconcile_kube_err_common, log_reconcile_result_common,
};
use crate::error_codes::{is_409_conflict_error, is_transient_error};
use crate::labels_and_annotations::{
    DRAINING_LABEL_KEY, DRAINING_LABEL_VALUE__EVICTING, DrainingLabelValue,
    am_i_pod_drain_controller, get_pod_draining_label_value, get_pod_evict_after,
};
use crate::loadbalancing::LoadBalancingConfig;
use crate::patch::drain::{PatchToDrainCaller, PatchToDrainError, patch_to_drain};
use crate::patch::evict_later::{PatchToEvictLaterError, patch_to_evict_later};
use crate::pod_disruption_budget::{
    DecreasePodDisruptionBudgetError, decrease_pod_disruption_budget,
};
use crate::report::report;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::{ServiceRegistry, Stores};

pub fn start_evict_controller(
    api_resolver: &ApiResolver,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
    stores: &Stores,
    recorder: &Recorder,
    shutdown: &Shutdown,
) -> Result<()> {
    let api_resolver = api_resolver.clone();

    let context = Arc::new(EvictReconcilerContext {
        api_resolver: api_resolver.clone(),
        loadbalancing: loadbalancing.clone(),
        stores: stores.clone(),
        recorder: recorder.clone(),
    });

    let pods: Api<Pod> = api_resolver.all();
    let controller = Controller::new(
        pods,
        watcher::Config::default().labels(&format!(
            "{DRAINING_LABEL_KEY}={DRAINING_LABEL_VALUE__EVICTING}"
        )),
    )
    .graceful_shutdown_on(shutdown.wait_shutdown_triggered());

    let signal = service_registry.register("controller:evict");
    spawn_service(
        shutdown,
        span!(Level::INFO, "controller:evict"),
        async move {
            signal.ready();
            controller
                .run(reconcile, error_policy, context)
                .for_each(|result| async move {
                    log_reconcile_result(result);
                })
                .await
        },
    )?;

    Ok(())
}

struct EvictReconcilerContext {
    api_resolver: ApiResolver,
    loadbalancing: LoadBalancingConfig,
    stores: Stores,
    recorder: Recorder,
}

#[derive(Error, Debug)]
enum EvictReconcilerError {
    #[error(transparent)]
    PodDisruptionBudget(#[from] DecreasePodDisruptionBudgetError),
    #[error(transparent)]
    PatchToDrain(#[from] PatchToDrainError),
    #[error(transparent)]
    PatchToEvictLater(#[from] PatchToEvictLaterError),
}

const CONTROLLER_EXCLUSIVE_DURATION: Duration = Duration::from_secs(10);
const CONTROLLER_TIMEOUT_JITTER: Duration = Duration::from_secs(10);
const DEFAULT_ERROR_RECONCILE: Duration = Duration::from_secs(10);
const DEFAULT_TRANSIENT_ERROR_RECONCILE: Duration = Duration::from_secs(5);
const DEFAULT_RECONCILE_DURATION: Duration = Duration::from_secs(3600);

async fn reconcile(
    pod: Arc<Pod>,
    context: Arc<EvictReconcilerContext>,
) -> Result<Action, EvictReconcilerError> {
    if pod.metadata.deletion_timestamp.is_some() {
        return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
    }
    let Ok(Some(DrainingLabelValue::Evicting)) = get_pod_draining_label_value(&pod) else {
        return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
    };

    if let Ok(Some(evict_after)) = get_pod_evict_after(&pod) {
        let evict_after = if am_i_pod_drain_controller(&pod, &context.loadbalancing) {
            evict_after
        } else {
            // Let the original controller handle first.
            let controller_exclusive_until = evict_after + CONTROLLER_EXCLUSIVE_DURATION;
            let jitter = get_stable_jitter(
                &pod,
                &context.loadbalancing,
                Default::default()..CONTROLLER_TIMEOUT_JITTER,
            );
            controller_exclusive_until + jitter
        };

        // backoff eviction
        if let Ok(remaining) = (evict_after - Utc::now()).to_std() {
            return Ok(Action::requeue(remaining));
        }
    } else {
        // eviction label is missing or malformed. No load balancing.
        // multiple pods are going to race over PodDisruptionBudget and only one of them will win.
    };

    match decrease_pod_disruption_budget(&pod, &context.stores, &context.api_resolver).await {
        Ok(()) => {
            patch_to_drain(
                &pod,
                &context.api_resolver,
                &context.loadbalancing,
                PatchToDrainCaller::Controller,
            )
            .await?;

            Ok(Action::requeue(DEFAULT_RECONCILE_DURATION))
        }
        Err(DecreasePodDisruptionBudgetError::TooManyRequests(err)) => {
            let now = Utc::now();
            let duration = Duration::from_secs(err.retry_after_seconds.max(1) as _);
            let evict_after = now + duration;

            debug!(?err);

            patch_to_evict_later(
                &pod,
                evict_after,
                &context.api_resolver,
                &context.loadbalancing,
            )
            .await?;

            report(
                &context.recorder,
                &pod.object_ref(&()),
                EventType::Normal,
                "WaitForPodDisruptionBudget",
                "PodDisruptionBudget",
                err.to_string(),
            )
            .await;

            Ok(Action::requeue(duration))
        }
        Err(err) => Err(err.into()),
    }
}

fn error_policy(
    _pod: Arc<Pod>,
    err: &EvictReconcilerError,
    _context: Arc<EvictReconcilerContext>,
) -> Action {
    match err {
        EvictReconcilerError::PodDisruptionBudget(err) => match err {
            DecreasePodDisruptionBudgetError::TooManyRequests(_) => {
                // handled by reconcile
            }
            DecreasePodDisruptionBudgetError::Kube(err) => {
                if is_409_conflict_error(err) {
                    return Action::requeue(CONTROLLER_EXCLUSIVE_DURATION);
                }

                if is_transient_error(err) {
                    return Action::requeue(DEFAULT_TRANSIENT_ERROR_RECONCILE);
                }
            }
            _ => {}
        },
        EvictReconcilerError::PatchToDrain(_) => {
            // patcher tried its best to recover, or a bug. let's requeue.
        }
        EvictReconcilerError::PatchToEvictLater(_) => {
            // patcher tried its best to recover, or a bug. let's requeue.
        }
    }

    Action::requeue(DEFAULT_ERROR_RECONCILE)
}

fn log_reconcile_result(
    result: Result<
        (ObjectRef<Pod>, Action),
        controller::Error<EvictReconcilerError, watcher::Error>,
    >,
) {
    let span = span!(Level::INFO, "log");
    let _entered = span.enter();

    log_reconcile_result_common(result, |reconciler_err, object_ref| {
        let span = span!(Level::ERROR, "error", %object_ref);
        let _entered = span.enter();

        match reconciler_err {
            EvictReconcilerError::PodDisruptionBudget(err) => match err {
                DecreasePodDisruptionBudgetError::TooManyRequests(_) => {
                    // handled by reconcile
                }
                DecreasePodDisruptionBudgetError::Kube(err) => {
                    log_reconcile_kube_err_common(err);
                }
                DecreasePodDisruptionBudgetError::Bug(err) => {
                    error!(%err, "bug on reconcile")
                }
                DecreasePodDisruptionBudgetError::NotMyFault(err) => {
                    error!(%err, "there's a problem during reconcile")
                }
            },
            EvictReconcilerError::PatchToDrain(err) => {
                error!(%err, "patch to drain failed");
            }
            EvictReconcilerError::PatchToEvictLater(err) => {
                error!(%err, "patch to evict later failed");
            }
        };
    });
}
