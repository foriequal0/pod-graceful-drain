use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use eyre::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::api::Preconditions;
use kube::runtime::controller::Action;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{Controller, controller, watcher};
use kube::{Api, ResourceExt};
use thiserror::Error;
use tracing::{Level, debug, error, info, span, warn};

use crate::api_resolver::ApiResolver;
use crate::controllers::utils::{
    get_stable_jitter, log_reconcile_kube_err_common, log_reconcile_result_common,
};
use crate::error_codes::{is_404_not_found_error, is_409_conflict_error, is_transient_error};
use crate::labels_and_annotations::{
    DRAINING_LABEL_KEY, DRAINING_LABEL_VALUE__DRAINING, DrainingLabelValue,
    am_i_pod_drain_controller, get_pod_delete_options, get_pod_drain_timestamp,
    get_pod_draining_label_value,
};
use crate::loadbalancing::LoadBalancingConfig;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::utils::to_delete_params;
use crate::{Config, ServiceRegistry};

pub fn start_drain_controller(
    api_resolver: &ApiResolver,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
    config: &Config,
    shutdown: &Shutdown,
) -> Result<()> {
    let api_resolver = api_resolver.clone();

    let context = Arc::new(DrainReconcilerContext {
        api_resolver: api_resolver.clone(),
        loadbalancing: loadbalancing.clone(),
        config: config.clone(),
    });

    let pods: Api<Pod> = api_resolver.all();
    let controller = Controller::new(
        pods,
        watcher::Config::default().labels(&format!(
            "{DRAINING_LABEL_KEY}={DRAINING_LABEL_VALUE__DRAINING}"
        )),
    )
    .graceful_shutdown_on(shutdown.wait_shutdown_triggered());

    let signal = service_registry.register("controller:drain");
    spawn_service(
        shutdown,
        span!(Level::INFO, "controller:drain"),
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

struct DrainReconcilerContext {
    api_resolver: ApiResolver,
    loadbalancing: LoadBalancingConfig,
    config: Config,
}

#[derive(Error, Debug)]
enum DrainReconcilerError {
    #[error("kube error")]
    KubeError(#[from] kube::Error),
}

const CONTROLLER_EXCLUSIVE_DURATION: Duration = Duration::from_secs(10);
const CONTROLLER_TIMEOUT_JITTER: Duration = Duration::from_secs(10);
const DEFAULT_ERROR_RECONCILE: Duration = Duration::from_secs(10);
const DEFAULT_TRANSIENT_ERROR_RECONCILE: Duration = Duration::from_secs(5);
const DEFAULT_RECONCILE_DURATION: Duration = Duration::from_secs(3600);

async fn reconcile(
    pod: Arc<Pod>,
    context: Arc<DrainReconcilerContext>,
) -> Result<Action, DrainReconcilerError> {
    if pod.metadata.deletion_timestamp.is_some() {
        return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
    }

    let Ok(Some(DrainingLabelValue::Draining)) = get_pod_draining_label_value(&pod) else {
        return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
    };
    let Ok(Some(drain_timestamp)) = get_pod_drain_timestamp(&pod) else {
        return Ok(Action::requeue(DEFAULT_RECONCILE_DURATION));
    };

    let drain_until = drain_timestamp + context.config.delete_after;
    if am_i_pod_drain_controller(&pod, &context.loadbalancing) {
        let remaining = drain_until - Utc::now();
        if let Ok(remaining) = remaining.to_std() {
            return Ok(Action::requeue(remaining));
        }
    } else {
        // Let the original controller handle first.
        let controller_exclusive_until = drain_until + CONTROLLER_EXCLUSIVE_DURATION;
        let jitter = get_stable_jitter(
            &pod,
            &context.loadbalancing,
            Default::default()..CONTROLLER_TIMEOUT_JITTER,
        );
        let jittered = controller_exclusive_until + jitter;
        let remaining = jittered - Utc::now();
        if let Ok(remaining) = remaining.to_std() {
            return Ok(Action::requeue(remaining));
        }
    };

    let delete_options = match get_pod_delete_options(&pod) {
        Ok(Some(delete_options)) => delete_options,
        Ok(None) => DeleteOptions::default(),
        Err(err) => {
            warn!(
                "Invalid delete options, recover with default option: '{}'",
                err
            );
            DeleteOptions::default()
        }
    };

    delete_pod(&context.api_resolver, &pod, &delete_options).await?;

    Ok(Action::requeue(DEFAULT_RECONCILE_DURATION))
}

fn error_policy(
    _pod: Arc<Pod>,
    err: &DrainReconcilerError,
    _context: Arc<DrainReconcilerContext>,
) -> Action {
    match err {
        DrainReconcilerError::KubeError(err) => {
            // 404 is handled by `delete_pod`
            if is_409_conflict_error(err) {
                return Action::requeue(CONTROLLER_EXCLUSIVE_DURATION);
            }

            if is_transient_error(err) {
                return Action::requeue(DEFAULT_TRANSIENT_ERROR_RECONCILE);
            }
        }
    }

    Action::requeue(DEFAULT_ERROR_RECONCILE)
}

fn log_reconcile_result(
    result: Result<
        (ObjectRef<Pod>, Action),
        controller::Error<DrainReconcilerError, watcher::Error>,
    >,
) {
    let span = span!(Level::INFO, "log");
    let _entered = span.enter();

    log_reconcile_result_common(result, |reconciler_err, object_ref| {
        let span = span!(Level::ERROR, "error", %object_ref);
        let _entered = span.enter();

        match reconciler_err {
            DrainReconcilerError::KubeError(err) => {
                log_reconcile_kube_err_common(err);
            }
        };
    });
}

async fn delete_pod(
    api_resolver: &ApiResolver,
    pod: &Pod,
    delete_options: &DeleteOptions,
) -> kube::Result<()> {
    let api = api_resolver.api_for(pod);
    let name = pod.name_any();

    let mut delete_params = to_delete_params(delete_options);
    delete_params.preconditions = Some(Preconditions {
        uid: pod.metadata.uid.clone(),
        resource_version: pod.metadata.resource_version.clone(),
    });

    debug!("deleting pod");
    let result = api.delete(&name, &delete_params).await;
    match result {
        Ok(_) => {
            info!("pod is deleted");
            Ok(())
        }
        Err(err) if is_404_not_found_error(&err) => {
            debug!("pod is gone anyway"); // This is what we desired.
            Ok(())
        }
        Err(err) => Err(err),
    }
}
