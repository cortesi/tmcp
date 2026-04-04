use std::{
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
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
    /// Relative resource metadata URI included in challenges.
    resource_metadata: String,
}

impl BearerAuthLayer {
    /// Create a new bearer-auth layer for the given endpoint path.
    pub fn new(validator: Arc<dyn TokenValidator>, endpoint_path: &str) -> Self {
        Self {
            validator,
            resource_metadata: resource_metadata_relative_uri(endpoint_path),
        }
    }
}

impl<S> Layer<S> for BearerAuthLayer {
    type Service = BearerAuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BearerAuthMiddleware {
            inner,
            validator: self.validator.clone(),
            resource_metadata: self.resource_metadata.clone(),
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
    /// Relative resource metadata URI included in challenges.
    resource_metadata: String,
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
        let resource_metadata = self.resource_metadata.clone();

        Box::pin(async move {
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
