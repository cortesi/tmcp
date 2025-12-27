//! Test utilities for `tmcp`.
//!
//! This module aggregates the helper types and functions that are useful when
//! writing unit and integration tests against this crate. Everything is kept
//! behind the `testutils` module so that the public API surface of the crate
//! remains clean while still making the helpers available to *external* test
//! crates via `use tmcp::testutils::*`.
//!
//! The intent is **not** to provide a full-blown test framework but rather to
//! centralise the small bits of boiler-plate that were previously copied into
//! each individual test file (creation of in-emory duplex streams, sending
//! and receiving newline-delimited JSON-RPC messages, spinning up an in-process
//! server, …). Centralising this logic makes the tests shorter, avoids subtle
//! divergences, and gives downstream users example code they can re-use in
//! their own test suites.

use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    sync::broadcast,
};

use crate::{
    Client, ClientCtx, ClientHandler, Server, ServerCtx, ServerHandle, ServerHandler,
    error::Result,
    schema::{ClientNotification, ServerNotification},
};

/// Conveniently create **two** independent in-memory duplex pipes that together
/// form a bidirectional channel suitable for wiring up a test client and
/// server.
///
/// The return value is laid out so that the first two elements can be given to
/// the server (`reader`, `writer`) and the remaining pair to the client. The
/// exact concrete stream types are hidden behind `impl Trait` so that callers
/// don't have to rely on the *exact* type (`tokio::io::DuplexStream`).
pub fn make_duplex_pair() -> (
    impl AsyncRead + Send + Sync + Unpin + 'static,
    impl AsyncWrite + Send + Sync + Unpin + 'static,
    impl AsyncRead + Send + Sync + Unpin + 'static,
    impl AsyncWrite + Send + Sync + Unpin + 'static,
) {
    // 8 KiB buffer on each side – more than enough for the very small test
    // messages we send around.
    let (server_reader, client_writer) = io::duplex(8 * 1024);
    let (client_reader, server_writer) = io::duplex(8 * 1024);
    (server_reader, server_writer, client_reader, client_writer)
}

/// Spin up an in-memory server using the supplied handler factory
/// and establish a connected [`Client`] without running initialization.
///
/// The helper takes care of wiring up the in-memory transport and saves the
/// caller from having to remember the exact incantations required to start the
/// server in the background.
pub async fn connected_client_and_server<F>(
    handler_factory: F,
) -> Result<(Client<()>, ServerHandle)>
where
    F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
{
    // Build server.
    let server = Server::from_factory(handler_factory);

    // Two in-memory pipes to serve as the transport.
    let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();

    // Start server.
    let server_handle = ServerHandle::from_stream(server, server_reader, server_writer).await?;

    // Build client instance.
    let mut client = Client::new("test-client", "1.0.0");

    // Connect the client to its side of the in-memory transport.
    client
        .connect_stream_raw(client_reader, client_writer)
        .await?;

    Ok((client, server_handle))
}

/// Helper function to create a connected client and server with a custom client handler.
/// The client transport is connected but not initialized.
pub async fn connected_client_and_server_with_conn<F, C>(
    handler_factory: F,
    client_handler: C,
) -> Result<(Client<C>, ServerHandle)>
where
    F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
    C: ClientHandler + 'static,
{
    // Build server.
    let server = Server::from_factory(handler_factory);

    // Two in-memory pipes to serve as the transport.
    let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();

    // Start server.
    let server_handle = ServerHandle::from_stream(server, server_reader, server_writer).await?;

    // Build client instance.
    let mut client = Client::new("test-client", "1.0.0").with_handler(client_handler);

    // Connect the client to its side of the in-memory transport.
    client
        .connect_stream_raw(client_reader, client_writer)
        .await?;

    Ok((client, server_handle))
}

/// Gracefully shut down a client–server pair previously created with
/// [`connected_client_and_server`]. The helper first drops the client so that
/// the underlying transport is closed and then waits (with a short timeout) for
/// the server task to notice the closed connection and terminate.
pub async fn shutdown_client_and_server<C>(client: Client<C>, server: ServerHandle)
where
    C: ClientHandler + 'static,
{
    use tokio::time::{Duration, timeout};

    // Explicitly drop so that the transport is closed *before* we await the
    // server shutdown.
    drop(client);

    timeout(Duration::from_millis(10), server.stop()).await.ok();
}

/// Create a ServerCtx for testing purposes.
/// This creates a ServerCtx with only notification capability (no request/response).
pub fn test_server_ctx(notification_tx: broadcast::Sender<ServerNotification>) -> ServerCtx {
    ServerCtx::new(notification_tx, None)
}

/// Create a ClientCtx for testing purposes.
/// This creates a ClientCtx with only notification capability (no request/response).
pub fn test_client_ctx(notification_tx: broadcast::Sender<ClientNotification>) -> ClientCtx {
    ClientCtx::new(notification_tx)
}

/// Test context for [`ServerHandler`] implementations.
///
/// Provides a [`ServerCtx`] and channels for testing.
pub struct TestServerContext {
    /// Server context for tests.
    ctx: ServerCtx,
    /// Receiver for server notifications.
    notification_rx: broadcast::Receiver<ServerNotification>,
}

impl TestServerContext {
    /// Create a new test server context with notification channels
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = broadcast::channel(100);
        let ctx = test_server_ctx(notification_tx);
        Self {
            ctx,
            notification_rx,
        }
    }

    /// Get a reference to the ServerCtx
    pub fn ctx(&self) -> &ServerCtx {
        &self.ctx
    }

    /// Try to receive a notification, returning None if no notification is available
    pub async fn try_recv_notification(&mut self) -> Option<ServerNotification> {
        use tokio::time::{Duration, timeout};
        timeout(Duration::from_millis(10), self.notification_rx.recv())
            .await
            .ok()
            .and_then(|result| result.ok())
    }
}

impl Default for TestServerContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Test context for [`ClientHandler`] implementations.
///
/// Provides a [`ClientCtx`] and channels for testing.
pub struct TestClientContext {
    /// Client context for tests.
    ctx: ClientCtx,
    /// Receiver for client notifications.
    notification_rx: broadcast::Receiver<ClientNotification>,
}

impl TestClientContext {
    /// Create a new test client context with notification channels
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = broadcast::channel(100);
        let ctx = test_client_ctx(notification_tx);
        Self {
            ctx,
            notification_rx,
        }
    }

    /// Get a reference to the ClientCtx
    pub fn ctx(&self) -> &ClientCtx {
        &self.ctx
    }

    /// Try to receive a notification, returning None if no notification is available
    pub async fn try_recv_notification(&mut self) -> Option<ClientNotification> {
        use tokio::time::{Duration, timeout};
        timeout(Duration::from_millis(10), self.notification_rx.recv())
            .await
            .ok()
            .and_then(|result| result.ok())
    }
}

impl Default for TestClientContext {
    fn default() -> Self {
        Self::new()
    }
}
