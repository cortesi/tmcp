//! Unknown #[mcp_server] arguments are rejected.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server(frobnicate_fn = helper)]
impl Server {
    #[tool]
    async fn echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
