//! Integration test for notification meta propagation.

#![allow(missing_docs)]
#![allow(clippy::tests_outside_test_module)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tmcp::{
    Result, Server, ServerCtx, ServerHandle, ServerHandler, schema, testutils::make_duplex_pair,
};
use tokio::{
    io::AsyncWriteExt,
    sync::oneshot,
    time::{Duration, timeout},
};

#[derive(Clone)]
struct MetaRecorder {
    tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait]
impl ServerHandler for MetaRecorder {
    async fn notification(
        &self,
        _context: &ServerCtx,
        notification: schema::ClientNotification,
    ) -> Result<()> {
        if let schema::ClientNotification::Initialized { _meta } = notification
            && let Some(meta) = _meta
            && meta.contains_key("test_key")
            && let Some(tx) = self.tx.lock().unwrap().take()
        {
            tx.send(()).ok();
        }
        Ok(())
    }

    async fn initialize(
        &self,
        _ctx: &ServerCtx,
        _protocol_version: String,
        _capabilities: schema::ClientCapabilities,
        _client_info: schema::Implementation,
    ) -> Result<schema::InitializeResult> {
        Ok(schema::InitializeResult::new("meta-server"))
    }
}

#[tokio::test]
async fn test_notification_meta_propagation() {
    let (tx_success, rx_success) = oneshot::channel::<()>();

    let handler = MetaRecorder {
        tx: Arc::new(Mutex::new(Some(tx_success))),
    };

    let server = Server::new(move || handler.clone());
    let (server_reader, server_writer, _client_reader, mut client_writer) = make_duplex_pair();

    let server_handle = ServerHandle::from_stream(server, server_reader, server_writer)
        .await
        .expect("Failed to start server");

    // Manually send JSON-RPC notification with _meta

    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1.0" }
        }
    });

    let req_str = serde_json::to_string(&init_req).unwrap();
    client_writer.write_all(req_str.as_bytes()).await.unwrap();
    client_writer.write_all(b"\n").await.unwrap();

    // Now send notification with _meta
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {
            "_meta": { "test_key": "found" }
        }
    });

    let notif_str = serde_json::to_string(&notification).unwrap();
    client_writer.write_all(notif_str.as_bytes()).await.unwrap();
    client_writer.write_all(b"\n").await.unwrap();

    // Wait for the server to receive and verify meta
    let result = timeout(Duration::from_secs(2), rx_success).await;

    assert!(
        result.is_ok(),
        "Timeout waiting for notification with meta. This implies _meta was lost or not processed."
    );

    // Cleanup
    server_handle.stop().await.ok();
}
