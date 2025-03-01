use std::fmt::Debug;
use std::future::Future;
use std::time::{Duration, Instant};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use eyre::Result;
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::serde::Serialize;
use kube::Resource;
use kube::core::DynamicObject;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::runtime::reflector::ObjectRef;
use tracing::{Level, span, trace};

use crate::instrumented;
use crate::utils::get_object_ref_from_name;
use crate::webhooks::AppState;
use crate::webhooks::report::{debug_report_for_ref, warn_report_for_ref};
use crate::webhooks::self_recognize::is_my_serviceaccount;

#[derive(Debug, Clone, Copy)]
pub enum ValueOrStatusCode<T> {
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

pub enum InterceptResult {
    Allow,
    Delay(Duration),
    Patch(Box<AdmissionResponse>),
}

pub async fn handle_common<'a, K, Fut>(
    handle: impl FnOnce(&'a AppState, &'a AdmissionRequest<K>) -> Fut,
    state: &'a AppState,
    review: &'a AdmissionReview<K>,
    timeout: Duration,
) -> ValueOrStatusCode<AdmissionReview<DynamicObject>>
where
    K: Resource + Debug + Serialize,
    K::DynamicType: Default,
    Fut: Future<Output = Result<InterceptResult>>,
{
    let start = Instant::now();

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

            if is_my_serviceaccount(&state.downward_api, &request.user_info) {
                debug_report_for_ref(
                    state,
                    ObjectReference::from(object_ref),
                    "Allow",
                    "Reentry-Controller",
                    format!(
                        "operation={:?}, kind={}",
                        request.operation,
                        <K as Resource>::kind(&Default::default())
                    ),
                )
                .await;

                return ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review());
            }

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

            let result = handle(state, request).await;

            match result {
                Ok(InterceptResult::Allow) => {
                    ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Delay(duration)) => {
                    const DEADLINE_OFFSET: Duration = Duration::from_secs(2);

                    let now = Instant::now();
                    let deadline = start + timeout - DEADLINE_OFFSET;
                    let to_sleep = deadline.min(now + duration) - now;

                    tokio::time::sleep(to_sleep).await;
                    ValueOrStatusCode::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Patch(response)) => {
                    ValueOrStatusCode::Value(response.into_review())
                }
                Err(err) => {
                    warn_report_for_ref(
                        state,
                        ObjectReference::from(object_ref),
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
