//! Progressive discovery server example.
//!
//! Run with:
//!   cargo run --example progressive_server

use tmcp::{
    Group, Result, Server, ToolResult, ToolSet, group, mcp_server, tool, tool_params, tool_result,
};

/// Parameters for the group echo tool.
#[derive(Debug)]
#[tool_params]
struct EchoParams {
    /// Message to echo back.
    message: String,
}

/// Echo tool response.
#[derive(Debug)]
#[tool_result]
struct EchoResponse {
    /// Echoed message.
    message: String,
}

/// Response listing current groups.
#[derive(Debug)]
#[tool_result]
struct GroupList {
    /// Currently registered group names.
    groups: Vec<String>,
}

/// File operation tool group.
#[derive(Debug, Group)]
#[group(description = "File operations")]
struct FileOps;

#[group]
impl FileOps {
    #[tool]
    /// Echo back a message within the file ops group.
    async fn echo(&self, params: EchoParams) -> ToolResult<EchoResponse> {
        Ok(EchoResponse {
            message: params.message,
        })
    }
}

/// Diagnostics tool group.
#[derive(Debug, Group)]
#[group(description = "Diagnostics")]
struct Diagnostics;

#[group]
impl Diagnostics {
    #[tool]
    /// Return a basic ping response.
    async fn ping(&self) -> ToolResult<EchoResponse> {
        Ok(EchoResponse {
            message: "pong".to_string(),
        })
    }
}

/// Server exposing progressive discovery groups.
#[derive(Default)]
struct ProgressiveServer {
    /// Tool registry for progressive discovery.
    tools: ToolSet,
}

#[mcp_server(toolset = "tools")]
impl ProgressiveServer {
    #[group]
    /// Root group for file operations.
    fn file_ops(&self) -> FileOps {
        FileOps
    }

    #[group]
    /// Root group for diagnostics.
    fn diagnostics(&self) -> Diagnostics {
        Diagnostics
    }

    #[tool(always)]
    /// List registered group names.
    async fn list_groups(&self) -> ToolResult<GroupList> {
        let groups = self
            .tools
            .list_groups()
            .into_iter()
            .map(|group| group.name)
            .collect();
        Ok(GroupList { groups })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Server::new(ProgressiveServer::default).serve_stdio().await
}
