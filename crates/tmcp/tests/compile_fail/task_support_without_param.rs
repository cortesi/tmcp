//! task_support = "optional"/"required" needs a task metadata parameter.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server]
impl Server {
    #[tool(task_support = "required")]
    async fn echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
