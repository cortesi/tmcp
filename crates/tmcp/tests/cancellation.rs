//! Request cancellation integration tests.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use tmcp::{
        Result, Server, ServerCtx, ServerHandle, ServerHandler, schema, testutils::make_duplex_pair,
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
        sync::Notify,
        time::{Duration, timeout},
    };

    #[derive(Clone)]
    struct CancellationServer {
        observed: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl ServerHandler for CancellationServer {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: String,
            _capabilities: schema::ClientCapabilities,
            _client_info: schema::Implementation,
        ) -> Result<schema::InitializeResult> {
            Ok(schema::InitializeResult::new("cancellation-server").with_tools(false))
        }

        async fn call_tool(
            &self,
            ctx: &ServerCtx,
            _name: String,
            _arguments: Option<tmcp::Arguments>,
            _task: Option<schema::TaskMetadata>,
        ) -> Result<schema::CallToolResult> {
            ctx.cancelled().await;
            self.observed.notify_one();
            self.release.notified().await;
            Ok(schema::CallToolResult::new().with_text_content("cancelled"))
        }
    }

    #[tokio::test]
    async fn cancelled_request_does_not_emit_late_response() {
        let observed = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler = CancellationServer {
            observed: Arc::clone(&observed),
            release: Arc::clone(&release),
        };
        let server = Server::new(move || handler.clone());
        let (server_reader, server_writer, client_reader, mut client_writer) = make_duplex_pair();
        let mut client_reader = BufReader::new(client_reader).lines();

        let server_handle = ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .expect("start server");

        write_message(
            &mut client_writer,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "1.0.0" }
                }
            }),
        )
        .await;
        client_reader
            .next_line()
            .await
            .expect("read init response")
            .expect("init response line");

        write_message(
            &mut client_writer,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "wait_for_cancel",
                    "arguments": {}
                }
            }),
        )
        .await;
        write_message(
            &mut client_writer,
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {
                    "requestId": 2,
                    "reason": "test"
                }
            }),
        )
        .await;

        timeout(Duration::from_secs(2), observed.notified())
            .await
            .expect("handler did not observe cancellation");
        release.notify_one();
        let response = timeout(Duration::from_millis(200), client_reader.next_line()).await;
        assert!(response.is_err(), "cancelled request emitted {response:?}");

        server_handle.stop().await.expect("stop server");
    }

    async fn write_message<W>(writer: &mut W, value: serde_json::Value)
    where
        W: AsyncWrite + Unpin,
    {
        let line = serde_json::to_string(&value).expect("serialize message");
        writer
            .write_all(line.as_bytes())
            .await
            .expect("write message");
        writer.write_all(b"\n").await.expect("write newline");
    }
}
