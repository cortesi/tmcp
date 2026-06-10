//! Duplicate tool names in one impl block are rejected.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server]
impl Server {
    #[tool]
    async fn echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }

    #[tool]
    async fn r#echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
