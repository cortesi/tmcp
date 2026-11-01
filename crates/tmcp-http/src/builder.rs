//! The HTTP builder of a server, and the extension of [`Server`] that starts
//! it.

use axum::Router;
use tmcp::{Error, Result, Server, ServerHandle};

use crate::{
    auth::server::{AuthConfig, BearerAuthLayer, protected_resource_handler},
    http::{CorsPolicy, EmbeddedHttpRoutes, HttpServer, normalize_endpoint_path},
};

/// Serves a [`Server`] over the streamable HTTP transport.
pub trait ServerHttpExt {
    /// Configure an HTTP server bound to the provided address.
    ///
    /// Returns an [`HttpBuilder`] for further configuration; call
    /// [`HttpBuilder::serve`] to start serving.
    fn http(self, addr: impl AsRef<str>) -> HttpBuilder;

    /// Configure an HTTP server for embedding into an existing Axum
    /// application.
    fn http_embed(self) -> HttpBuilder;

    /// Serve HTTP connections, and return a [`ServerHandle`] that stops the
    /// server.
    fn serve_http(self, addr: impl AsRef<str>)
    -> impl Future<Output = Result<ServerHandle>> + Send;
}

impl ServerHttpExt for Server {
    fn http(self, addr: impl AsRef<str>) -> HttpBuilder {
        HttpBuilder::new(self, Some(addr.as_ref().to_string()))
    }

    fn http_embed(self) -> HttpBuilder {
        HttpBuilder::new(self, None)
    }

    fn serve_http(
        self,
        addr: impl AsRef<str>,
    ) -> impl Future<Output = Result<ServerHandle>> + Send {
        let builder = self.http(addr);
        builder.serve()
    }
}

/// Builder that configures and serves the HTTP transport.
pub struct HttpBuilder {
    /// Server to serve.
    server: Server,
    /// Bind address for the HTTP listener.
    bind_addr: Option<String>,
    /// Public endpoint path where MCP is served.
    endpoint_path: String,
    /// Transformation applied to the MCP routes after state is attached.
    middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
    /// Additional routes merged outside the middleware scope.
    routes: Option<Router>,
    /// Cross-origin policy for the MCP routes.
    cors: CorsPolicy,
}

/// Result of embedding tmcp HTTP routes into an existing Axum application.
pub struct EmbeddedHttpServer {
    /// Router containing the mounted MCP handlers and any auxiliary routes.
    ///
    /// Merge this router into the host application at the root.
    pub router: Router,
    /// Live tmcp server handle that must be shut down with the host
    /// application.
    pub handle: ServerHandle,
}

impl HttpBuilder {
    /// Create a builder for a server, with an optional bind address.
    fn new(server: Server, bind_addr: Option<String>) -> Self {
        Self {
            server,
            bind_addr,
            endpoint_path: "/".to_string(),
            middleware: None,
            routes: None,
            cors: CorsPolicy::default(),
        }
    }

    /// Override the public endpoint path where the MCP handlers are mounted.
    pub fn with_endpoint_path(mut self, endpoint_path: impl Into<String>) -> Self {
        self.endpoint_path = normalize_endpoint_path(endpoint_path);
        self
    }

    /// Wrap the MCP routes in middleware after state has been attached.
    pub fn with_middleware<G>(mut self, middleware: G) -> Self
    where
        G: FnOnce(Router) -> Router + Send + 'static,
    {
        self.middleware = Some(match self.middleware.take() {
            Some(previous) => Box::new(move |router| middleware(previous(router))),
            None => Box::new(middleware),
        });
        self
    }

    /// Merge additional routes that bypass the configured middleware.
    pub fn with_routes(mut self, routes: Router) -> Self {
        self.routes = Some(match self.routes.take() {
            Some(existing) => existing.merge(routes),
            None => routes,
        });
        self
    }

    /// Set the cross-origin policy for the MCP routes.
    ///
    /// The default is [`CorsPolicy::SameOrigin`], which rejects cross-origin
    /// browser requests and emits no CORS headers.
    pub fn with_cors(mut self, cors: CorsPolicy) -> Self {
        self.cors = cors;
        self
    }

    /// Protect the MCP routes with bearer-token auth and expose PRM discovery
    /// routes.
    pub fn with_auth(self, config: &AuthConfig) -> Self {
        let middleware = BearerAuthLayer::new(config.validator.clone(), &config.endpoint_path)
            .with_base_url(&config.base_url)
            .with_trusted_forwarded_headers(config.trust_forwarded_headers);
        self.with_endpoint_path(&config.endpoint_path)
            .with_middleware(|router| router.layer(middleware))
            .with_routes(protected_resource_handler(config))
    }

    /// Start serving the configured HTTP transport.
    pub async fn serve(self) -> Result<ServerHandle> {
        let bind_addr = self.bind_addr.clone().ok_or_else(|| {
            Error::InvalidConfiguration(
                "HTTP embed builders do not have a bind address; call into_router()".to_string(),
            )
        })?;
        let (factory, protocol_versions) = self.server.into_factory();
        let http = HttpServer::new(factory, protocol_versions, self.cors);
        let EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        } = http.routes(&self.endpoint_path, self.middleware, self.routes);
        let router = mcp_router.merge(aux_routes);

        let (task, bound_addr) = http.listen(&bind_addr, router).await?;
        let mut handle =
            ServerHandle::listener(task, http.shutdown_token(), http.notification_fanout());
        handle.set_http_endpoint(Some(bound_addr), self.endpoint_path);
        Ok(handle)
    }

    /// Build tmcp HTTP routers for embedding into an existing Axum application.
    pub async fn into_router(self) -> Result<EmbeddedHttpServer> {
        let endpoint_path = self.endpoint_path;
        let (factory, protocol_versions) = self.server.into_factory();
        let http = HttpServer::new(factory, protocol_versions, self.cors);
        let EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        } = http.routes("/", self.middleware, self.routes);
        let router = mount_embedded_router(&endpoint_path, mcp_router, aux_routes);

        let shutdown_token = http.shutdown_token();
        let watch_token = shutdown_token.clone();
        let task = tokio::spawn(async move {
            watch_token.cancelled().await;
        });
        let mut handle = ServerHandle::listener(task, shutdown_token, http.notification_fanout());
        handle.set_http_endpoint(None, endpoint_path);
        Ok(EmbeddedHttpServer { router, handle })
    }
}

/// Return an embedded router mounted at the configured endpoint path.
fn mount_embedded_router(endpoint_path: &str, mcp_router: Router, aux_routes: Router) -> Router {
    let endpoint_path = normalize_endpoint_path(endpoint_path);
    if endpoint_path == "/" {
        mcp_router.merge(aux_routes)
    } else {
        Router::new()
            .nest(&endpoint_path, mcp_router)
            .merge(aux_routes)
    }
}
