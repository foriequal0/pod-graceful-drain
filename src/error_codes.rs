use kube::error::ErrorResponse;
use kube::Error;

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

pub fn is_410_gone_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Api(ErrorResponse {
            code: STATUS_CODE_410_GONE,
            ..
        })
    )
}

pub fn is_generic_server_response_422_invalid_for_json_patch_error(err: &Error) -> bool {
    matches!(
        err,
        Error::Api(ErrorResponse {
            code,
            reason,
            ..
        }) if *code == STATUS_CODE_422_UNPROCESSABLE_ENTITY && reason == "Invalid"
    )
}

pub fn is_transient_error(err: &Error) -> bool {
    match err {
        Error::Api(ErrorResponse {
            code:
                STATUS_CODE_408_TIMEOUT
                | STATUS_CODE_429_TOO_MANY_REQUESTS // related to PodDisruptionBudget 
                | STATUS_CODE_500_INTERNAL_SERVER_ERROR
                | STATUS_CODE_502_BAD_GATEWAY
                | STATUS_CODE_503_SERVICE_UNAVAILABLE
                | STATUS_CODE_504_GATEWAY_TIMEOUT..,
            ..
        }) => true,

        // TODO: Handle more transient err
        _ => false,
    }
}
