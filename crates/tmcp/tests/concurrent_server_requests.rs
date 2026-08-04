//! Server-initiated requests must not stall the client's own traffic.
//!
//! Elicitation and sampling can wait on a human or an LLM for a long time;
//! while one is pending, the client's own requests (and responses to them)
//! must keep flowing.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use async_trait::async_trait;
    use tmcp::{
        Arguments, ClientCtx, ClientHandler, Result, ServerCtx, ServerHandler,
        schema::{
            CallToolResponse, CallToolResult, ClientCapabilities, ElicitAction,
            ElicitRequestFormParams, ElicitRequestParams, ElicitResult, ElicitSchema,
            Implementation, InitializeResult, ProtocolVersion, TaskMetadata,
        },
        testutils::connected_client_and_server_with_conn,
    };
    use tokio::{
        sync::Notify,
        time::{Duration, timeout},
    };

    /// Server whose only tool elicits input from the client.
    #[derive(Clone, Default)]
    struct ElicitingServer;

    #[async_trait]
    impl ServerHandler for ElicitingServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("eliciting-server").with_version("1.0.0"))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            _name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResponse> {
            let params = ElicitRequestParams::Form(ElicitRequestFormParams {
                mode: None,
                message: "Need input".to_string(),
                requested_schema: ElicitSchema {
                    schema: None,
                    schema_type: "object".to_string(),
                    properties: HashMap::new(),
                    required: None,
                },
                task: None,
                _meta: None,
            });
            let result = context.elicit(params).await?;
            Ok(CallToolResponse::result(
                CallToolResult::new().with_text_content(format!("{:?}", result.action)),
            ))
        }
    }

    /// Client handler whose elicitation blocks until the test releases it.
    #[derive(Clone)]
    struct BlockingElicitHandler {
        /// Signalled when the elicitation reaches the client handler.
        entered: Arc<Notify>,
        /// Released by the test to let the elicitation complete.
        release: Arc<Notify>,
    }

    #[async_trait]
    impl ClientHandler for BlockingElicitHandler {
        async fn elicit(
            &self,
            _context: &ClientCtx,
            _params: ElicitRequestParams,
        ) -> Result<ElicitResult> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(ElicitResult {
                action: ElicitAction::Accept,
                content: Some(HashMap::new()),
                _meta: None,
                _extra: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn pending_elicitation_does_not_stall_client_requests() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler = BlockingElicitHandler {
            entered: entered.clone(),
            release: release.clone(),
        };

        let (mut client, server) =
            connected_client_and_server_with_conn(|| ElicitingServer, handler)
                .await
                .unwrap();
        client.init().await.unwrap();

        let client = Arc::new(client);
        let tool_client = client.clone();
        let tool_call = tokio::spawn(async move { tool_client.call_tool("ask", ()).await });

        // Wait until the server's elicitation reaches the client handler.
        timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("elicitation never reached the client handler");

        // The elicitation is pending; the client's own requests must proceed.
        timeout(Duration::from_secs(5), client.ping())
            .await
            .expect("ping stalled behind a pending elicitation")
            .unwrap();

        release.notify_one();
        let result = timeout(Duration::from_secs(5), tool_call)
            .await
            .expect("tool call never completed")
            .unwrap()
            .unwrap();
        assert_eq!(result.text(), Some("Accept"));

        drop(client);
        server.stop().await.ok();
    }
}
