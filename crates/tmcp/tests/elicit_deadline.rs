//! The deadline of one elicitation: a server waits for a person longer, or
//! shorter, than the default deadline of the connection.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use async_trait::async_trait;
    use tmcp::{
        Arguments, ClientCtx, ClientHandler, Error, Result, ServerCtx, ServerHandler,
        schema::{
            CallToolResponse, CallToolResult, ClientCapabilities, ElicitAction,
            ElicitRequestFormParams, ElicitRequestParams, ElicitResult, ElicitSchema,
            Implementation, InitializeResult, ProtocolVersion, TaskMetadata,
        },
        testutils::connected_client_and_server_with_conn,
    };
    use tokio::{sync::mpsc, time::sleep};

    /// The time that the person of the test client takes to answer.
    const ANSWER_TIME: Duration = Duration::from_secs(45);

    /// Returns one elicitation with an empty form.
    fn params() -> ElicitRequestParams {
        ElicitRequestParams::Form(ElicitRequestFormParams {
            mode: None,
            message: "Need input".to_owned(),
            requested_schema: ElicitSchema {
                schema: None,
                schema_type: "object".to_owned(),
                properties: HashMap::new(),
                required: None,
            },
            task: None,
            _meta: None,
        })
    }

    /// Returns the deadline of one tool name: the default, or seconds.
    fn deadline(name: &str) -> Option<Duration> {
        name.parse().ok().map(Duration::from_secs)
    }

    /// Server whose tool elicits with the deadline that its name states.
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
            name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResponse> {
            let result = match deadline(&name) {
                Some(deadline) => context.elicit_with_timeout(params(), deadline).await,
                None => context.elicit(params()).await,
            };
            let text = match result {
                Ok(result) => format!("{:?}", result.action),
                Err(error) => error.to_string(),
            };
            Ok(CallToolResponse::result(
                CallToolResult::new().with_text_content(text),
            ))
        }
    }

    /// Client whose person answers each elicitation after [`ANSWER_TIME`].
    #[derive(Clone)]
    struct SlowPerson;

    #[async_trait]
    impl ClientHandler for SlowPerson {
        async fn elicit(
            &self,
            _context: &ClientCtx,
            _params: ElicitRequestParams,
        ) -> Result<ElicitResult> {
            sleep(ANSWER_TIME).await;
            Ok(ElicitResult {
                action: ElicitAction::Accept,
                content: Some(HashMap::new()),
                _meta: None,
                _extra: Default::default(),
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_elicitation_waits_for_its_own_deadline_across_a_transport() {
        let (client, server) =
            connected_client_and_server_with_conn(|| ElicitingServer, SlowPerson)
                .await
                .expect("connected");
        let mut client = client.without_request_timeout();
        client.init().await.expect("initialized");
        for (tool, text) in [
            // The default deadline of the connection ends before the answer.
            ("default", "Request timed out after 30000ms"),
            ("600", "Accept"),
            ("10", "Request timed out after 10000ms"),
        ] {
            let result = client.call_tool(tool, ()).await.expect("tool call");
            let shown = result.text().expect("text");
            assert!(shown.starts_with(text), "{tool}: {shown}");
        }
        drop(client);
        server.stop().await.ok();
    }

    #[tokio::test(start_paused = true)]
    async fn an_elicitation_waits_for_its_own_deadline_in_process() {
        let (notifications, _received) = mpsc::channel(8);
        let context = ServerCtx::in_process(notifications, SlowPerson);
        let result = context
            .elicit_with_timeout(params(), Duration::from_secs(600))
            .await
            .expect("answer");
        assert!(matches!(result.action, ElicitAction::Accept));
        let error = context
            .elicit_with_timeout(params(), Duration::from_secs(10))
            .await
            .expect_err("deadline");
        assert!(
            matches!(
                error,
                Error::Timeout {
                    timeout_ms: 10_000,
                    ..
                }
            ),
            "{error:?}"
        );
    }
}
