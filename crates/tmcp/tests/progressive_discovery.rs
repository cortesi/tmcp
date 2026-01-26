//! Progressive discovery integration tests.

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tmcp::{
        Arguments, Error, Group, GroupConfig, Result, ServerCtx, ServerHandler, ToolFuture,
        ToolSet, Visibility, group, mcp_server,
        schema::{CallToolResult, ServerNotification, Tool, ToolSchema},
        testutils::TestServerContext,
        tool,
    };

    #[derive(Debug, Group)]
    struct FileOps;

    #[group]
    impl FileOps {
        #[tool]
        async fn read_file(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new())
        }
    }

    #[derive(Debug, Group)]
    struct Network;

    #[group]
    impl Network {
        #[tool]
        async fn fetch_url(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new())
        }
    }

    #[derive(Default)]
    struct AutomationHub {
        tools: ToolSet,
    }

    #[mcp_server(toolset = "tools")]
    impl AutomationHub {
        #[group]
        fn file_ops(&self) -> FileOps {
            FileOps
        }

        #[group]
        fn network(&self) -> Network {
            Network
        }
    }

    fn ok_handler<'a>(_ctx: &'a ServerCtx, _args: Option<Arguments>) -> ToolFuture<'a> {
        Box::pin(async { Ok(CallToolResult::new()) })
    }

    fn group_config(name: &str, description: &str) -> GroupConfig {
        GroupConfig {
            name: name.to_string(),
            description: description.to_string(),
            parent: None,
            on_activate: None,
            on_deactivate: None,
            show_deactivator: true,
        }
    }

    #[tokio::test]
    async fn test_progressive_activation() {
        let server = AutomationHub::default();
        let ctx = TestServerContext::new();

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert!(tools.tools.iter().any(|t| t.name == "file_ops.activate"));
        assert!(!tools.tools.iter().any(|t| t.name == "file_ops.read_file"));

        let result = server
            .call_tool(ctx.ctx(), "file_ops.read_file".to_string(), None, None)
            .await;
        assert!(matches!(result, Err(Error::ToolNotFound(_))));

        server
            .call_tool(ctx.ctx(), "file_ops.activate".to_string(), None, None)
            .await
            .unwrap();

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert!(tools.tools.iter().any(|t| t.name == "file_ops.read_file"));
    }

    #[tokio::test]
    async fn test_mutual_exclusion() {
        let tools = ToolSet::new();
        tools
            .register_group(group_config("mode_a", "Mode A"))
            .unwrap();
        tools
            .register_group(group_config("mode_b", "Mode B"))
            .unwrap();
        tools.register_exclusion(&["mode_a", "mode_b"]).unwrap();

        let ctx = TestServerContext::new();

        tools.activate_group("mode_a", ctx.ctx()).await.unwrap();
        assert!(tools.is_group_active("mode_a"));
        assert!(!tools.is_group_active("mode_b"));

        tools.activate_group("mode_b", ctx.ctx()).await.unwrap();
        assert!(!tools.is_group_active("mode_a"));
        assert!(tools.is_group_active("mode_b"));
    }

    #[tokio::test]
    async fn test_hierarchical_groups() {
        let tools = ToolSet::new();
        tools
            .register_group(group_config("database", "Database operations"))
            .unwrap();
        tools
            .register_group(GroupConfig {
                name: "database.write".to_string(),
                description: "Write operations".to_string(),
                parent: Some("database".to_string()),
                on_activate: None,
                on_deactivate: None,
                show_deactivator: true,
            })
            .unwrap();

        let ctx = TestServerContext::new();

        let result = tools.activate_group("database.write", ctx.ctx()).await;
        assert!(matches!(result, Err(Error::GroupInactive { .. })));

        tools.activate_group("database", ctx.ctx()).await.unwrap();
        tools
            .activate_group("database.write", ctx.ctx())
            .await
            .unwrap();

        assert!(tools.is_group_active("database"));
        assert!(tools.is_group_active("database.write"));

        tools.deactivate_group("database", ctx.ctx()).await.unwrap();
        assert!(!tools.is_group_active("database"));
        assert!(!tools.is_group_active("database.write"));
    }

    #[tokio::test]
    async fn test_when_visibility() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let tools = ToolSet::new();
        tools
            .register_with_visibility(
                "conditional_tool",
                Tool::new("conditional_tool", ToolSchema::empty()),
                Visibility::When(Arc::new(move |_view| flag_clone.load(Ordering::Relaxed))),
                ok_handler,
            )
            .unwrap();

        let visible = tools.list_tools(None).unwrap();
        assert!(!visible.tools.iter().any(|t| t.name == "conditional_tool"));

        flag.store(true, Ordering::Relaxed);

        let visible = tools.list_tools(None).unwrap();
        assert!(visible.tools.iter().any(|t| t.name == "conditional_tool"));
    }

    #[tokio::test]
    async fn test_deactivation() {
        let server = AutomationHub::default();
        let ctx = TestServerContext::new();

        server
            .call_tool(ctx.ctx(), "file_ops.activate".to_string(), None, None)
            .await
            .unwrap();
        assert!(server.tools.is_group_active("file_ops"));

        server
            .call_tool(ctx.ctx(), "file_ops.deactivate".to_string(), None, None)
            .await
            .unwrap();
        assert!(!server.tools.is_group_active("file_ops"));

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert!(!tools.tools.iter().any(|t| t.name == "file_ops.read_file"));
    }

    #[tokio::test]
    async fn test_activation_hooks() {
        let tools = ToolSet::new();
        let ctx = TestServerContext::new();

        let activated = Arc::new(AtomicBool::new(false));
        let flag = activated.clone();

        tools
            .register_group(GroupConfig {
                name: "file_ops".to_string(),
                description: "File operations".to_string(),
                parent: None,
                on_activate: Some(Box::new(move |_ctx| {
                    let flag = flag.clone();
                    Box::pin(async move {
                        flag.store(true, Ordering::Relaxed);
                        Ok(())
                    })
                })),
                on_deactivate: None,
                show_deactivator: true,
            })
            .unwrap();

        tools.activate_group("file_ops", ctx.ctx()).await.unwrap();
        assert!(activated.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_hidden_deactivator() {
        let tools = ToolSet::new();
        let ctx = TestServerContext::new();

        tools
            .register_group(GroupConfig {
                name: "file_ops".to_string(),
                description: "File operations".to_string(),
                parent: None,
                on_activate: None,
                on_deactivate: None,
                show_deactivator: false,
            })
            .unwrap();

        let list = tools.list_tools(None).unwrap();
        assert!(list.tools.iter().any(|t| t.name == "file_ops.activate"));
        assert!(!list.tools.iter().any(|t| t.name == "file_ops.deactivate"));

        tools.activate_group("file_ops", ctx.ctx()).await.unwrap();
        tools.deactivate_group("file_ops", ctx.ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_groups() {
        let tools = ToolSet::new();
        tools
            .register_group(group_config("file_ops", "File operations"))
            .unwrap();
        tools
            .register_group(group_config("network", "Network operations"))
            .unwrap();

        let ctx = TestServerContext::new();
        tools.activate_group("file_ops", ctx.ctx()).await.unwrap();

        let groups = tools.list_groups();
        assert_eq!(groups.len(), 2);

        let file_ops = groups.iter().find(|g| g.name == "file_ops").unwrap();
        assert!(file_ops.active);
        assert_eq!(file_ops.description, "File operations");

        let network = groups.iter().find(|g| g.name == "network").unwrap();
        assert!(!network.active);
    }

    #[tokio::test]
    async fn test_root_not_listed() {
        let tools = ToolSet::new();
        tools
            .register_group(group_config("file_ops", "File operations"))
            .unwrap();
        let groups = tools.list_groups();
        assert!(groups.iter().all(|g| g.name != "root"));
    }

    #[tokio::test]
    async fn test_list_tools_ordering() {
        let tools = ToolSet::new();
        tools
            .register(
                "b_tool",
                Tool::new("b_tool", ToolSchema::empty()),
                ok_handler,
            )
            .unwrap();
        tools
            .register(
                "a_tool",
                Tool::new("a_tool", ToolSchema::empty()),
                ok_handler,
            )
            .unwrap();
        let list = tools.list_tools(None).unwrap();
        assert_eq!(list.tools[0].name, "a_tool");
        assert_eq!(list.tools[1].name, "b_tool");
    }

    #[tokio::test]
    async fn test_tool_list_changed_notification() {
        let server = AutomationHub::default();
        let mut ctx = TestServerContext::new();

        server
            .call_tool(ctx.ctx(), "file_ops.activate".to_string(), None, None)
            .await
            .unwrap();

        let notification = ctx.try_recv_notification().await;
        assert!(matches!(
            notification,
            Some(ServerNotification::ToolListChanged { .. })
        ));
    }

    #[tokio::test]
    async fn test_concurrent_activation() {
        let server = Arc::new(AutomationHub::default());
        let ctx = TestServerContext::new();
        server.list_tools(ctx.ctx(), None).await.unwrap();
        let ctx = ctx.ctx().clone();

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let server = server.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let group = if i % 2 == 0 { "file_ops" } else { "network" };
                    let _ = server
                        .tools
                        .activate_group(group, &ctx)
                        .await
                        .unwrap_or(false);
                })
            })
            .collect();

        for handle in handles {
            handle.await.unwrap();
        }

        let groups = server.tools.list_groups();
        assert!(groups.iter().any(|g| g.active));
    }
}
