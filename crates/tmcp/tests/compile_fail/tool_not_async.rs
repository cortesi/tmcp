//! #[tool] methods must be async.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server]
impl Server {
    #[tool]
    fn echo(&self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
