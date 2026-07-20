use std::{
    fmt, str,
    time::{Duration, Instant, SystemTime},
};

use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, StandardRevocableToken,
    TokenResponse, TokenUrl,
    basic::{
        BasicClient, BasicErrorResponse, BasicRevocationErrorResponse,
        BasicTokenIntrospectionResponse, BasicTokenResponse,
    },
    reqwest::Client,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock, watch},
};
use url::{Url, form_urlencoded};

use super::{
    dynamic_registration::{ClientMetadata, DynamicRegistrationClient},
    util::require_https_or_loopback,
};
use crate::{error::Error, http::is_loopback_http_url};

/// Time before expiration at which tokens are proactively refreshed.
const TOKEN_REFRESH_LEEWAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
/// OAuth2 client configuration values.
pub struct OAuth2Config {
    /// OAuth client identifier.
    pub client_id: String,
    /// OAuth client secret, if applicable.
    pub client_secret: Option<String>,
    /// Authorization endpoint URL.
    pub auth_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Redirect/callback URL.
    pub redirect_url: String,
    /// Resource audience for MCP.
    pub resource: String,
    /// Requested OAuth scopes.
    pub scopes: Vec<String>,
}

#[derive(Clone)]
/// OAuth2 token information.
pub struct OAuth2Token {
    /// Access token value.
    pub access_token: String,
    /// Optional refresh token value.
    pub refresh_token: Option<String>,
    /// Optional token lifetime.
    pub expires_in: Option<Duration>,
    /// Optional expiration instant.
    pub expires_at: Option<Instant>,
}

impl fmt::Debug for OAuth2Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2Token")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_in", &self.expires_in)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl OAuth2Token {
    /// Create a token from a durable wall-clock expiration time.
    pub fn from_system_time(
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<SystemTime>,
    ) -> Self {
        Self::from_system_time_at(
            access_token,
            refresh_token,
            expires_at,
            Instant::now(),
            SystemTime::now(),
        )
    }

    /// Return the runtime expiration as a durable wall-clock time.
    pub fn system_expires_at(&self) -> Option<SystemTime> {
        self.system_expires_at_at(Instant::now(), SystemTime::now())
    }

    /// Convert wall-clock expiry using one captured clock pair.
    fn from_system_time_at(
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<SystemTime>,
        instant_now: Instant,
        system_now: SystemTime,
    ) -> Self {
        let (expires_in, expires_at) = expires_at.map_or((None, None), |expires_at| {
            let remaining = expires_at
                .duration_since(system_now)
                .unwrap_or(Duration::ZERO);
            let remaining = if instant_now.checked_add(remaining).is_some() {
                remaining
            } else {
                Duration::ZERO
            };
            (Some(remaining), Some(instant_now + remaining))
        });
        Self {
            access_token,
            refresh_token,
            expires_in,
            expires_at,
        }
    }

    /// Convert runtime expiry using one captured clock pair.
    fn system_expires_at_at(
        &self,
        instant_now: Instant,
        system_now: SystemTime,
    ) -> Option<SystemTime> {
        let remaining = self
            .expires_at?
            .checked_duration_since(instant_now)
            .unwrap_or(Duration::ZERO);
        system_now.checked_add(remaining)
    }
}

/// Configuration for OAuth 2.0 dynamic client registration (RFC 7591).
#[derive(Debug, Clone)]
pub struct DynamicRegistrationConfig {
    /// Authorization endpoint URL of the identity provider.
    pub auth_url: String,
    /// Token endpoint URL of the identity provider.
    pub token_url: String,
    /// Resource audience the registered client requests tokens for.
    pub resource: String,
    /// Explicit registration endpoint; discovered from the auth URL origin when `None`.
    pub registration_endpoint: Option<String>,
    /// Client metadata submitted to the registration endpoint.
    pub metadata: ClientMetadata,
}

/// In-flight authorization code flow state.
///
/// Created by [`OAuth2Client::begin_authorization`]; holds the PKCE verifier and CSRF
/// token for one flow. Direct the user to [`Self::auth_url`], then consume the flow with
/// [`OAuth2Client::exchange_code`].
pub struct AuthorizationFlow {
    /// Authorization URL the user must visit.
    auth_url: Url,
    /// PKCE verifier matching the challenge embedded in the auth URL.
    pkce_verifier: PkceCodeVerifier,
    /// Expected CSRF state value for the callback.
    csrf_token: CsrfToken,
}

impl AuthorizationFlow {
    /// The authorization URL the user must visit to grant access.
    pub fn auth_url(&self) -> &Url {
        &self.auth_url
    }
}

/// Internal OAuth2 client type alias with configured endpoint states.
type ConfiguredClient = oauth2::Client<
    BasicErrorResponse,
    BasicTokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

/// OAuth2 client with token caching and refresh support.
pub struct OAuth2Client {
    /// OAuth2 client used for token flows.
    client: ConfiguredClient,
    /// Configuration used to build the OAuth2 client.
    config: OAuth2Config,
    /// Cached token stored behind a lock.
    token: RwLock<Option<OAuth2Token>>,
    /// Mutex serializing every token lifecycle mutation.
    lifecycle_lock: Mutex<()>,
    /// Monotonic notification of installed token revisions.
    token_revision_tx: watch::Sender<u64>,
}

impl OAuth2Client {
    /// Perform dynamic client registration and create an `OAuth2Client`.
    ///
    /// When `config.metadata` carries no resource, the configured resource is filled in
    /// so the registered client is audience-bound as required by MCP.
    pub async fn register_dynamic(config: DynamicRegistrationConfig) -> Result<Self, Error> {
        let DynamicRegistrationConfig {
            auth_url,
            token_url,
            resource,
            registration_endpoint,
            mut metadata,
        } = config;

        if metadata.resource.is_none() {
            metadata.resource = Some(resource.clone());
        }

        let registration_client = DynamicRegistrationClient::for_endpoint(
            registration_endpoint.as_deref().unwrap_or(&auth_url),
        )?;
        let reg_endpoint =
            Self::registration_endpoint(&registration_client, &auth_url, registration_endpoint)
                .await?;

        let registration = registration_client
            .register(&reg_endpoint, metadata, None)
            .await?;

        let config = OAuth2Config::from_registration(registration, auth_url, token_url, resource)?;
        Self::new(config)
    }

    /// Resolve a dynamic registration endpoint from explicit input or discovery.
    async fn registration_endpoint(
        registration_client: &DynamicRegistrationClient,
        auth_url: &str,
        registration_endpoint: Option<String>,
    ) -> Result<String, Error> {
        if let Some(endpoint) = registration_endpoint {
            return Ok(endpoint);
        }

        let auth_url_parsed = Url::parse(auth_url)
            .map_err(|e| Error::InvalidConfiguration(format!("Invalid auth URL: {e}")))?;
        if !matches!(auth_url_parsed.scheme(), "http" | "https") {
            return Err(Error::InvalidConfiguration(
                "Auth URL must use http or https".to_string(),
            ));
        }
        let issuer = auth_url_parsed.origin().ascii_serialization();

        registration_client
            .discover_registration_endpoint(&issuer)
            .await?
            .ok_or_else(|| {
                Error::InvalidConfiguration(
                    "No registration endpoint found in OAuth metadata".to_string(),
                )
            })
    }

    /// Create a new OAuth2 client from configuration.
    ///
    /// The authorization, token, and redirect URLs must use HTTPS; plain HTTP is only
    /// accepted for loopback hosts (`127.0.0.1`, `::1`, `localhost`).
    pub fn new(config: OAuth2Config) -> Result<Self, Error> {
        require_https_or_loopback(&config.auth_url, "auth URL")?;
        require_https_or_loopback(&config.token_url, "token URL")?;
        require_https_or_loopback(&config.redirect_url, "redirect URL")?;

        let mut client = BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(config.auth_url.clone())
                    .map_err(|e| Error::InvalidConfiguration(format!("Invalid auth URL: {e}")))?,
            )
            .set_token_uri(
                TokenUrl::new(config.token_url.clone())
                    .map_err(|e| Error::InvalidConfiguration(format!("Invalid token URL: {e}")))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(config.redirect_url.clone()).map_err(|e| {
                    Error::InvalidConfiguration(format!("Invalid redirect URL: {e}"))
                })?,
            );

        if let Some(client_secret) = config.client_secret.as_ref() {
            client = client.set_client_secret(ClientSecret::new(client_secret.clone()));
        }

        let (token_revision_tx, _) = watch::channel(0);
        Ok(Self {
            client,
            config,
            token: RwLock::new(None),
            lifecycle_lock: Mutex::new(()),
            token_revision_tx,
        })
    }

    /// Begin an authorization code flow with PKCE.
    ///
    /// Returns the flow state holding the generated PKCE verifier and CSRF token. Direct
    /// the user to [`AuthorizationFlow::auth_url`] and complete the flow with
    /// [`Self::exchange_code`].
    pub fn begin_authorization(&self) -> AuthorizationFlow {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut auth_request = self
            .client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge)
            .add_extra_param("resource", &self.config.resource);

        for scope in &self.config.scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.clone()));
        }

        let (auth_url, csrf_token) = auth_request.url();
        AuthorizationFlow {
            auth_url,
            pkce_verifier,
            csrf_token,
        }
    }

    /// Exchange an authorization code for a token, consuming the flow state.
    ///
    /// The callback `state` is compared against the flow's CSRF token in constant time.
    pub async fn exchange_code(
        &self,
        flow: AuthorizationFlow,
        code: String,
        state: String,
    ) -> Result<OAuth2Token, Error> {
        let state_matches: bool = state
            .as_bytes()
            .ct_eq(flow.csrf_token.secret().as_bytes())
            .into();
        if !state_matches {
            return Err(Error::AuthorizationFailed("CSRF token mismatch".into()));
        }
        let _lifecycle_guard = self.lifecycle_lock.lock().await;

        let mut token_request = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(flow.pkce_verifier);

        // Only add resource parameter if it's not empty (some providers don't support it)
        if !self.config.resource.is_empty() {
            token_request = token_request.add_extra_param("resource", &self.config.resource);
        }

        let token_result = token_request
            .request_async(&oauth_http_client(&self.config.token_url)?)
            .await
            .map_err(|e| {
                // Try to extract OAuth error details from the response
                if let oauth2::RequestTokenError::ServerResponse(resp) = &e {
                    if let Some(error_description) = resp.error_description() {
                        return Error::AuthorizationFailed(format!(
                            "OAuth error ({}): {}",
                            resp.error(),
                            error_description
                        ));
                    } else {
                        return Error::AuthorizationFailed(format!(
                            "OAuth error: {}",
                            resp.error()
                        ));
                    }
                }
                Error::AuthorizationFailed(format!("Token exchange failed: {e}"))
            })?;

        let expires_in = token_result.expires_in();
        let oauth_token = OAuth2Token {
            access_token: token_result.access_token().secret().clone(),
            refresh_token: token_result.refresh_token().map(|t| t.secret().clone()),
            expires_in,
            expires_at: expires_in.map(|duration| Instant::now() + duration),
        };

        self.install_token_locked(oauth_token.clone()).await;
        Ok(oauth_token)
    }

    /// Retrieve a valid access token, refreshing if necessary.
    pub async fn get_valid_token(&self) -> Result<String, Error> {
        let now = Instant::now();
        {
            let token_guard = self.token.read().await;
            if let Some(token) = &*token_guard
                && token_is_fresh(token, now)
            {
                return Ok(token.access_token.clone());
            }
        }

        let _lifecycle_guard = self.lifecycle_lock.lock().await;

        // Check again after obtaining the lock in case another task refreshed
        let refresh_token_opt = {
            let token_guard = self.token.read().await;
            if let Some(token) = &*token_guard {
                if token_is_fresh(token, Instant::now()) {
                    return Ok(token.access_token.clone());
                }
                token.refresh_token.clone()
            } else {
                None
            }
        };

        if let Some(refresh_token) = refresh_token_opt {
            let token = self.request_refreshed_token(&refresh_token).await?;
            let access_token = token.access_token.clone();
            self.install_token_locked(token).await;
            Ok(access_token)
        } else {
            Err(Error::AuthorizationFailed(
                "No valid token available".to_string(),
            ))
        }
    }

    /// Refresh the access token if the cached token still matches the caller's token.
    pub async fn refresh_access_token_if_current(
        &self,
        current_access_token: &str,
    ) -> Result<String, Error> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let token = self.token.read().await.clone().ok_or_else(|| {
            Error::AuthorizationFailed("No token available for refresh".to_string())
        })?;

        if token.access_token != current_access_token {
            return Ok(token.access_token);
        }

        let refresh_token = token
            .refresh_token
            .ok_or_else(|| Error::AuthorizationFailed("No refresh token available".to_string()))?;
        let token = self.request_refreshed_token(&refresh_token).await?;
        let access_token = token.access_token.clone();
        self.install_token_locked(token).await;
        Ok(access_token)
    }

    /// Refresh the currently cached token unconditionally.
    pub async fn refresh_access_token(&self) -> Result<OAuth2Token, Error> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let refresh_token = self
            .token
            .read()
            .await
            .as_ref()
            .and_then(|token| token.refresh_token.clone())
            .ok_or_else(|| Error::AuthorizationFailed("No refresh token available".to_string()))?;
        let token = self.request_refreshed_token(&refresh_token).await?;
        self.install_token_locked(token.clone()).await;
        Ok(token)
    }

    /// Request a replacement token without mutating the cache.
    async fn request_refreshed_token(&self, refresh_token: &str) -> Result<OAuth2Token, Error> {
        let refresh_token_obj = RefreshToken::new(refresh_token.to_string());
        let mut refresh_request = self.client.exchange_refresh_token(&refresh_token_obj);

        // Only add resource parameter if it's not empty (some providers don't support it)
        if !self.config.resource.is_empty() {
            refresh_request = refresh_request.add_extra_param("resource", &self.config.resource);
        }
        for scope in &self.config.scopes {
            refresh_request = refresh_request.add_scope(Scope::new(scope.clone()));
        }

        let token_result = refresh_request
            .request_async(&oauth_http_client(&self.config.token_url)?)
            .await
            .map_err(|e| Error::AuthorizationFailed(format!("Token refresh failed: {e}")))?;

        let expires_in = token_result.expires_in();
        let oauth_token = OAuth2Token {
            access_token: token_result.access_token().secret().clone(),
            refresh_token: token_result
                .refresh_token()
                .map(|t| t.secret().clone())
                .or_else(|| Some(refresh_token.to_string())),
            expires_in,
            expires_at: expires_in.map(|duration| Instant::now() + duration),
        };

        Ok(oauth_token)
    }

    /// Set the current token in the cache.
    pub async fn set_token(&self, token: OAuth2Token) {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        self.install_token_locked(token).await;
    }

    /// Return the currently cached token, if one is present.
    pub async fn current_token(&self) -> Option<OAuth2Token> {
        self.token.read().await.clone()
    }

    /// Subscribe to monotonic notifications for installed token revisions.
    pub fn subscribe_token_revisions(&self) -> watch::Receiver<u64> {
        self.token_revision_tx.subscribe()
    }

    /// Install one token and publish its revision while holding the token lock.
    async fn install_token_locked(&self, token: OAuth2Token) {
        let mut current = self.token.write().await;
        *current = Some(token);
        self.token_revision_tx.send_modify(|revision| {
            *revision = revision
                .checked_add(1)
                .expect("OAuth token revision overflow");
        });
    }
}

/// Returns whether a token is usable without refreshing.
fn token_is_fresh(token: &OAuth2Token, now: Instant) -> bool {
    token
        .expires_at
        .map(|expires_at| expires_at > now + TOKEN_REFRESH_LEEWAY)
        .unwrap_or(true)
}

/// Builds an OAuth HTTP client, bypassing proxy lookup for loopback endpoints.
fn oauth_http_client(url: &str) -> Result<Client, Error> {
    let mut builder = Client::builder();
    if is_loopback_http_url(url) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| Error::Transport(format!("Failed to build OAuth HTTP client: {error}")))
}

/// Maximum length of callback query string accepted by the OAuth callback server.
const MAX_CALLBACK_QUERY_LEN: usize = 2 * 1024;
/// Maximum length of the callback HTTP request we accept.
const MAX_CALLBACK_REQUEST_LEN: usize = 8 * 1024;

/// Parse and validate OAuth callback query parameters.
///
/// Surfaces `error`/`error_description` parameters from OAuth error redirects as
/// authorization failures; otherwise both `code` and `state` are required.
fn parse_callback_query(query: Option<&str>) -> Result<(String, String), Error> {
    let query = query.unwrap_or_default();
    if query.is_empty() {
        return Err(Error::AuthorizationFailed(
            "Missing callback query".to_string(),
        ));
    }

    if query.len() > MAX_CALLBACK_QUERY_LEN {
        return Err(Error::AuthorizationFailed(
            "Callback query is too large".to_string(),
        ));
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => error_description = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(error) = error {
        let message = match error_description {
            Some(description) => format!("OAuth error ({error}): {description}"),
            None => format!("OAuth error: {error}"),
        };
        return Err(Error::AuthorizationFailed(message));
    }

    let code =
        code.ok_or_else(|| Error::AuthorizationFailed("Missing authorization code".to_string()))?;
    let state = state.ok_or_else(|| Error::AuthorizationFailed("Missing state".to_string()))?;

    Ok((code, state))
}

/// Split a request target into its path and optional query string.
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

/// Parse the HTTP request line and return the request target.
fn parse_request_line(line: &str) -> Result<&str, Error> {
    let line = line.trim_end_matches('\r');
    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Error::AuthorizationFailed("Missing HTTP method".to_string()))?;
    if method != "GET" {
        return Err(Error::AuthorizationFailed(
            "Invalid callback method".to_string(),
        ));
    }
    let target = parts
        .next()
        .ok_or_else(|| Error::AuthorizationFailed("Missing callback target".to_string()))?;
    Ok(target)
}

/// Read the HTTP request headers from the stream.
async fn read_http_request(stream: &mut TcpStream) -> Result<String, Error> {
    let mut buffer = Vec::new();
    let mut scratch = [0u8; 1024];
    loop {
        let bytes_read = stream
            .read(&mut scratch)
            .await
            .map_err(|e| Error::Transport(format!("Failed to read callback request: {e}")))?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&scratch[..bytes_read]);
        if buffer.len() > MAX_CALLBACK_REQUEST_LEN {
            return Err(Error::AuthorizationFailed(
                "Callback request is too large".to_string(),
            ));
        }
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    if buffer.is_empty() {
        return Err(Error::AuthorizationFailed(
            "Callback request is empty".to_string(),
        ));
    }

    let request = str::from_utf8(&buffer).map_err(|_| {
        Error::AuthorizationFailed("Callback request is not valid UTF-8".to_string())
    })?;

    Ok(request.to_string())
}

/// Send a minimal HTTP response with the provided body.
async fn send_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Error> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| Error::Transport(format!("Failed to send callback response: {e}")))?;
    stream
        .shutdown()
        .await
        .map_err(|e| Error::Transport(format!("Failed to close callback connection: {e}")))?;
    Ok(())
}

/// Minimal HTTP callback server for OAuth redirects.
///
/// The listener is bound eagerly on construction, so the server accepts connections as
/// soon as it exists.
pub struct OAuth2CallbackServer {
    /// Bound listener accepting callback connections.
    listener: TcpListener,
    /// Port the listener is bound to.
    port: u16,
}

impl OAuth2CallbackServer {
    /// Bind a callback server on `127.0.0.1` at the provided port.
    pub async fn new(port: u16) -> Result<Self, Error> {
        Self::bind(&format!("127.0.0.1:{port}")).await
    }

    /// Bind a callback server to `127.0.0.1:0`, letting the OS choose the port.
    pub async fn bind_loopback() -> Result<Self, Error> {
        Self::bind("127.0.0.1:0").await
    }

    /// Bind the callback listener to the provided address.
    async fn bind(addr: &str) -> Result<Self, Error> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| Error::Transport(format!("Failed to bind callback listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Transport(format!("Failed to inspect callback listener: {e}")))?
            .port();
        Ok(Self { listener, port })
    }

    /// Return the bound callback port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Return the redirect URL for this callback server.
    pub fn redirect_url(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// Wait for the OAuth redirect callback and return (code, state).
    ///
    /// Requests for paths other than `/callback` (e.g. `/favicon.ico`) receive a 404
    /// response and the server keeps waiting. OAuth error redirects surface their
    /// `error`/`error_description` parameters as an authorization failure.
    pub async fn wait_for_callback(&self) -> Result<(String, String), Error> {
        loop {
            let (mut stream, _) = self.listener.accept().await.map_err(|e| {
                Error::Transport(format!("Failed to accept callback connection: {e}"))
            })?;

            let request = read_http_request(&mut stream).await?;
            let request_line = request
                .lines()
                .next()
                .ok_or_else(|| Error::AuthorizationFailed("Missing request line".to_string()))?;
            let target = parse_request_line(request_line)?;
            let (path, query) = split_target(target);

            if path != "/callback" {
                send_http_response(
                    &mut stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    "Not Found",
                )
                .await?;
                continue;
            }

            match parse_callback_query(query) {
                Ok((code, state)) => {
                    send_http_response(
                        &mut stream,
                        "200 OK",
                        "text/html; charset=utf-8",
                        SUCCESS_HTML,
                    )
                    .await?;
                    return Ok((code, state));
                }
                Err(err) => {
                    let message = err.to_string();
                    send_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "text/plain; charset=utf-8",
                        &message,
                    )
                    .await?;
                    return Err(err);
                }
            }
        }
    }
}

/// HTML returned to the browser after a successful OAuth callback.
const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>Authorization Successful</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background-color: #f5f5f5;
        }
        .container {
            text-align: center;
            padding: 2rem;
            background: white;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        h1 { color: #22c55e; }
        p { color: #666; margin-top: 1rem; }
    </style>
</head>
<body>
    <div class="container">
        <h1>✓ Authorization Successful</h1>
        <p>You can now close this window and return to your terminal.</p>
    </div>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::*;

    async fn exchanged_token_response() -> Json<Value> {
        Json(json!({
            "access_token": "exchanged-access",
            "token_type": "Bearer",
            "refresh_token": "exchanged-refresh",
            "expires_in": 3600
        }))
    }

    fn test_config() -> OAuth2Config {
        OAuth2Config {
            client_id: "client".to_owned(),
            client_secret: None,
            auth_url: "https://example.com/authorize".to_owned(),
            token_url: "https://example.com/token".to_owned(),
            redirect_url: "http://127.0.0.1/callback".to_owned(),
            resource: "https://example.com/mcp".to_owned(),
            scopes: Vec::new(),
        }
    }

    #[test]
    fn oauth_token_debug_redacts_secrets() {
        let token = OAuth2Token {
            access_token: "secret-access".to_string(),
            refresh_token: Some("secret-refresh".to_string()),
            expires_in: Some(Duration::from_secs(60)),
            expires_at: None,
        };

        let debug = format!("{token:?}");
        assert!(!debug.contains("secret-access"));
        assert!(!debug.contains("secret-refresh"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn oauth_token_converts_wall_clock_expiry_from_one_clock_pair() {
        let instant_now = Instant::now();
        let system_now = UNIX_EPOCH + Duration::from_secs(1_000);
        let system_expiry = system_now + Duration::from_secs(60);
        let token = OAuth2Token::from_system_time_at(
            "access".to_owned(),
            Some("refresh".to_owned()),
            Some(system_expiry),
            instant_now,
            system_now,
        );

        assert_eq!(token.expires_in, Some(Duration::from_secs(60)));
        assert_eq!(
            token.expires_at,
            Some(instant_now + Duration::from_secs(60))
        );
        assert_eq!(
            token.system_expires_at_at(instant_now, system_now),
            Some(system_expiry)
        );

        let expired = OAuth2Token::from_system_time_at(
            "access".to_owned(),
            None,
            Some(system_now - Duration::from_secs(1)),
            instant_now,
            system_now,
        );
        assert_eq!(expired.expires_in, Some(Duration::ZERO));
        assert_eq!(expired.expires_at, Some(instant_now));
        assert_eq!(
            expired.system_expires_at_at(instant_now, system_now),
            Some(system_now)
        );

        let unbounded = OAuth2Token::from_system_time_at(
            "access".to_owned(),
            None,
            None,
            instant_now,
            system_now,
        );
        assert_eq!(unbounded.expires_in, None);
        assert_eq!(unbounded.expires_at, None);
        assert_eq!(
            unbounded.system_expires_at_at(instant_now, system_now),
            None
        );
    }

    #[tokio::test]
    async fn setting_token_publishes_revision_after_installation() {
        let client = OAuth2Client::new(test_config()).unwrap();
        let mut revisions = client.subscribe_token_revisions();

        client
            .set_token(OAuth2Token {
                access_token: "access".to_owned(),
                refresh_token: Some("refresh".to_owned()),
                expires_in: None,
                expires_at: None,
            })
            .await;

        revisions.changed().await.unwrap();
        assert_eq!(*revisions.borrow_and_update(), 1);
        assert_eq!(client.current_token().await.unwrap().access_token, "access");
    }

    #[tokio::test]
    async fn authorization_code_exchange_installs_one_revision() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/token", post(exchanged_token_response)),
            )
            .await
            .unwrap();
        });

        let mut config = test_config();
        config.token_url = format!("http://{addr}/token");
        let client = OAuth2Client::new(config).unwrap();
        let flow = client.begin_authorization();
        let state = flow.csrf_token.secret().clone();
        let mut revisions = client.subscribe_token_revisions();

        let token = client
            .exchange_code(flow, "code".to_owned(), state)
            .await
            .unwrap();

        revisions.changed().await.unwrap();
        assert_eq!(*revisions.borrow_and_update(), 1);
        assert_eq!(token.access_token, "exchanged-access");
        assert_eq!(
            client
                .current_token()
                .await
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("exchanged-refresh")
        );
        server.abort();
    }

    #[test]
    fn oauth_http_client_accepts_loopback_and_remote_http_urls() {
        for url in [
            "http://localhost",
            "https://LOCALHOST/token",
            "http://127.11.12.13:8080/token",
            "https://[::1]/token",
            "https://example.com/token",
        ] {
            oauth_http_client(url).expect(url);
        }
    }

    #[test]
    fn new_rejects_plain_http_remote_urls() {
        let config = OAuth2Config {
            client_id: "client".to_string(),
            client_secret: None,
            auth_url: "http://auth.example.com/authorize".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            redirect_url: "http://127.0.0.1:8080/callback".to_string(),
            resource: "https://mcp.example.com".to_string(),
            scopes: vec![],
        };

        let err = OAuth2Client::new(config).map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("auth URL"));
    }

    #[test]
    fn callback_query_surfaces_oauth_errors() {
        let err = parse_callback_query(Some(
            "error=access_denied&error_description=User%20cancelled",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("access_denied"));
        assert!(err.to_string().contains("User cancelled"));
    }
}
