//! Flat tool parameters cannot mention the impl block's generic parameters.
#![allow(unused_imports)]
use tmcp::{Result, mcp_server, schema::CallToolResult, tool};

struct Server<T> {
    marker: std::marker::PhantomData<T>,
}

#[mcp_server]
impl<T: Send + Sync + 'static> Server<T> {
    #[tool]
    async fn echo(&self, a: T, b: i32) -> Result<CallToolResult> {
        Ok(CallToolResult::new())
    }
}

fn main() {}
