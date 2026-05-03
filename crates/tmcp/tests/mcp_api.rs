//! MCP API inspection tests.

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tmcp::{
        Error, Result, ServerCtx, ServerHandler, inspect_server,
        schema::{
            ClientCapabilities, Cursor, Implementation, InitializeResult, ListPromptsResult,
            ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, Prompt, Resource,
            ResourceTemplate, Tool, ToolSchema,
        },
    };

    /// Server used to exercise each inspected MCP API list.
    struct ApiServer;

    /// Server that rejects listing methods it does not advertise.
    struct NoCapabilitiesServer;

    #[async_trait]
    impl ServerHandler for ApiServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("api-server")
                .with_version("1.2.3")
                .with_tools(false)
                .with_resources(false, true)
                .with_prompts(false))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            match cursor.as_ref().map(|cursor| cursor.0.as_str()) {
                None => Ok(ListToolsResult::new()
                    .with_tool(Tool::new("first_tool", ToolSchema::empty()))
                    .with_cursor("next-tools")),
                Some("next-tools") => Ok(ListToolsResult::new().with_tool(
                    Tool::new("second_tool", ToolSchema::empty()).with_description("Second page"),
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
            _protocol_version: String,
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

    /// The inspector collects initialization metadata and every static list page.
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

    /// The inspector does not call list methods for capabilities the server omits.
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
}
