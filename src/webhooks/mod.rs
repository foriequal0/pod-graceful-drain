mod config;
mod handle_common;
mod handle_delete;
mod handle_eviction;
mod reactive_rustls_config;
mod report;
mod self_recognize;
mod try_bind;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router, extract::State, routing::post};
use eyre::Result;
use k8s_openapi::api::{core::v1::Pod, policy::v1::Eviction};
use kube::core::DynamicObject;
use kube::core::admission::AdmissionReview;
use kube::runtime::events::Reporter;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tracing::info;

use crate::api_resolver::ApiResolver;
use crate::config::Config;
use crate::consts::CONTROLLER_NAME;
use crate::downward_api::DownwardAPI;
use crate::reflector::Stores;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
pub use crate::webhooks::config::WebhookConfig;
use crate::webhooks::handle_common::{ValueOrStatusCode, handle_common};
use crate::webhooks::handle_delete::delete_handler;
use crate::webhooks::handle_eviction::eviction_handler;
use crate::webhooks::reactive_rustls_config::build_reactive_rustls_config;
use crate::webhooks::report::debug_report_for_ref;
use crate::webhooks::try_bind::try_bind;
use crate::{LoadBalancingConfig, ServiceRegistry};

/// Start an admission webhook that intercepts pod deletion, pod eviction requests.
#[allow(clippy::too_many_arguments)]
pub async fn start_webhook(
    api_resolver: &ApiResolver,
    config: Config,
    webhook_config: WebhookConfig,
    stores: Stores,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
    downward_api: &DownwardAPI,
    shutdown: &Shutdown,
) -> Result<SocketAddr> {
    let app = Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/merics", get(metrics_handler))
        .route("/webhook/mutate", post(mutate_handler))
        .route("/webhook/validate", post(validate_handler))
        .with_state(AppState {
            api_resolver: api_resolver.clone(),
            config: config.clone(),
            stores,
            service_registry: service_registry.clone(),
            loadbalancing: loadbalancing.clone(),
            downward_api: downward_api.clone(),
            event_reporter: Reporter {
                controller: String::from(CONTROLLER_NAME),
                instance: downward_api.pod_name.clone(),
            },
        });

    let rustls_config =
        build_reactive_rustls_config(&webhook_config.cert, api_resolver, shutdown).await?;

    let addr_incoming = try_bind(&webhook_config.bind).await?;
    let local_addr = addr_incoming.local_addr()?;
    info!("listening {}", local_addr);

    let handle = axum_server::Handle::new();
    let server = {
        axum_server::bind_rustls(local_addr, rustls_config)
            .handle(handle.clone())
            .serve(app.into_make_service())
    };

    tokio::spawn({
        let shutdown = shutdown.clone();
        let handle = handle.clone();
        let draining_graceful_period = config.delete_after;

        async move {
            shutdown.wait_drain_triggered().await;
            handle.graceful_shutdown(Some(draining_graceful_period));
            shutdown.wait_drain_complete().await;
        }
    });

    let signal = service_registry.register("webhook");
    spawn_service(shutdown, "webhook", {
        let shutdown = shutdown.clone();

        async move {
            let _drain_token = shutdown.delay_drain_token();
            signal.ready();
            server.await.unwrap();
        }
    })?;

    Ok(local_addr)
}

#[derive(Clone)]
struct AppState {
    api_resolver: ApiResolver,
    config: Config,
    stores: Stores,
    service_registry: ServiceRegistry,
    event_reporter: Reporter,
    loadbalancing: LoadBalancingConfig,
    downward_api: DownwardAPI,
}

async fn healthz_handler(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let not_ready = state.service_registry.get_not_ready_services();
    let status_code = if not_ready.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(json!({ "not_ready": not_ready })))
}

async fn metrics_handler(State(_state): State<AppState>) -> StatusCode {
    // TODO
    StatusCode::OK
}

#[derive(Deserialize)]
struct QueryParams {
    #[serde(deserialize_with = "parse_duration")]
    timeout: Option<Duration>,
}

fn parse_duration<'de, D>(de: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    let option = Option::<String>::deserialize(de)?;
    let Some(option) = option else {
        return Ok(None);
    };

    let duration = humantime::parse_duration(&option).map_err(serde::de::Error::custom)?;
    Ok(Some(duration))
}

async fn mutate_handler(
    State(state): State<AppState>,
    Query(QueryParams { timeout }): Query<QueryParams>,
    Json(review): Json<AdmissionReview<Eviction>>,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>> {
    let timeout = timeout.unwrap_or(Duration::from_secs(10));

    handle_common(eviction_handler, &state, &review, timeout).await
}

async fn validate_handler(
    State(state): State<AppState>,
    Query(QueryParams { timeout }): Query<QueryParams>,
    Json(review): Json<AdmissionReview<Pod>>,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>> {
    let timeout = timeout.unwrap_or(Duration::from_secs(10));

    handle_common(delete_handler, &state, &review, timeout).await
}
