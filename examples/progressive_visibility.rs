//! Dynamic visibility example using ToolSet predicates.
//!
//! Run with:
//!   cargo run --example progressive_visibility

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tmcp::{
    Arguments, Result, Server, ServerCtx, ToolFuture, ToolSet, Visibility, mcp_server,
    schema::{CallToolResult, Tool, ToolSchema},
    tool,
};

/// Server demonstrating predicate-based tool visibility.
struct VisibilityServer {
    /// Tool registry managing visibility rules.
    tools: ToolSet,
    /// Runtime flag for the beta tool visibility.
    beta_enabled: Arc<AtomicBool>,
}

/// Handler for the beta tool.
fn beta_greet<'a>(_ctx: &'a ServerCtx, _args: Option<Arguments>) -> ToolFuture<'a> {
    Box::pin(async { Ok(CallToolResult::new().with_text_content("hello from the beta tool")) })
}

#[mcp_server(toolset = "tools")]
impl VisibilityServer {
    /// Create a new server with a predicate-gated tool.
    fn new() -> Self {
        let tools = ToolSet::default();
        let beta_enabled = Arc::new(AtomicBool::new(false));
        let flag = beta_enabled.clone();

        tools
            .register_with_visibility(
                "beta_greet",
                Tool::new("beta_greet", ToolSchema::empty())
                    .with_description("Greet from a beta-only tool"),
                Visibility::When(Arc::new(move |_view| flag.load(Ordering::Relaxed))),
                beta_greet,
            )
            .expect("register beta tool");

        Self {
            tools,
            beta_enabled,
        }
    }

    #[tool(always)]
    /// Enable the beta tool and notify clients.
    async fn enable_beta(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.beta_enabled.store(true, Ordering::Relaxed);
        self.tools.notify_list_changed(ctx)?;
        Ok(CallToolResult::new().with_text_content("beta tools enabled"))
    }

    #[tool(always)]
    /// Disable the beta tool and notify clients.
    async fn disable_beta(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.beta_enabled.store(false, Ordering::Relaxed);
        self.tools.notify_list_changed(ctx)?;
        Ok(CallToolResult::new().with_text_content("beta tools disabled"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Server::new(VisibilityServer::new).serve_stdio().await
}
