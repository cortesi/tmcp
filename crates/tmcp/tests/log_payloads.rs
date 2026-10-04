//! Transport logs hold message metadata and never a payload.
//!
//! This binary holds one test, because it installs the global subscriber.

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use tmcp::{
        Arguments, Result, ServerCtx, ServerHandler,
        schema::{self, ClientCapabilities, Implementation, InitializeResult, ProtocolVersion},
        testutils::{connected_client_and_server, shutdown_client_and_server},
    };

    const SENTINEL: &str = "sentinel-credential-value";

    /// A writer that keeps each log line for inspection.
    #[derive(Clone, Default)]
    struct CapturedLog(Arc<Mutex<Vec<u8>>>);

    impl io::Write for CapturedLog {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A server whose one tool returns the sentinel.
    struct EchoServer;

    #[async_trait]
    impl ServerHandler for EchoServer {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("echo-server"))
        }

        async fn call_tool(
            &self,
            _ctx: &ServerCtx,
            _name: String,
            _arguments: Option<Arguments>,
            _task: Option<schema::TaskMetadata>,
        ) -> Result<schema::CallToolResponse> {
            Ok(schema::CallToolResponse::result(
                schema::CallToolResult::new().with_text_content(SENTINEL),
            ))
        }
    }

    #[tokio::test]
    async fn transport_logs_hold_no_payload() {
        let log = CapturedLog::default();
        let writer = log.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .init();

        let (client, server) = connected_client_and_server(|| EchoServer)
            .await
            .expect("setup");
        let result = client
            .call_tool("echo", serde_json::json!({ "value": SENTINEL }))
            .await
            .expect("call");
        assert!(format!("{result:?}").contains(SENTINEL));
        shutdown_client_and_server(client, server).await;

        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("\"tools/call\""), "{text}");
        assert!(text.contains("Encoded JSON-RPC response"), "{text}");
        assert!(text.contains("Decoded JSON-RPC response"), "{text}");
        assert!(!text.contains(SENTINEL), "{text}");
    }
}
