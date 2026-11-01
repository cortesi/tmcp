//! The extension of [`Client`] that connects over the streamable HTTP
//! transport.

use std::sync::Arc;

use tmcp::{Client, ClientHandler, Result, schema::InitializeResult};

use crate::{auth::OAuth2Client, http::HttpClientTransport};

/// Connects a [`Client`] over HTTP or HTTPS, and initializes the connection.
///
/// Each `endpoint` is a server URL with its protocol and path, such as
/// `http://localhost:3000` or `https://api.example.com/mcp`.
pub trait ClientHttpExt {
    /// Connect via HTTP or HTTPS and initialize the connection.
    fn connect_http(
        &mut self,
        endpoint: impl Into<String> + Send,
    ) -> impl Future<Output = Result<InitializeResult>> + Send;

    /// Connect via HTTP or HTTPS with static headers and initialize the
    /// connection.
    fn connect_http_with_headers(
        &mut self,
        endpoint: impl Into<String> + Send,
        headers: http::HeaderMap,
    ) -> impl Future<Output = Result<InitializeResult>> + Send;

    /// Connect via HTTP or HTTPS with OAuth authentication and initialize the
    /// connection. The OAuth client holds valid tokens, or is ready to run
    /// the OAuth flow.
    fn connect_http_with_oauth(
        &mut self,
        endpoint: impl Into<String> + Send,
        oauth_client: Arc<OAuth2Client>,
    ) -> impl Future<Output = Result<InitializeResult>> + Send;

    /// Connect via HTTP or HTTPS with OAuth authentication plus static
    /// headers.
    fn connect_http_with_oauth_and_headers(
        &mut self,
        endpoint: impl Into<String> + Send,
        oauth_client: Arc<OAuth2Client>,
        headers: http::HeaderMap,
    ) -> impl Future<Output = Result<InitializeResult>> + Send;
}

impl<C> ClientHttpExt for Client<C>
where
    C: ClientHandler + Send + 'static,
{
    async fn connect_http(
        &mut self,
        endpoint: impl Into<String> + Send,
    ) -> Result<InitializeResult> {
        self.connect(Box::new(HttpClientTransport::new(endpoint)))
            .await?;
        self.init().await
    }

    async fn connect_http_with_headers(
        &mut self,
        endpoint: impl Into<String> + Send,
        headers: http::HeaderMap,
    ) -> Result<InitializeResult> {
        let transport = HttpClientTransport::new(endpoint).with_static_headers(headers);
        self.connect(Box::new(transport)).await?;
        self.init().await
    }

    async fn connect_http_with_oauth(
        &mut self,
        endpoint: impl Into<String> + Send,
        oauth_client: Arc<OAuth2Client>,
    ) -> Result<InitializeResult> {
        let transport = HttpClientTransport::new(endpoint).with_oauth(oauth_client);
        self.connect(Box::new(transport)).await?;
        self.init().await
    }

    async fn connect_http_with_oauth_and_headers(
        &mut self,
        endpoint: impl Into<String> + Send,
        oauth_client: Arc<OAuth2Client>,
        headers: http::HeaderMap,
    ) -> Result<InitializeResult> {
        let transport = HttpClientTransport::new(endpoint)
            .with_static_headers(headers)
            .with_oauth(oauth_client);
        self.connect(Box::new(transport)).await?;
        self.init().await
    }
}
