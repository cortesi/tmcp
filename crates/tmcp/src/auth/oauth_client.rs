use std::{future::Future, sync::Arc, time::Instant};

use axum::{Router, extract::Query, response::Html, routing::get};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, StandardRevocableToken,
    TokenResponse, TokenUrl,
    basic::{
        BasicClient, BasicErrorResponse, BasicRevocationErrorResponse,
        BasicTokenIntrospectionResponse, BasicTokenResponse,
    },
};
use serde::Deserialize;
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock, oneshot},
};
use url::Url;

use super::dynamic_registration::{ClientMetadata, DynamicRegistrationClient};
use crate::error::Error;

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

#[derive(Debug, Clone)]
/// OAuth2 token information.
pub struct OAuth2Token {
    /// Access token value.
    pub access_token: String,
    /// Optional refresh token value.
    pub refresh_token: Option<String>,
    /// Optional expiration instant.
    pub expires_at: Option<Instant>,
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

/// OAuth2 client with optional token caching and refresh support.
pub struct OAuth2Client {
    /// OAuth2 client used for token flows.
    client: ConfiguredClient,
    /// Configuration used to build the OAuth2 client.
    config: OAuth2Config,
    /// Cached token stored behind a lock.
    token: Arc<RwLock<Option<OAuth2Token>>>,
    /// Mutex to serialize refreshes.
    refresh_lock: Arc<Mutex<()>>,
    /// PKCE verifier captured during authorization.
    pkce_verifier: Option<PkceCodeVerifier>,
    /// CSRF token captured during authorization.
    csrf_token: Option<CsrfToken>,
}

impl OAuth2Client {
    /// Perform dynamic client registration and create an OAuth2Client
    pub async fn register_dynamic(
        auth_url: String,
        token_url: String,
        resource: String,
        client_name: String,
        redirect_url: String,
        scopes: Vec<String>,
        registration_endpoint: Option<String>,
    ) -> Result<Self, Error> {
        let registration_client = DynamicRegistrationClient::new();

        // Try to discover registration endpoint if not provided
        let reg_endpoint = if let Some(endpoint) = registration_endpoint {
            endpoint
        } else {
            // Extract issuer from auth URL
            let auth_url_parsed = Url::parse(&auth_url)
                .map_err(|e| Error::InvalidConfiguration(format!("Invalid auth URL: {e}")))?;
            let issuer = format!(
                "{}://{}",
                auth_url_parsed.scheme(),
                auth_url_parsed.host_str().unwrap_or("")
            );

            // Try to discover registration endpoint
            match registration_client
                .discover_registration_endpoint(&issuer)
                .await?
            {
                Some(endpoint) => endpoint,
                None => {
                    return Err(Error::InvalidConfiguration(
                        "No registration endpoint found in OAuth metadata".to_string(),
                    ));
                }
            }
        };

        // Create client metadata
        let metadata = ClientMetadata::new(&client_name, &redirect_url)
            .with_resource(&resource)
            .with_scopes(&scopes)
            .with_software_info("tmcp", env!("CARGO_PKG_VERSION"));

        // Register the client
        let registration = registration_client
            .register(&reg_endpoint, metadata, None)
            .await?;

        // Create OAuth2Config from registration response
        let config = OAuth2Config::from_registration(registration, auth_url, token_url, resource);

        // Create the OAuth2Client
        Self::new(config)
    }

    /// Create a new OAuth2 client from configuration.
    pub fn new(config: OAuth2Config) -> Result<Self, Error> {
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

        // For now, we don't set revocation URL during construction to maintain type compatibility
        // The revocation URL can be set later if needed

        Ok(Self {
            client,
            config,
            token: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
            pkce_verifier: None,
            csrf_token: None,
        })
    }

    /// Build an authorization URL and return the CSRF token.
    pub fn get_authorization_url(&mut self) -> (Url, CsrfToken) {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        self.pkce_verifier = Some(pkce_verifier);

        let mut auth_request = self
            .client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge)
            .add_extra_param("resource", &self.config.resource);

        for scope in &self.config.scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.clone()));
        }

        let (url, csrf_token) = auth_request.url();
        self.csrf_token = Some(csrf_token.clone());
        (url, csrf_token)
    }

    /// Exchange an authorization code for a token.
    pub async fn exchange_code(
        &mut self,
        code: String,
        state: String,
    ) -> Result<OAuth2Token, Error> {
        let stored_token = self
            .csrf_token
            .take()
            .ok_or_else(|| Error::InvalidConfiguration("Missing CSRF token".to_string()))?;

        if state != *stored_token.secret() {
            return Err(Error::AuthorizationFailed("CSRF token mismatch".into()));
        }

        let pkce_verifier = self
            .pkce_verifier
            .take()
            .ok_or_else(|| Error::InvalidConfiguration("Missing PKCE verifier".to_string()))?;

        let mut token_request = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier);

        // Only add resource parameter if it's not empty (some providers don't support it)
        if !self.config.resource.is_empty() {
            token_request = token_request.add_extra_param("resource", &self.config.resource);
        }

        let token_result = token_request
            .request_async(&reqwest::Client::new())
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

        let oauth_token = OAuth2Token {
            access_token: token_result.access_token().secret().clone(),
            refresh_token: token_result.refresh_token().map(|t| t.secret().clone()),
            expires_at: token_result
                .expires_in()
                .map(|duration| Instant::now() + duration),
        };

        *self.token.write().await = Some(oauth_token.clone());
        Ok(oauth_token)
    }

    /// Retrieve a valid access token, refreshing if necessary.
    pub async fn get_valid_token(&self) -> Result<String, Error> {
        let now = Instant::now();
        {
            let token_guard = self.token.read().await;
            if let Some(token) = &*token_guard
                && token.expires_at.map(|exp| exp > now).unwrap_or(true)
            {
                return Ok(token.access_token.clone());
            }
        }

        let _refresh_guard = self.refresh_lock.lock().await;

        // Check again after obtaining the lock in case another task refreshed
        let refresh_token_opt = {
            let token_guard = self.token.read().await;
            if let Some(token) = &*token_guard {
                if token.expires_at.map(|exp| exp > now).unwrap_or(true) {
                    return Ok(token.access_token.clone());
                }
                token.refresh_token.clone()
            } else {
                None
            }
        };

        if let Some(refresh_token) = refresh_token_opt {
            self.refresh_token_inner(&refresh_token).await
        } else {
            Err(Error::AuthorizationFailed(
                "No valid token available".to_string(),
            ))
        }
    }
    /// Refresh the access token using a refresh token.
    async fn refresh_token_inner(&self, refresh_token: &str) -> Result<String, Error> {
        let refresh_token_obj = RefreshToken::new(refresh_token.to_string());
        let mut refresh_request = self.client.exchange_refresh_token(&refresh_token_obj);

        // Only add resource parameter if it's not empty (some providers don't support it)
        if !self.config.resource.is_empty() {
            refresh_request = refresh_request.add_extra_param("resource", &self.config.resource);
        }

        let token_result = refresh_request
            .request_async(&reqwest::Client::new())
            .await
            .map_err(|e| Error::AuthorizationFailed(format!("Token refresh failed: {e}")))?;

        let oauth_token = OAuth2Token {
            access_token: token_result.access_token().secret().clone(),
            refresh_token: token_result.refresh_token().map(|t| t.secret().clone()),
            expires_at: token_result
                .expires_in()
                .map(|duration| Instant::now() + duration),
        };

        let access_token = oauth_token.access_token.clone();
        *self.token.write().await = Some(oauth_token);
        Ok(access_token)
    }

    /// Set the current token in the cache.
    pub fn set_token(&self, token: OAuth2Token) -> impl Future<Output = ()> + Send {
        let token_arc = self.token.clone();
        async move {
            *token_arc.write().await = Some(token);
        }
    }
}

#[derive(Debug, Deserialize)]
/// Query parameters provided to the OAuth callback endpoint.
struct CallbackQuery {
    /// Authorization code returned by the provider.
    code: String,
    /// CSRF state value returned by the provider.
    state: String,
}

/// Minimal HTTP callback server for OAuth redirects.
pub struct OAuth2CallbackServer {
    /// Port to bind the callback server on.
    port: u16,
}

impl OAuth2CallbackServer {
    /// Create a new callback server bound to the provided port.
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Wait for the OAuth redirect callback and return (code, state).
    pub async fn wait_for_callback(&self) -> Result<(String, String), Error> {
        let (tx, rx) = oneshot::channel::<(String, String)>();
        let tx = Arc::new(Mutex::new(Some(tx)));

        let callback_handler = {
            let tx = tx.clone();
            move |Query(params): Query<CallbackQuery>| {
                let tx = tx.clone();
                async move {
                    if let Some(sender) = tx.lock().await.take() {
                        sender.send((params.code, params.state)).ok();
                    }
                    Html(SUCCESS_HTML)
                }
            }
        };

        let app = Router::new().route("/callback", get(callback_handler));

        let addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| Error::TransportError(format!("Failed to bind to {addr}: {e}")))?;

        let server = axum::serve(listener, app);

        let server_handle = tokio::spawn(async move {
            server.await.ok();
        });

        let result = rx.await.map_err(|_| {
            Error::AuthorizationFailed("Callback server closed unexpectedly".into())
        })?;

        server_handle.abort();

        Ok(result)
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
