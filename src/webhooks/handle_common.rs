use std::default::Default;
use std::error::Error;
use std::fmt::Debug;

use axum::Json;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use eyre::Result;
use futures::future::BoxFuture;
use k8s_openapi::serde::Serialize;
use kube::Resource;
use kube::client::Status;
use kube::core::DynamicObject;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use kube::core::response::{StatusCause, StatusDetails, StatusSummary};
use kube::runtime::reflector::ObjectRef;
use tracing::{Level, span, trace};

use crate::instrumented;
use crate::report::{debug_report_for_ref, err_report_for_ref};
use crate::utils::get_object_ref_from_name;
use crate::webhooks::AppState;
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
    Deny(String),
    Patch(Box<AdmissionResponse>),
}

pub async fn handle_common<K, F>(
    handle: F,
    state: AppState,
    review: AdmissionReview<K>,
) -> HandlerResult<AdmissionReview<DynamicObject>>
where
    K: Resource + Debug + Serialize,
    K::DynamicType: Default + Clone,
    for<'a> F:
        FnOnce(&'a AppState, &'a AdmissionRequest<K>) -> BoxFuture<'a, Result<InterceptResult>>,
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
        get_object_ref_from_name(&request.name, request.namespace.as_deref());

    instrumented!(
        span!(Level::INFO, "webhook", %object_ref, operation = ?request.operation, uid=request.uid),
        async move {
            trace!(user_info=?request.user_info);

            if is_my_serviceaccount(&state.downward_api, &request.user_info) {
                debug_report_for_ref(
                    &state.recorder,
                    &object_ref,
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
                    &state.recorder,
                    &object_ref,
                    "Allow",
                    "DryRun",
                    "dry run".to_owned(),
                )
                .await;

                return HandlerResult::Value(AdmissionResponse::from(request).into_review());
            }

            let result = handle(&state, request).await;

            match result {
                Ok(InterceptResult::Allow) => {
                    HandlerResult::Value(AdmissionResponse::from(request).into_review())
                }
                Ok(InterceptResult::Deny(reason)) => HandlerResult::Value(
                    AdmissionResponse::from(request).deny(reason).into_review(),
                ),
                Ok(InterceptResult::Patch(response)) => {
                    HandlerResult::Value(response.into_review())
                }
                Err(err) => {
                    let status = handle_error(err.as_ref(), &state, &object_ref).await;
                    HandlerResult::Status(status)
                }
            }
        }
    )
    .await
}

async fn handle_error<K>(
    err: &(dyn Error + Send + Sync),
    state: &AppState,
    object_ref: &ObjectRef<K>,
) -> Status
where
    K: Resource,
    K::DynamicType: Default + Clone,
{
    err_report_for_ref(
        &state.recorder,
        object_ref,
        "Error",
        "Error",
        format!("{err:#}"),
    )
    .await;

    let mut causes = vec![];
    let mut chain: Option<&dyn Error> = Some(err);
    while let Some(err) = chain {
        causes.push(StatusCause {
            message: err.to_string(),
            reason: String::new(),
            field: String::new(),
        });

        chain = err.source();
    }

    let status = Status {
        code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        status: Some(StatusSummary::Failure),
        reason: String::from("InternalError"),
        message: format!("Internal server error occurred: {}", err),
        details: Some(StatusDetails {
            name: object_ref.name.clone(),
            group: K::group(&Default::default()).into_owned(),
            kind: K::kind(&Default::default()).into_owned(),
            uid: String::new(),
            causes,
            retry_after_seconds: 0,
        }),
    };

    status
}
