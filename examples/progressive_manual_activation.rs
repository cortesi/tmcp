//! Progressive discovery example with custom activate/deactivate tools.
//!
//! Run with:
//!   cargo run --example progressive_manual_activation

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tmcp::{
    Group, Result, Server, ServerCtx, ToolResult, ToolSet, group, mcp_server,
    schema::CallToolResult, tool,
};

/// Shared state for the workspace group.
#[derive(Debug, Default)]
struct WorkspaceState {
    /// Tracks whether setup has completed.
    ready: Arc<AtomicBool>,
}

/// Group that overrides activate/deactivate to perform setup and teardown.
#[derive(Clone, Group)]
#[group(description = "Workspace tools")]
struct Workspace {
    /// Tool registry used to toggle group activation.
    tools: ToolSet,
    /// Shared setup flag.
    ready: Arc<AtomicBool>,
    /// Fully qualified group name for activation calls.
    name: String,
}

#[group]
impl Workspace {
    /// Create a workspace group bound to the server toolset.
    fn new(tools: ToolSet, state: &WorkspaceState, name: &str) -> Self {
        Self {
            tools,
            ready: state.ready.clone(),
            name: name.to_string(),
        }
    }

    #[tool]
    /// Custom activation: run setup before enabling the group.
    async fn activate(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.ready.store(true, Ordering::Relaxed);
        self.tools.activate_group(&self.name, ctx).await?;
        Ok(CallToolResult::new().with_text_content("workspace activated"))
    }

    #[tool]
    /// Custom deactivation: run teardown before disabling the group.
    async fn deactivate(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.ready.store(false, Ordering::Relaxed);
        self.tools.deactivate_group(&self.name, ctx).await?;
        Ok(CallToolResult::new().with_text_content("workspace deactivated"))
    }

    #[tool]
    /// Report the current setup state.
    async fn status(&self) -> ToolResult<CallToolResult> {
        let message = if self.ready.load(Ordering::Relaxed) {
            "ready"
        } else {
            "not ready"
        };
        Ok(CallToolResult::new().with_text_content(message))
    }
}

/// Server that exposes the workspace group.
#[derive(Default)]
struct ManualActivationServer {
    /// Tool registry for progressive discovery.
    tools: ToolSet,
    /// Shared workspace state.
    state: WorkspaceState,
}

#[mcp_server(toolset = "tools")]
impl ManualActivationServer {
    #[group]
    /// Root group for workspace tools.
    fn workspace(&self) -> Workspace {
        Workspace::new(self.tools.clone(), &self.state, "workspace")
    }

    #[tool(always)]
    /// Query readiness from a root-level tool.
    async fn ready(&self) -> ToolResult<CallToolResult> {
        let message = if self.state.ready.load(Ordering::Relaxed) {
            "workspace ready"
        } else {
            "workspace not ready"
        };
        Ok(CallToolResult::new().with_text_content(message))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Server::new(ManualActivationServer::default)
        .serve_stdio()
        .await
}
