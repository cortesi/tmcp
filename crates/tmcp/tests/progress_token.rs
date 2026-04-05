//! Integration tests for request progress token propagation.

#![allow(missing_docs)]
#![allow(clippy::tests_outside_test_module)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tmcp::{
    Result, Server, ServerCtx, ServerHandle, ServerHandler, schema,
    testutils::{TestServerContext, make_duplex_pair},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
    time::{Duration, timeout},
};

#[tokio::test]
async fn send_progress_uses_request_token_and_monotonic_counter() {
    let mut test_ctx = TestServerContext::new();
    let ctx = test_ctx
        .ctx()
        .clone()
        .with_progress_token(schema::ProgressToken::String("progress-1".to_owned()));

    ctx.send_progress("first");
    ctx.send_progress("second");

    let first = test_ctx
        .try_recv_notification()
        .await
        .expect("first notification");
    match first {
        schema::ServerNotification::Progress {
            progress_token,
            progress,
            total,
            message,
            _meta: _,
        } => {
            assert!(matches!(
                progress_token,
                schema::ProgressToken::String(token) if token == "progress-1"
            ));
            assert_eq!(progress, 1.0);
            assert_eq!(total, None);
            assert_eq!(message.as_deref(), Some("first"));
        }
        other => panic!("expected progress notification, got {other:?}"),
    }

    let second = test_ctx
        .try_recv_notification()
        .await
        .expect("second notification");
    match second {
        schema::ServerNotification::Progress {
            progress_token,
            progress,
            total,
            message,
            _meta: _,
        } => {
            assert!(matches!(
                progress_token,
                schema::ProgressToken::String(token) if token == "progress-1"
            ));
            assert_eq!(progress, 2.0);
            assert_eq!(total, None);
            assert_eq!(message.as_deref(), Some("second"));
        }
        other => panic!("expected progress notification, got {other:?}"),
    }
}

#[tokio::test]
async fn send_progress_is_noop_without_token() {
    let mut test_ctx = TestServerContext::new();
    test_ctx.ctx().send_progress("ignored");
    assert!(
        test_ctx.try_recv_notification().await.is_none(),
        "progress without token should not enqueue notifications"
    );
}

#[derive(Clone)]
struct ProgressTokenRecorder {
    tx: Arc<Mutex<Option<oneshot::Sender<schema::ProgressToken>>>>,
}

#[async_trait]
impl ServerHandler for ProgressTokenRecorder {
    async fn initialize(
        &self,
        _ctx: &ServerCtx,
        _protocol_version: String,
        _capabilities: schema::ClientCapabilities,
        _client_info: schema::Implementation,
    ) -> Result<schema::InitializeResult> {
        Ok(schema::InitializeResult::new("progress-token-server"))
    }

    async fn call_tool(
        &self,
        ctx: &ServerCtx,
        _name: String,
        _arguments: Option<tmcp::Arguments>,
        _task: Option<schema::TaskMetadata>,
    ) -> Result<schema::CallToolResult> {
        if let Some(token) = ctx.progress_token().cloned()
            && let Some(tx) = self.tx.lock().expect("token recorder lock").take()
        {
            tx.send(token).ok();
        }
        Ok(schema::CallToolResult::new().with_text_content("ok"))
    }
}

#[tokio::test]
async fn tools_call_meta_progress_token_reaches_server_context() {
    let (tx_token, rx_token) = oneshot::channel::<schema::ProgressToken>();
    let handler = ProgressTokenRecorder {
        tx: Arc::new(Mutex::new(Some(tx_token))),
    };

    let server = Server::new(move || handler.clone());
    let (server_reader, server_writer, client_reader, mut client_writer) = make_duplex_pair();
    let mut client_reader = BufReader::new(client_reader).lines();

    let server_handle = ServerHandle::from_stream(server, server_reader, server_writer)
        .await
        .expect("start server");

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0.0" }
        }
    });
    let init_line = serde_json::to_string(&initialize).expect("serialize init");
    client_writer
        .write_all(init_line.as_bytes())
        .await
        .expect("write init");
    client_writer.write_all(b"\n").await.expect("write newline");
    client_reader
        .next_line()
        .await
        .expect("read init response")
        .expect("init response line");

    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "noop",
            "arguments": {},
            "_meta": {
                "progressToken": "progress-token-42"
            }
        }
    });
    let call_line = serde_json::to_string(&call).expect("serialize call");
    client_writer
        .write_all(call_line.as_bytes())
        .await
        .expect("write call");
    client_writer.write_all(b"\n").await.expect("write newline");

    let observed = timeout(Duration::from_secs(2), rx_token)
        .await
        .expect("timed out waiting for token")
        .expect("token sender dropped");

    match observed {
        schema::ProgressToken::String(token) => assert_eq!(token, "progress-token-42"),
        schema::ProgressToken::Number(value) => {
            panic!("expected string progress token, got numeric token {value}")
        }
    }

    server_handle.stop().await.ok();
}
