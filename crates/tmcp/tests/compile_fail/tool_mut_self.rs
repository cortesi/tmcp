//! #[tool] methods must take &self, not &mut self.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server;

#[mcp_server]
impl Server {
    #[tool]
    async fn echo(&mut self) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
