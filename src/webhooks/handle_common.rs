use std::fmt::Debug;
use std::future::Future;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use eyre::Result;
use k8s_openapi::api::authentication::v1::UserInfo;
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::serde::Serialize;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::core::DynamicObject;
use kube::runtime::reflector::ObjectRef;
use kube::Resource;
use tracing::{span, trace, Level};

use crate::impersonate::is_impersonated_self;
use crate::instrumented;
use crate::utils::get_object_ref_from_name;
use crate::webhooks::report::{debug_report_for_ref, warn_report_for_ref};
use crate::webhooks::self_recognize::is_my_serviceaccount;
use crate::webhooks::AppState;

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

            if is_impersonated_self(&request.user_info) {
                // impersonated reentry is always a dry-run
                if !request.dry_run {
                    return ValueOrStatusCode::StatusCode(StatusCode::BAD_REQUEST);
                }

                debug_report_for_ref(
                    state,
                    ObjectReference::from(object_ref),
                    "Allow",
                    "Reentry-Impersonated",
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
