mod config;
mod handle_delete;
mod handle_eviction;
mod patch;
mod reactive_rustls_config;
mod report;
mod try_bind;

use std::fmt::Debug;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{extract::State, routing::post, Json, Router};
use eyre::Result;
use k8s_openapi::api::authentication::v1::UserInfo;
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::api::{core::v1::Pod, policy::v1::Eviction};
use k8s_openapi::serde::Serialize;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::core::DynamicObject;
use kube::runtime::events::{EventType, Reporter};
use kube::runtime::reflector::ObjectRef;
use kube::Resource;
use serde_json::{json, Value};
use tracing::{info, span, trace, Level};

use crate::api_resolver::ApiResolver;
use crate::config::Config;
use crate::consts::CONTROLLER_NAME;
use crate::reflector::Stores;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::utils::get_object_ref_from_name;
pub use crate::webhooks::config::WebhookConfig;
use crate::webhooks::handle_delete::delete_handler;
use crate::webhooks::handle_eviction::eviction_handler;
pub use crate::webhooks::patch::patch_pod_isolate;
use crate::webhooks::reactive_rustls_config::build_reactive_rustls_config;
use crate::webhooks::report::{debug_report_for_ref, report};
use crate::webhooks::try_bind::try_bind;
use crate::{instrumented, LoadBalancingConfig, ServiceRegistry};

/// Start an admission webhook that intercepts pod deletion, pod eviction requests.
pub async fn start_webhook(
    api_resolver: &ApiResolver,
    config: Config,
    webhook_config: WebhookConfig,
    stores: Stores,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
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
            event_reporter: Reporter {
                controller: String::from(CONTROLLER_NAME),
                instance: hostname::get()
                    .ok()
                    .and_then(|n| n.to_str().map(String::from)),
            },
        });

    let rustls_config = build_reactive_rustls_config(&webhook_config.cert, shutdown).await?;

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

async fn mutate_handler(
    State(state): State<AppState>,
    Json(review): Json<AdmissionReview<Eviction>>,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>> {
    handle_common(eviction_handler, &state, &review).await
}

async fn validate_handler(
    State(state): State<AppState>,
    Json(review): Json<AdmissionReview<Pod>>,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>> {
    handle_common(delete_handler, &state, &review).await
}

#[derive(Debug, Clone, Copy)]
enum ValueOrStatusCode<T> {
    Value(T),
    StatusCode(StatusCode),
}

impl<T> IntoResponse for ValueOrStatusCode<T>
where
    Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            ValueOrStatusCode::Value(value) => Json(value).into_response(),
            ValueOrStatusCode::StatusCode(status_code) => status_code.into_response(),
        }
    }
}

enum InterceptResult {
    Allow,
    Delay(Duration),
    Patch(Box<AdmissionResponse>),
}

async fn handle_common<'a, K, Fut>(
    handle: impl FnOnce(&'a AppState, &'a AdmissionRequest<K>, &'a UserInfo) -> Fut,
    state: &'a AppState,
    review: &'a AdmissionReview<K>,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>>
where
    K: Resource + Debug + Serialize,
    K::DynamicType: Default,
    Fut: Future<Output = Result<InterceptResult>>,
{
    let request = match &review.request {
        Some(request) => request,
        None => return ValueOrStatusCode::StatusCode(StatusCode::BAD_REQUEST),
    };

    let object_ref: ObjectRef<K> =
        get_object_ref_from_name(&request.name, request.namespace.as_ref());
    let request_id: u32 = rand::random();
    instrumented!(
        span!(Level::ERROR, "admission", %object_ref, operation = ?request.operation, request_id),
        async move {
            trace!(user_info=?request.user_info);

            if request.dry_run {
                debug_report_for_ref(
                    state,
                    ObjectReference::from(object_ref),
                    "Allow",
                    "DryRun",
                    format!(
                        "operation={:?}, kind={}",
                        request.operation,
                        <K as Resource>::kind(&Default::default())
                    ),
                )
                .await;

                return ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review());
            }

            let result = handle(state, request, &request.user_info).await;

            match result {
                Ok(InterceptResult::Allow) => {
                    ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Delay(duration)) => {
                    tokio::time::sleep(duration).await;
                    ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Patch(response)) => {
                    ValueOrStatusCode::Value(response.into_review())
                }
                Err(err) => {
                    report(
                        state,
                        ObjectReference::from(object_ref),
                        EventType::Warning,
                        "Error",
                        "Error",
                        format!("{err:#}"),
                    )
                    .await;
                    ValueOrStatusCode::StatusCode(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
    )
}
