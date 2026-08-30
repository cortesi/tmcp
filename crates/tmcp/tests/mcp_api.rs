//! MCP API inspection tests.

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tmcp::{
        Error, McpApiRefreshState, Result, ServerCtx, ServerHandler, inspect_client,
        inspect_server,
        schema::{
            ClientCapabilities, Cursor, Implementation, InitializeResult, ListPromptsResult,
            ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, Prompt,
            ProtocolVersion, Resource, ResourceTemplate, ServerNotification, Tool, ToolSchema,
        },
        testutils::{connected_client_and_server, shutdown_client_and_server},
    };
    #[cfg(feature = "render")]
    use tmcp::{McpApiRenderOptions, render_mcp_api};

    /// Server used to exercise each inspected MCP API list.
    struct ApiServer;

    /// Server that rejects listing methods it does not advertise.
    struct NoCapabilitiesServer;

    #[async_trait]
    impl ServerHandler for ApiServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("api-server")
                .with_version("1.2.3")
                .with_tools(Some(false))
                .with_resources(Some(false), Some(true))
                .with_prompts(Some(false)))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            match cursor.as_ref().map(|cursor| cursor.0.as_str()) {
                None => Ok(ListToolsResult::new()
                    .with_tool(Tool::new("first_tool", ToolSchema::default()))
                    .with_cursor("next-tools")),
                Some("next-tools") => Ok(ListToolsResult::new().with_tool(
                    Tool::new("second_tool", ToolSchema::default()).with_description("Second page"),
                )),
                other => Err(Error::InvalidRequest(format!(
                    "unexpected tools cursor: {other:?}"
                ))),
            }
        }

        async fn list_resources(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListResourcesResult> {
            Ok(ListResourcesResult::new().with_resource(
                Resource::new("Guide", "tmcp://guide").with_mime_type("text/markdown"),
            ))
        }

        async fn list_resource_templates(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListResourceTemplatesResult> {
            Ok(
                ListResourceTemplatesResult::new().with_resource_template(ResourceTemplate::new(
                    "Guide Template",
                    "tmcp://guide/{name}",
                )),
            )
        }

        async fn list_prompts(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListPromptsResult> {
            Ok(ListPromptsResult::new().with_prompt(Prompt::new("summarize")))
        }
    }

    #[async_trait]
    impl ServerHandler for NoCapabilitiesServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("no-capabilities-server"))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            Err(Error::InvalidRequest("tools are not advertised".to_owned()))
        }
    }

    /// The inspector collects initialization metadata and every static list
    /// page.
    #[tokio::test]
    async fn inspect_server_collects_mcp_api() {
        let api = inspect_server(&ApiServer).await.expect("inspect server");

        assert_eq!(api.initialize.server_info.name, "api-server");
        assert_eq!(
            api.tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["first_tool", "second_tool"]
        );
        assert_eq!(api.resources[0].uri, "tmcp://guide");
        assert_eq!(
            api.resource_templates[0].uri_template,
            "tmcp://guide/{name}"
        );
        assert_eq!(api.prompts[0].name, "summarize");

        let json = serde_json::to_value(&api).expect("serialize API");
        assert!(json.get("resourceTemplates").is_some());
        assert!(json.get("resource_templates").is_none());
    }

    /// The connected-client inspector collects the same advertised API over
    /// MCP.
    #[tokio::test]
    async fn inspect_client_collects_mcp_api() {
        let (mut client, server) = connected_client_and_server(|| ApiServer)
            .await
            .expect("connect");
        let initialize = client.init().await.expect("initialize");

        let api = inspect_client(&client, initialize)
            .await
            .expect("inspect client");

        assert_eq!(api.initialize.server_info.name, "api-server");
        assert_eq!(
            api.tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["first_tool", "second_tool"]
        );
        assert_eq!(api.resources[0].uri, "tmcp://guide");
        assert_eq!(
            api.resource_templates[0].uri_template,
            "tmcp://guide/{name}"
        );
        assert_eq!(api.prompts[0].name, "summarize");

        shutdown_client_and_server(client, server).await;
    }

    /// API refresh state coalesces list-change notifications until the next
    /// take.
    #[test]
    fn api_refresh_state_tracks_list_changed_notifications() {
        let state = McpApiRefreshState::default();

        state.observe_server_notification(&ServerNotification::tool_list_changed());
        state.observe_server_notification(&ServerNotification::resource_list_changed());

        let snapshot = state.snapshot();
        assert!(snapshot.tools);
        assert!(snapshot.resources);
        assert!(!snapshot.prompts);
        assert!(!snapshot.is_empty());

        let taken = state.take();
        assert_eq!(snapshot, taken);
        assert!(state.take().is_empty());
    }

    /// Notifications observed while another snapshot is in progress are not
    /// lost.
    #[test]
    fn api_refresh_state_preserves_mid_snapshot_notifications() {
        let state = McpApiRefreshState::default();

        let first = state.take();
        state.observe_server_notification(&ServerNotification::prompt_list_changed());

        assert!(first.is_empty());
        assert!(state.take().prompts);
    }

    /// The inspector does not call list methods for capabilities the server
    /// omits.
    #[tokio::test]
    async fn inspect_server_skips_unadvertised_lists() {
        let api = inspect_server(&NoCapabilitiesServer)
            .await
            .expect("inspect server");

        assert_eq!(api.initialize.server_info.name, "no-capabilities-server");
        assert!(api.tools.is_empty());
        assert!(api.resources.is_empty());
        assert!(api.resource_templates.is_empty());
        assert!(api.prompts.is_empty());
    }

    /// The public renderer formats inspected APIs as human-readable text.
    #[cfg(feature = "render")]
    #[tokio::test]
    async fn render_mcp_api_formats_inspected_api() {
        let api = inspect_server(&ApiServer).await.expect("inspect server");

        let rendered = render_mcp_api(&api, McpApiRenderOptions { color: false });

        assert!(rendered.contains("MCP API"));
        assert!(rendered.contains("Tools (2)"));
        assert!(rendered.contains("first_tool"));
        assert!(rendered.contains("Resource Templates (1)"));
        assert!(!rendered.contains("No fields."));
        assert!(!rendered.contains("\x1b["));
    }

    /// Surface digests are stable across list ordering but change with schema
    /// content.
    #[tokio::test]
    async fn surface_digest_is_stable_for_ordering() {
        let api = inspect_server(&ApiServer).await.expect("inspect server");
        let mut reordered = api.clone();
        reordered.tools.reverse();

        assert_eq!(api.surface_digest(), reordered.surface_digest());

        reordered.tools[0].description = Some("Changed description".to_owned());
        assert_ne!(api.surface_digest(), reordered.surface_digest());
    }
}
