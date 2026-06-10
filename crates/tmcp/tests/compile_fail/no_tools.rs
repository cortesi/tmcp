//! #[mcp_server] requires at least one tool or resource callback.
use tmcp::mcp_server;

struct Server;

#[mcp_server]
impl Server {
    async fn helper(&self) -> String {
        "helper".to_string()
    }
}

fn main() {}
