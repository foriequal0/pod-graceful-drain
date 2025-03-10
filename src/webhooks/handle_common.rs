use std::fmt::Debug;
use std::future::Future;
use std::time::Duration;

use axum::Json;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use eyre::Result;
use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::serde::Serialize;
use kube::Resource;
use kube::client::Status;
use kube::core::DynamicObject;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::core::response::{StatusCause, StatusDetails, StatusSummary};
use kube::runtime::reflector::ObjectRef;
use tracing::{Level, span, trace};

use crate::instrumented;
use crate::utils::get_object_ref_from_name;
use crate::webhooks::AppState;
use crate::webhooks::report::{debug_report_for_ref, warn_report_for_ref};
use crate::webhooks::self_recognize::is_my_serviceaccount;

#[derive(Debug)]
pub enum HandlerResult<T> {
    Value(T),
    Status(Status),
}

impl<T> IntoResponse for HandlerResult<T>
where
    Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            HandlerResult::Value(value) => Json(value).into_response(),
            HandlerResult::Status(status) => {
                let mut resp = Json(&status).into_response();

                *resp.status_mut() =
                    StatusCode::try_from(status.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

                if let Some(details) = &status.details {
                    if details.retry_after_seconds > 0 {
                        let name = HeaderName::from_static("retry-after");
                        let value = HeaderValue::from(details.retry_after_seconds);
                        resp.headers_mut().insert(name, value);
                    }
                }

                resp
            }
        }
    }
}

pub enum InterceptResult {
    Allow,
    Patch(Box<AdmissionResponse>),
    TooManyRequests {
        message: String,
        causes: Vec<StatusCause>,
        retry_after: u32,
    },
}

pub async fn handle_common<'a, K, Fut>(
    handle: impl FnOnce(&'a AppState, &'a AdmissionRequest<K>, Duration) -> Fut,
    state: &'a AppState,
    review: &'a AdmissionReview<K>,
    timeout: Duration,
) -> HandlerResult<AdmissionReview<DynamicObject>>
where
    K: Resource + Debug + Serialize,
    K::DynamicType: Default,
    Fut: Future<Output = Result<InterceptResult>>,
{
    let request = match &review.request {
        Some(request) => request,
        None => {
            return HandlerResult::Status(
                Status::failure("AdmissionReview.request is missing", "BadRequest")
                    .with_code(StatusCode::BAD_REQUEST.as_u16()),
            );
        }
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

                return HandlerResult::Value(AdmissionResponse::from(request).into_review());
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

                return HandlerResult::Value(AdmissionResponse::from(request).into_review());
            }

            let result = handle(state, request, timeout).await;

            match result {
                Ok(InterceptResult::Allow) => {
                    HandlerResult::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Patch(response)) => {
                    HandlerResult::Value(response.into_review())
                }
                Ok(InterceptResult::TooManyRequests {
                    message,
                    causes,
                    retry_after,
                }) => {
                    let (group, kind) = if let Some(gvk) = request.request_kind.clone() {
                        (gvk.group, gvk.kind)
                    } else {
                        (String::new(), String::new())
                    };

                    let status = Status {
                        code: StatusCode::TOO_MANY_REQUESTS.as_u16(),
                        status: Some(StatusSummary::Failure),
                        reason: String::from("TooManyRequests"),
                        message,
                        details: Some(StatusDetails {
                            name: request.name.clone(),
                            group,
                            kind,
                            uid: request.uid.clone(),
                            causes,
                            retry_after_seconds: retry_after,
                        }),
                    };

                    HandlerResult::Status(status)
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

                    let (group, kind) = if let Some(gvk) = request.request_kind.clone() {
                        (gvk.group, gvk.kind)
                    } else {
                        (String::new(), String::new())
                    };

                    let mut causes = vec![];
                    for chain in err.chain() {
                        causes.push(StatusCause {
                            message: chain.to_string(),
                            reason: String::new(),
                            field: String::new(),
                        });
                    }

                    let status = Status {
                        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                        status: Some(StatusSummary::Failure),
                        reason: String::from("InternalError"),
                        message: format!("Internal server error occurred: {}", err),
                        details: Some(StatusDetails {
                            name: request.name.clone(),
                            group,
                            kind,
                            uid: request.uid.clone(),
                            causes,
                            retry_after_seconds: 0,
                        }),
                    };

                    HandlerResult::Status(status)
                }
            }
        }
    )
    .await
}
