use std::str::FromStr;

use axum::http::{HeaderName, HeaderValue, Request};
use eyre::{Context, Result};
use k8s_openapi::api::authentication::v1::UserInfo;
use percent_encoding::{AsciiSet, utf8_percent_encode};

const IMPERSONATED_SELF_USER_EXTRA: &str = "pod-graceful-drain/impersonated";

pub fn impersonate<T>(req: &mut Request<T>, user_info: &UserInfo) -> Result<()> {
    let headers = req.headers_mut();
    if let Some(uid) = &user_info.uid {
        let header_value = to_header_value(uid).context("Impersonate-Uid")?;
        headers.append("impersonate-uid", header_value);
    }

    if let Some(user) = &user_info.username {
        let header_value = to_header_value(user).context("Impersonate-User")?;
        headers.append("impersonate-user", header_value);
    }

    if let Some(groups) = &user_info.groups {
        for group in groups {
            let header_value = to_header_value(group).context("Impersonate-Group")?;
            headers.append("impersonate-group", header_value);
        }
    }

    if let Some(extra) = &user_info.extra {
        for (key, values) in extra {
            let header_name = to_header_name("impersonate-extra-", key)?;

            for value in values {
                let header_value =
                    to_header_value(value).with_context(|| header_name.to_string())?;
                headers.append(header_name.clone(), header_value);
            }
        }
    }

    {
        let header_name = to_header_name("impersonate-extra-", IMPERSONATED_SELF_USER_EXTRA)?;
        let header_value = HeaderValue::from_static("1");
        headers.append(header_name, header_value);
    }

    Ok(())
}

pub fn is_impersonated_self(user_info: &UserInfo) -> bool {
    if let Some(extra) = &user_info.extra {
        return extra.contains_key(IMPERSONATED_SELF_USER_EXTRA);
    }

    false
}

// https://kubernetes.io/docs/reference/access-authn-authz/authentication/#user-impersonation
// https://datatracker.ietf.org/doc/html/rfc7230#section-3.2.6
const ILLEGAL_IN_HEADER_NAME: &AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'(')
    .add(b')')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'{')
    .add(b'}');

fn to_header_name(prefix: &str, input: &str) -> Result<HeaderName> {
    let encoded = utf8_percent_encode(input, ILLEGAL_IN_HEADER_NAME);
    let value = format!("{prefix}{encoded}");
    let header_name = HeaderName::from_str(&value).context(value)?;
    Ok(header_name)
}

const ILLEGAL_IN_HEADER_VALUES: &AsciiSet = &percent_encoding::CONTROLS
    // Technically followings are not illegal unless it is leading or trailing
    .add(b' ')
    .add(b'\t');

fn to_header_value(input: &str) -> Result<HeaderValue> {
    let value = utf8_percent_encode(input, ILLEGAL_IN_HEADER_VALUES).to_string();
    let header_value = HeaderValue::from_str(&value)?;
    Ok(header_value)
}
