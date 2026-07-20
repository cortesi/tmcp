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

use serde_json::Value;
use tokio::{
    io::{self, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines},
    sync::mpsc,
    time::{Duration, timeout},
};

use crate::{
    Client, ClientCtx, ClientHandler, Server, ServerCtx, ServerHandle, ServerHandler,
    error::Result,
    schema::{ClientNotification, ServerNotification},
};

/// Queue size for test-only notification channels.
const TEST_NOTIFICATION_BUFFER: usize = 16;

/// Upper bound on how long [`WireConnection::recv`] waits for a message before
/// failing the test.
const WIRE_RECV_TIMEOUT: Duration = Duration::from_secs(5);

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
pub async fn connected_client_and_server<H, F>(
    handler_factory: F,
) -> Result<(Client<()>, ServerHandle)>
where
    H: ServerHandler + 'static,
    F: Fn() -> H + Send + Sync + 'static,
{
    // Build server.
    let server = Server::new(handler_factory);

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
pub async fn connected_client_and_server_with_conn<H, F, C>(
    handler_factory: F,
    client_handler: C,
) -> Result<(Client<C>, ServerHandle)>
where
    H: ServerHandler + 'static,
    F: Fn() -> H + Send + Sync + 'static,
    C: ClientHandler + 'static,
{
    // Build server.
    let server = Server::new(handler_factory);

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

/// A raw JSON-RPC wire connection to an in-process server.
///
/// Unlike [`connected_client_and_server`], this bypasses the [`Client`]
/// machinery entirely: messages are written and read as raw newline-delimited
/// JSON, which lets tests assert the exact wire-level protocol a server
/// produces.
pub struct WireConnection {
    /// Handle to the running server, exposed so tests can stop it or inspect
    /// captured capabilities.
    pub server: ServerHandle,
    /// Write half of the client side of the transport.
    writer: io::DuplexStream,
    /// Buffered line reader over the server's output.
    lines: Lines<BufReader<io::DuplexStream>>,
}

impl WireConnection {
    /// Start a server from `handler_factory` on an in-memory transport and
    /// return a raw wire connection to it.
    pub async fn start<H, F>(handler_factory: F) -> Result<Self>
    where
        H: ServerHandler + 'static,
        F: Fn() -> H + Send + Sync + 'static,
    {
        let server = Server::new(handler_factory);
        let (server_reader, client_writer) = io::duplex(64 * 1024);
        let (client_reader, server_writer) = io::duplex(64 * 1024);
        let server = ServerHandle::from_stream(server, server_reader, server_writer).await?;
        Ok(Self {
            server,
            writer: client_writer,
            lines: BufReader::new(client_reader).lines(),
        })
    }

    /// Send one raw JSON-RPC message as a newline-delimited JSON line.
    pub async fn send(&mut self, message: &Value) {
        let line = serde_json::to_string(message).expect("serialize wire message");
        self.send_raw(&line).await;
    }

    /// Send one raw line verbatim, for tests that exercise malformed input.
    pub async fn send_raw(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write wire message");
        self.writer
            .write_all(b"\n")
            .await
            .expect("write wire newline");
    }

    /// Receive the next message sent by the server.
    ///
    /// Panics if the connection closes or no message arrives within
    /// [`WIRE_RECV_TIMEOUT`].
    pub async fn recv(&mut self) -> Value {
        let line = timeout(WIRE_RECV_TIMEOUT, self.lines.next_line())
            .await
            .expect("timed out waiting for wire message")
            .expect("read wire message")
            .expect("connection closed while waiting for wire message");
        serde_json::from_str(&line).expect("parse wire message")
    }
}

/// Run a JSON fixture of request/response steps against a server.
///
/// The fixture is a JSON array of steps. Each step must contain a `send`
/// message, which is written to the server verbatim, and may contain an
/// `expect` message, which is compared structurally against the next message
/// the server sends. Steps without `expect` (notifications) produce no read.
pub async fn run_wire_fixture<H, F>(handler_factory: F, fixture: &str)
where
    H: ServerHandler + 'static,
    F: Fn() -> H + Send + Sync + 'static,
{
    let steps: Vec<Value> = serde_json::from_str(fixture).expect("parse wire fixture");
    let mut conn = WireConnection::start(handler_factory)
        .await
        .expect("start wire server");
    for (index, step) in steps.iter().enumerate() {
        let step = step.as_object().expect("fixture step must be an object");
        let send = step.get("send").expect("fixture step missing `send`");
        conn.send(send).await;
        if let Some(expected) = step.get("expect") {
            let actual = conn.recv().await;
            assert_eq!(
                &actual, expected,
                "wire fixture step {index} mismatch\nsent:     {send}\nexpected: {expected:#}\nactual:   {actual:#}"
            );
        }
    }
}

/// Create a ServerCtx for testing purposes.
/// This creates a ServerCtx with only notification capability (no request/response).
pub fn test_server_ctx(notification_tx: mpsc::Sender<ServerNotification>) -> ServerCtx {
    ServerCtx::notification_only(notification_tx)
}

/// Create a ClientCtx for testing purposes.
/// This creates a ClientCtx with only notification capability (no request/response).
pub fn test_client_ctx(notification_tx: mpsc::Sender<ClientNotification>) -> ClientCtx {
    ClientCtx::new(notification_tx)
}

/// Test context for [`ServerHandler`] implementations.
///
/// Provides a [`ServerCtx`] and channels for testing.
pub struct TestServerContext {
    /// Server context for tests.
    ctx: ServerCtx,
    /// Receiver for server notifications.
    notification_rx: mpsc::Receiver<ServerNotification>,
}

impl TestServerContext {
    /// Create a new test server context with notification channels
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(TEST_NOTIFICATION_BUFFER);
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
            .flatten()
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
    notification_rx: mpsc::Receiver<ClientNotification>,
}

impl TestClientContext {
    /// Create a new test client context with notification channels
    pub fn new() -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(TEST_NOTIFICATION_BUFFER);
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
            .flatten()
    }
}

impl Default for TestClientContext {
    fn default() -> Self {
        Self::new()
    }
}
