//! Server-side OAuth 2.1 resource server support.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    http::{
        HeaderValue, StatusCode,
        header::{InvalidHeaderValue, WWW_AUTHENTICATE},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use thiserror::Error;

use super::ProtectedResourceMetadata;

/// JWT/JWKS-backed bearer token validation.
mod jwt;
/// Tower middleware for bearer-token enforcement.
mod middleware;

pub use jwt::JwtValidator;
pub use middleware::BearerAuthLayer;

/// Relative well-known path prefix for protected resource metadata.
const WELL_KNOWN_PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// High-level HTTP auth configuration for a protected MCP endpoint.
#[derive(Clone)]
pub struct AuthConfig {
    /// Protected resource metadata served from the public well-known endpoint.
    pub metadata: ProtectedResourceMetadata,
    /// Validator used to authenticate bearer tokens.
    pub validator: Arc<dyn TokenValidator>,
    /// External endpoint path for the protected MCP routes.
    pub endpoint_path: String,
}

impl AuthConfig {
    /// Create an auth configuration for the default root endpoint.
    pub fn new(metadata: ProtectedResourceMetadata, validator: Arc<dyn TokenValidator>) -> Self {
        Self {
            metadata,
            validator,
            endpoint_path: "/".to_string(),
        }
    }

    /// Override the externally visible MCP endpoint path.
    pub fn with_endpoint_path(mut self, endpoint_path: impl Into<String>) -> Self {
        self.endpoint_path = normalize_endpoint_path(endpoint_path.into());
        self
    }
}

/// Validated identity information extracted from a bearer token.
#[derive(Clone, Debug)]
pub struct AuthInfo {
    /// Subject identifier from the validated token.
    pub subject: String,
    /// Granted scopes.
    pub scopes: HashSet<String>,
    /// Audience claims accepted for this token.
    pub audiences: Vec<String>,
    /// Remaining custom claims.
    pub extra: Value,
}

impl AuthInfo {
    /// Return true if the token grants the requested scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }

    /// Return true if the token grants every requested scope.
    pub fn has_all_scopes(&self, scopes: &[&str]) -> bool {
        scopes.iter().all(|scope| self.has_scope(scope))
    }
}

/// Errors returned by bearer token validation.
#[derive(Debug, Clone, Error)]
pub enum AuthError {
    /// The token is invalid.
    #[error("invalid token")]
    Invalid,
    /// The token is expired.
    #[error("expired token")]
    Expired,
    /// Token validation infrastructure is temporarily unavailable.
    #[error("{0}")]
    Unavailable(String),
}

/// Validates bearer tokens and returns authenticated request context.
#[async_trait]
pub trait TokenValidator: Send + Sync {
    /// Validate a bearer token and return the authenticated identity.
    async fn validate(&self, token: &str) -> Result<AuthInfo, AuthError>;
}

/// Builder for `WWW-Authenticate: Bearer ...` challenge headers.
#[derive(Clone, Debug, Default)]
pub struct WwwAuthenticate {
    /// Bearer challenge parameters.
    params: Vec<(&'static str, String)>,
}

impl WwwAuthenticate {
    /// Create a bearer challenge with the provided resource metadata URI.
    pub fn bearer(resource_metadata: impl Into<String>) -> Self {
        Self::default().resource_metadata(resource_metadata)
    }

    /// Attach a `resource_metadata` parameter.
    pub fn resource_metadata(mut self, resource_metadata: impl Into<String>) -> Self {
        self.params
            .push(("resource_metadata", resource_metadata.into()));
        self
    }

    /// Attach an OAuth error code.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.params.push(("error", error.into()));
        self
    }

    /// Attach an OAuth error description.
    pub fn error_description(mut self, description: impl Into<String>) -> Self {
        self.params.push(("error_description", description.into()));
        self
    }

    /// Attach a required scope hint.
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.params.push(("scope", scope.into()));
        self
    }

    /// Build the final header value.
    pub fn header_value(self) -> Result<HeaderValue, InvalidHeaderValue> {
        let mut value = String::from("Bearer");
        if !self.params.is_empty() {
            let params = self
                .params
                .into_iter()
                .map(|(key, value)| format!("{key}=\"{}\"", escape_header_value(&value)))
                .collect::<Vec<_>>()
                .join(", ");
            value.push(' ');
            value.push_str(&params);
        }
        HeaderValue::from_str(&value)
    }

    /// Build a bare 401 bearer challenge.
    pub fn bearer_challenge(
        resource_metadata: impl Into<String>,
    ) -> Result<HeaderValue, InvalidHeaderValue> {
        Self::bearer(resource_metadata).header_value()
    }

    /// Build a 401 invalid-token challenge.
    pub fn invalid_token_challenge(
        resource_metadata: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<HeaderValue, InvalidHeaderValue> {
        Self::bearer(resource_metadata)
            .error("invalid_token")
            .error_description(description)
            .header_value()
    }

    /// Build a 403 insufficient-scope challenge.
    pub fn insufficient_scope_challenge(
        resource_metadata: impl Into<String>,
        required_scope: impl Into<String>,
    ) -> Result<HeaderValue, InvalidHeaderValue> {
        Self::bearer(resource_metadata)
            .error("insufficient_scope")
            .scope(required_scope)
            .header_value()
    }
}

/// Return an RFC 9728 protected resource metadata router for the endpoint path.
pub fn protected_resource_handler(
    metadata: ProtectedResourceMetadata,
    endpoint_path: &str,
) -> Router {
    let metadata = Arc::new(metadata);
    let mut router = Router::new().route(
        WELL_KNOWN_PROTECTED_RESOURCE_PATH,
        get({
            let metadata = metadata.clone();
            move || {
                let metadata = metadata.clone();
                async move { Json((*metadata).clone()) }
            }
        }),
    );

    let endpoint_path = normalize_endpoint_path(endpoint_path);
    if endpoint_path != "/" {
        let path = resource_metadata_relative_uri(&endpoint_path);
        router = router.route(
            &path,
            get(move || {
                let metadata = metadata.clone();
                async move { Json((*metadata).clone()) }
            }),
        );
    }

    router
}

/// Return a 403 response describing the required scope.
pub fn insufficient_scope_response(
    endpoint_path: &str,
    required_scope: impl Into<String>,
) -> Response {
    let required_scope = required_scope.into();
    let resource_metadata = resource_metadata_relative_uri(endpoint_path);
    let challenge =
        WwwAuthenticate::insufficient_scope_challenge(resource_metadata, required_scope.clone())
            .expect("scope challenge must serialize");

    let mut response = (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "insufficient_scope",
            "required_scope": required_scope,
        })),
    )
        .into_response();
    response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
    response
}

/// Normalize an externally visible endpoint path.
pub(crate) fn normalize_endpoint_path(endpoint_path: impl Into<String>) -> String {
    let endpoint_path = endpoint_path.into();
    let trimmed = endpoint_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }

    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Return the relative protected-resource metadata URI for an endpoint path.
pub(crate) fn resource_metadata_relative_uri(endpoint_path: &str) -> String {
    let endpoint_path = normalize_endpoint_path(endpoint_path);
    if endpoint_path == "/" {
        WELL_KNOWN_PROTECTED_RESOURCE_PATH.to_string()
    } else {
        format!("{WELL_KNOWN_PROTECTED_RESOURCE_PATH}{endpoint_path}")
    }
}

/// Escape a header parameter value.
fn escape_header_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Split a string-based scope claim into a set of scopes.
pub(crate) fn parse_scope_set(scope: &str) -> HashSet<String> {
    scope
        .split_whitespace()
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Remove standard claims from a claim map and keep only custom values.
pub(crate) fn extra_claims(mut claims: HashMap<String, Value>) -> Value {
    for key in ["aud", "exp", "iat", "iss", "nbf", "scope", "scp", "sub"] {
        claims.remove(key);
    }
    Value::Object(claims.into_iter().collect())
}

/// Default JWT/JWKS cache TTL.
pub(crate) const DEFAULT_JWKS_CACHE_TTL: Duration = Duration::from_secs(300);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    fn metadata() -> ProtectedResourceMetadata {
        ProtectedResourceMetadata {
            resource: "https://example.com/mcp".to_string(),
            authorization_servers: vec!["https://issuer.example.com".to_string()],
            scopes_supported: None,
            bearer_methods_supported: None,
            resource_documentation: None,
            additional: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn protected_resource_handler_serves_root_metadata() {
        let response = protected_resource_handler(metadata(), "/")
            .oneshot(
                Request::builder()
                    .uri(WELL_KNOWN_PROTECTED_RESOURCE_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn protected_resource_handler_serves_path_specific_metadata() {
        let response = protected_resource_handler(metadata(), "/mcp")
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn www_authenticate_formats_bearer_challenge() {
        let header =
            WwwAuthenticate::bearer_challenge("/.well-known/oauth-protected-resource").unwrap();
        assert_eq!(
            header.to_str().unwrap(),
            "Bearer resource_metadata=\"/.well-known/oauth-protected-resource\""
        );
    }

    #[test]
    fn www_authenticate_formats_insufficient_scope_challenge() {
        let header = WwwAuthenticate::insufficient_scope_challenge(
            "/.well-known/oauth-protected-resource/mcp",
            "tools:call",
        )
        .unwrap();
        assert_eq!(
            header.to_str().unwrap(),
            "Bearer resource_metadata=\"/.well-known/oauth-protected-resource/mcp\", error=\"insufficient_scope\", scope=\"tools:call\""
        );
    }
}
