//! Unknown #[tool] keys are rejected.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server]
impl Server {
    #[tool(frobnicate)]
    async fn echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
