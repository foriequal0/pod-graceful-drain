use kube::Error;
use kube::error::ErrorResponse;

const STATUS_CODE_404_NOT_FOUND: u16 = 404;
const STATUS_CODE_408_TIMEOUT: u16 = 408;
const STATUS_CODE_409_CONFLICT: u16 = 409;
const STATUS_CODE_410_GONE: u16 = 410;
const STATUS_CODE_422_UNPROCESSABLE_ENTITY: u16 = 422;
const STATUS_CODE_429_TOO_MANY_REQUESTS: u16 = 429;
const STATUS_CODE_500_INTERNAL_SERVER_ERROR: u16 = 500;
const STATUS_CODE_502_BAD_GATEWAY: u16 = 502;
const STATUS_CODE_503_SERVICE_UNAVAILABLE: u16 = 503;
const STATUS_CODE_504_GATEWAY_TIMEOUT: u16 = 504;

pub fn is_404_not_found_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Api(ErrorResponse {
            code: STATUS_CODE_404_NOT_FOUND,
            ..
        })
    )
}

pub fn is_409_conflict_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Api(ErrorResponse {
            code: STATUS_CODE_409_CONFLICT,
            ..
        })
    )
}

/// usually due to a resourceVersion that is too old for LIST or WATCH operations
pub fn is_410_expired_error(err: &Error) -> bool {
    matches!(err, Error::Api(err) if is_410_expired_error_response(err))
}

pub fn is_410_expired_error_response(err: &ErrorResponse) -> bool {
    matches!(
        err,
        ErrorResponse {
            code: STATUS_CODE_410_GONE,
            .. // reason: "Expired". It seems that reason is changing from "Gone"
        }
    )
}

pub fn is_422_invalid_for_json_patch_test_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Api(ErrorResponse {
            code: STATUS_CODE_422_UNPROCESSABLE_ENTITY,
            message,
            ..
        })
        // if there's a bug in the patch, different messages are returned
        if message == "the server rejected our request due to an error in our request"
    )
}

pub fn is_transient_error(err: &Error) -> bool {
    match err {
        Error::Api(ErrorResponse {
            code:
                STATUS_CODE_408_TIMEOUT
                | STATUS_CODE_429_TOO_MANY_REQUESTS
                | STATUS_CODE_502_BAD_GATEWAY
                | STATUS_CODE_503_SERVICE_UNAVAILABLE
                | STATUS_CODE_504_GATEWAY_TIMEOUT,
            ..
        }) => true,

        Error::Api(ErrorResponse {
            code: STATUS_CODE_500_INTERNAL_SERVER_ERROR,
            reason,
            ..
        }) if reason == "ServerTimeout" => true,

        // TODO: Handle more transient err
        _ => false,
    }
}
