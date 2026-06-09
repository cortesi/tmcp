//! Request validation helpers shared by the HTTP server handlers.

use std::result::Result as StdResult;

use axum::{
    body::{self, Body},
    http::{HeaderMap, StatusCode, header, uri::Authority},
    response::{IntoResponse, Response},
};
use url::Url;

use crate::schema::JSONRPCMessage;

/// Maximum accepted HTTP request body size.
const MAX_HTTP_BODY_SIZE: usize = 2 * 1024 * 1024;

/// Result type used for origin validation checks.
pub(super) type OriginResult = StdResult<(), Box<Response>>;

/// Validate the Origin header against the request Host when present.
pub(super) fn validate_origin(headers: &HeaderMap) -> OriginResult {
    if headers.get(header::ORIGIN).is_none() {
        return Ok(());
    }

    let origin = headers.get(header::ORIGIN).unwrap();

    let origin_str = origin
        .to_str()
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if origin_str == "null" {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    let origin_url = Url::parse(origin_str)
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if !matches!(origin_url.scheme(), "http" | "https") {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    let host_header = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    let authority = host_header
        .parse::<Authority>()
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;

    let origin_host = origin_url
        .host_str()
        .ok_or_else(|| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if !origin_host.eq_ignore_ascii_case(authority.host()) {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    if let Some(expected_port) = authority.port_u16() {
        if origin_url.port_or_known_default() != Some(expected_port) {
            return Err(Box::new(
                (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
            ));
        }
    } else if origin_url.port().is_some() {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    Ok(())
}

/// Ensure the request Content-Type is JSON.
pub(super) fn validate_json_content_type(headers: &HeaderMap) -> StdResult<(), Box<Response>> {
    let Some(content_type) = headers.get(header::CONTENT_TYPE) else {
        return Err(Box::new(
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            )
                .into_response(),
        ));
    };
    let Ok(content_type) = content_type.to_str() else {
        return Err(Box::new(
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "Invalid Content-Type").into_response(),
        ));
    };
    if content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        Ok(())
    } else {
        Err(Box::new(
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            )
                .into_response(),
        ))
    }
}

/// Read the full HTTP request body with an explicit size limit.
pub(super) async fn read_json_body(body: Body) -> StdResult<bytes::Bytes, Box<Response>> {
    body::to_bytes(body, MAX_HTTP_BODY_SIZE)
        .await
        .map_err(|error| {
            let status = if error.to_string().contains("length limit exceeded") {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            };
            Box::new((status, format!("Failed to read request body: {error}")).into_response())
        })
}

/// Parse a JSON-RPC message body into a typed value.
pub(super) fn parse_jsonrpc_body(body: &[u8]) -> StdResult<JSONRPCMessage, Box<Response>> {
    serde_json::from_slice(body).map_err(|error| {
        Box::new(
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid JSON body: {error}"),
            )
                .into_response(),
        )
    })
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn test_validate_origin_allows_same_host() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        assert!(validate_origin(&headers).is_ok());
    }

    #[test]
    fn test_validate_origin_rejects_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        let response = validate_origin(&headers).unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
