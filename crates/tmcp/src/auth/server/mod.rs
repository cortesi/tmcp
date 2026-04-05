//! Server-side OAuth 2.1 resource server support.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{self, AsHeaderName, InvalidHeaderValue, WWW_AUTHENTICATE},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use thiserror::Error;

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
    /// Validator used to authenticate bearer tokens.
    pub validator: Arc<dyn TokenValidator>,
    /// External endpoint path for the protected MCP routes.
    pub endpoint_path: String,
    /// Externally visible base URL used to construct absolute metadata URIs.
    ///
    /// Required by RFC 9728. When the server runs behind a reverse proxy, this is the
    /// public-facing origin (e.g. `https://example.com`). Request-time forwarding headers
    /// (`X-Forwarded-Host`, `X-Forwarded-Proto`) override this value when present.
    pub base_url: String,
    /// Authorization server issuer URLs advertised in the protected resource metadata.
    ///
    /// Defaults to `[base_url]`. Override with [`Self::with_authorization_servers`] when
    /// tokens are issued by an external identity provider.
    pub authorization_servers: Vec<String>,
    /// OAuth scopes advertised in the protected resource metadata.
    pub scopes_supported: Vec<String>,
    /// Bearer token delivery methods advertised in the protected resource metadata.
    pub bearer_methods_supported: Vec<String>,
}

impl AuthConfig {
    /// Create an auth configuration with the given base URL and token validator.
    ///
    /// The `base_url` is the externally visible origin (e.g. `https://example.com`). It is
    /// used to construct absolute `resource_metadata` URIs in `WWW-Authenticate` challenges
    /// and to serve RFC 9728 protected resource metadata. By default the base URL is also
    /// advertised as the sole authorization server; call [`Self::with_authorization_servers`]
    /// to override when tokens come from an external identity provider.
    pub fn new(base_url: impl Into<String>, validator: Arc<dyn TokenValidator>) -> Self {
        let base_url = base_url.into();
        Self {
            validator,
            endpoint_path: "/".to_string(),
            authorization_servers: vec![base_url.clone()],
            base_url,
            scopes_supported: vec!["mcp".to_string()],
            bearer_methods_supported: vec!["header".to_string()],
        }
    }

    /// Override the externally visible MCP endpoint path.
    pub fn with_endpoint_path(mut self, endpoint_path: impl Into<String>) -> Self {
        self.endpoint_path = normalize_endpoint_path(endpoint_path.into());
        self
    }

    /// Override the advertised authorization server URLs.
    ///
    /// Use this when tokens are issued by an external identity provider rather than the
    /// resource server itself (e.g. `["https://issuer.example.com"]`).
    pub fn with_authorization_servers(
        mut self,
        servers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.authorization_servers = servers.into_iter().map(Into::into).collect();
        self
    }

    /// Override the advertised OAuth scopes (default: `["mcp"]`).
    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scopes_supported = scopes.into_iter().map(Into::into).collect();
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

/// Shared state for dynamic protected-resource metadata handlers.
#[derive(Clone)]
struct ProtectedResourceState {
    /// Fallback externally visible base URL.
    base_url: String,
    /// MCP endpoint path component.
    endpoint_path: String,
    /// Authorization server issuer URLs.
    authorization_servers: Vec<String>,
    /// Scopes advertised in the metadata document.
    scopes_supported: Vec<String>,
    /// Bearer methods advertised in the metadata document.
    bearer_methods_supported: Vec<String>,
}

/// Return an RFC 9728 protected resource metadata router for the endpoint path.
///
/// The `base_url` is used as a fallback origin; request-time `X-Forwarded-Host` and
/// `X-Forwarded-Proto` headers take precedence so the metadata document reflects the
/// externally visible URL even when the server is behind a reverse proxy.
pub fn protected_resource_handler(config: &AuthConfig) -> Router {
    let state = ProtectedResourceState {
        base_url: config.base_url.trim_end_matches('/').to_string(),
        endpoint_path: normalize_endpoint_path(&config.endpoint_path),
        authorization_servers: config.authorization_servers.clone(),
        scopes_supported: config.scopes_supported.clone(),
        bearer_methods_supported: config.bearer_methods_supported.clone(),
    };

    let mut router = Router::new().route(
        WELL_KNOWN_PROTECTED_RESOURCE_PATH,
        get(serve_protected_resource_metadata),
    );

    let endpoint_path = normalize_endpoint_path(&config.endpoint_path);
    if endpoint_path != "/" {
        let path = resource_metadata_relative_uri(&endpoint_path);
        router = router.route(&path, get(serve_protected_resource_metadata));
    }

    router.with_state(state)
}

/// Handler that builds protected resource metadata dynamically from the request origin.
async fn serve_protected_resource_metadata(
    State(state): State<ProtectedResourceState>,
    headers: HeaderMap,
) -> Json<Value> {
    let base = resolve_base_url(&headers, &state.base_url);
    Json(json!({
        "resource": format!("{base}{}", state.endpoint_path),
        "authorization_servers": state.authorization_servers,
        "scopes_supported": state.scopes_supported,
        "bearer_methods_supported": state.bearer_methods_supported,
    }))
}

/// Resolve the externally visible origin from forwarding headers or a fallback base URL.
///
/// Only uses request headers when explicit forwarding headers (`X-Forwarded-Host`,
/// `X-Forwarded-Proto`) are present, indicating a reverse proxy. Otherwise the configured
/// base URL is returned unchanged.
fn resolve_base_url(headers: &HeaderMap, fallback: &str) -> String {
    let forwarded_host = first_forwarded_value(headers, "x-forwarded-host");
    let forwarded_proto = first_forwarded_value(headers, "x-forwarded-proto");

    if forwarded_host.is_none() && forwarded_proto.is_none() {
        return fallback.trim_end_matches('/').to_string();
    }

    let host = forwarded_host.or_else(|| first_forwarded_value(headers, header::HOST));
    let scheme = forwarded_proto.or_else(|| fallback.split_once("://").map(|(s, _)| s));

    match (scheme, host) {
        (Some(scheme), Some(host)) => format!("{scheme}://{host}"),
        _ => fallback.trim_end_matches('/').to_string(),
    }
}

/// Return the first value from a potentially comma-separated forwarding header.
fn first_forwarded_value<K: AsHeaderName>(headers: &HeaderMap, name: K) -> Option<&str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
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
    use axum::body::{self, Body};
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    fn test_config(endpoint_path: &str) -> AuthConfig {
        AuthConfig::new("https://example.com", stub_validator()).with_endpoint_path(endpoint_path)
    }

    fn stub_validator() -> Arc<dyn TokenValidator> {
        struct Stub;

        #[async_trait]
        impl TokenValidator for Stub {
            async fn validate(&self, _: &str) -> Result<AuthInfo, AuthError> {
                Err(AuthError::Invalid)
            }
        }

        Arc::new(Stub)
    }

    #[tokio::test]
    async fn protected_resource_handler_serves_root_metadata() {
        let response = protected_resource_handler(&test_config("/"))
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
        let response = protected_resource_handler(&test_config("/mcp"))
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

    #[tokio::test]
    async fn protected_resource_metadata_uses_forwarded_headers() {
        let response = protected_resource_handler(&test_config("/mcp"))
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/mcp")
                    .header("x-forwarded-host", "prod.example.com")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = body::to_bytes(response.into_body(), 4096).await.unwrap();
        let metadata: Value = serde_json::from_slice(&body).unwrap();
        // Resource URI reflects the forwarded public host.
        assert_eq!(metadata["resource"], "https://prod.example.com/mcp");
        // Authorization servers come from config, not from forwarding headers.
        assert_eq!(
            metadata["authorization_servers"],
            json!(["https://example.com"])
        );
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
