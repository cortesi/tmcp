use std::{
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    http::{
        HeaderMap, Request, StatusCode,
        header::{self, AUTHORIZATION, AsHeaderName, WWW_AUTHENTICATE},
    },
    response::{IntoResponse, Response},
};
use futures::future::BoxFuture;
use tower::{Layer, Service};

use super::{AuthError, TokenValidator, WwwAuthenticate, resource_metadata_relative_uri};

/// Tower layer that enforces bearer-token authentication.
#[derive(Clone)]
pub struct BearerAuthLayer {
    /// Token validator shared across cloned services.
    validator: Arc<dyn TokenValidator>,
    /// Relative resource metadata path (e.g. `/.well-known/oauth-protected-resource/mcp`).
    resource_metadata_path: String,
    /// Fallback base URL for constructing absolute resource_metadata URIs.
    base_url: String,
}

impl BearerAuthLayer {
    /// Create a new bearer-auth layer for the given endpoint path.
    pub fn new(validator: Arc<dyn TokenValidator>, endpoint_path: &str) -> Self {
        Self {
            validator,
            resource_metadata_path: resource_metadata_relative_uri(endpoint_path),
            base_url: String::new(),
        }
    }

    /// Set the fallback base URL for absolute resource-metadata URIs.
    ///
    /// RFC 9728 requires the `resource_metadata` parameter in `WWW-Authenticate` challenges
    /// to be an absolute URI. The middleware resolves the public origin from forwarding
    /// headers at request time, falling back to this value when no forwarding headers are
    /// present.
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }
}

impl<S> Layer<S> for BearerAuthLayer {
    type Service = BearerAuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BearerAuthMiddleware {
            inner,
            validator: self.validator.clone(),
            resource_metadata_path: self.resource_metadata_path.clone(),
            base_url: self.base_url.clone(),
        }
    }
}

/// Service produced by [`BearerAuthLayer`].
#[derive(Clone)]
pub struct BearerAuthMiddleware<S> {
    /// Wrapped service.
    inner: S,
    /// Token validator.
    validator: Arc<dyn TokenValidator>,
    /// Relative resource metadata path.
    resource_metadata_path: String,
    /// Fallback base URL.
    base_url: String,
}

impl<S, B> Service<Request<B>> for BearerAuthMiddleware<S>
where
    S: Service<Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<B>) -> Self::Future {
        let mut inner = self.inner.clone();
        let validator = self.validator.clone();
        let resource_metadata_path = self.resource_metadata_path.clone();
        let base_url = self.base_url.clone();

        Box::pin(async move {
            let resource_metadata =
                resolve_resource_metadata(request.headers(), &base_url, &resource_metadata_path);

            let Some(token) = bearer_token(request.headers()) else {
                return Ok(unauthorized_response(
                    WwwAuthenticate::bearer_challenge(resource_metadata)
                        .expect("bearer challenge must serialize"),
                ));
            };

            match validator.validate(token).await {
                Ok(auth_info) => {
                    request.extensions_mut().insert(auth_info);
                    inner.call(request).await
                }
                Err(AuthError::Invalid) => Ok(unauthorized_response(
                    WwwAuthenticate::invalid_token_challenge(
                        resource_metadata,
                        "Invalid access token",
                    )
                    .expect("invalid-token challenge must serialize"),
                )),
                Err(AuthError::Expired) => Ok(unauthorized_response(
                    WwwAuthenticate::invalid_token_challenge(
                        resource_metadata,
                        "Expired access token",
                    )
                    .expect("expired-token challenge must serialize"),
                )),
                Err(AuthError::Unavailable(message)) => {
                    Ok((StatusCode::SERVICE_UNAVAILABLE, message).into_response())
                }
            }
        })
    }
}

/// Build an absolute resource_metadata URI from the request's forwarding headers.
fn resolve_resource_metadata(
    headers: &HeaderMap,
    fallback_base: &str,
    relative_path: &str,
) -> String {
    let base = resolve_base_url(headers, fallback_base);
    if base.is_empty() {
        relative_path.to_string()
    } else {
        format!("{base}{relative_path}")
    }
}

/// Resolve the externally visible origin from forwarding headers or a fallback base URL.
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

/// Extract a bearer token from an `Authorization` header.
fn bearer_token(headers: &http::HeaderMap) -> Option<&str> {
    let authorization = headers.get(AUTHORIZATION)?;
    let authorization = authorization.to_str().ok()?;
    let (scheme, token) = authorization.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Some(token)
    } else {
        None
    }
}

/// Build a 401 response with the provided bearer challenge.
fn unauthorized_response(challenge: http::HeaderValue) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
    *response.body_mut() = Body::empty();
    response
}
