//! Request validation helpers shared by the HTTP server handlers.

use std::{error::Error as StdError, result::Result as StdResult};

use axum::{
    body::{self, Body},
    http::{HeaderMap, StatusCode, header, uri::Authority},
    response::{IntoResponse, Response},
};
use url::Url;

use super::CorsPolicy;
use crate::schema::{JSONRPCMessage, ProtocolVersion};

/// Maximum accepted HTTP request body size.
const MAX_HTTP_BODY_SIZE: usize = 2 * 1024 * 1024;

/// Result type used for origin validation checks.
pub(super) type OriginResult = StdResult<(), Box<Response>>;

/// Build a 403 response for an invalid Origin header.
fn forbidden_origin() -> Box<Response> {
    Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response())
}

/// Validate the Origin header according to the configured CORS policy.
pub(super) fn validate_origin(headers: &HeaderMap, policy: &CorsPolicy) -> OriginResult {
    match policy {
        CorsPolicy::Permissive => Ok(()),
        CorsPolicy::SameOrigin => validate_same_origin(headers),
        CorsPolicy::AllowList(origins) => {
            let Some(origin) = headers.get(header::ORIGIN) else {
                return Ok(());
            };
            let origin = origin.to_str().map_err(|_| forbidden_origin())?;
            if origins
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(origin))
            {
                Ok(())
            } else {
                Err(forbidden_origin())
            }
        }
    }
}

/// Validate the Origin header against the request Host when present.
fn validate_same_origin(headers: &HeaderMap) -> OriginResult {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };

    let origin_str = origin.to_str().map_err(|_| forbidden_origin())?;
    if origin_str == "null" {
        return Err(forbidden_origin());
    }

    let origin_url = Url::parse(origin_str).map_err(|_| forbidden_origin())?;
    if !matches!(origin_url.scheme(), "http" | "https") {
        return Err(forbidden_origin());
    }

    let host_header = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(forbidden_origin)?;
    let authority = host_header
        .parse::<Authority>()
        .map_err(|_| forbidden_origin())?;

    let origin_host = origin_url.host_str().ok_or_else(forbidden_origin)?;
    if !origin_host.eq_ignore_ascii_case(authority.host()) {
        return Err(forbidden_origin());
    }

    if let Some(expected_port) = authority.port_u16() {
        if origin_url.port_or_known_default() != Some(expected_port) {
            return Err(forbidden_origin());
        }
    } else if origin_url.port().is_some() {
        return Err(forbidden_origin());
    }

    Ok(())
}

/// Validate the MCP-Protocol-Version header, if present.
///
/// A missing header is accepted and treated as the oldest supported version,
/// per the streamable HTTP specification.
pub(super) fn validate_protocol_version(
    headers: &HeaderMap,
    expected: Option<&ProtocolVersion>,
) -> StdResult<(), Box<Response>> {
    let Some(version) = headers.get("MCP-Protocol-Version") else {
        return Ok(());
    };
    let Ok(version) = version.to_str() else {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, "Invalid protocol version").into_response(),
        ));
    };
    let Ok(version) = version.parse::<ProtocolVersion>() else {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, "Invalid protocol version").into_response(),
        ));
    };
    if expected.is_some_and(|expected| expected != &version) {
        Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                "Protocol version does not match session",
            )
                .into_response(),
        ))
    } else {
        Ok(())
    }
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

/// Return true if any error in the source chain is a body length-limit error.
fn is_length_limit_error(error: &(dyn StdError + 'static)) -> bool {
    let mut source: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(err) = source {
        if err.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = err.source();
    }
    false
}

/// Read the full HTTP request body with an explicit size limit.
pub(super) async fn read_json_body(body: Body) -> StdResult<bytes::Bytes, Box<Response>> {
    body::to_bytes(body, MAX_HTTP_BODY_SIZE)
        .await
        .map_err(|error| {
            let status = if is_length_limit_error(&error) {
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

        assert!(validate_origin(&headers, &CorsPolicy::SameOrigin).is_ok());
    }

    #[test]
    fn test_validate_origin_rejects_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        let response = validate_origin(&headers, &CorsPolicy::SameOrigin).unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_permissive_policy_allows_any_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://evil.com"));
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        assert!(validate_origin(&headers, &CorsPolicy::Permissive).is_ok());
    }

    #[test]
    fn test_allow_list_policy() {
        let policy = CorsPolicy::AllowList(vec!["https://app.example.com".to_string()]);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.example.com"),
        );
        assert!(validate_origin(&headers, &policy).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://other.example.com"),
        );
        let response = validate_origin(&headers, &policy).unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_protocol_version_validation() {
        let headers = HeaderMap::new();
        assert!(validate_protocol_version(&headers, None).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert(
            "MCP-Protocol-Version",
            HeaderValue::from_static("2025-06-18"),
        );
        assert!(validate_protocol_version(&headers, None).is_ok());

        let expected = "2025-11-25".parse().unwrap();
        let response = validate_protocol_version(&headers, Some(&expected)).unwrap_err();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let mut headers = HeaderMap::new();
        headers.insert(
            "MCP-Protocol-Version",
            HeaderValue::from_static("not-a-version"),
        );
        let response = validate_protocol_version(&headers, None).unwrap_err();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
