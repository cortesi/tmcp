//! Example MCP client using a custom connection type.

use async_trait::async_trait;
use tmcp::{Client, ClientCtx, ClientHandler, Result, ServerAPI, schema};
use tokio::signal::ctrl_c;
use tracing_subscriber::fmt;

/// Example client connection that handles server requests.
#[derive(Clone)]
struct MyClientHandler {
    /// Client name for logging.
    name: String,
}

#[async_trait]
impl ClientHandler for MyClientHandler {
    async fn on_connect(&self, context: &ClientCtx) -> Result<()> {
        println!("Client connection established for: {}", self.name);

        // Example: Send a notification when connected
        context.send_notification(schema::ClientNotification::Initialized)?;

        Ok(())
    }

    async fn on_shutdown(&self, _context: &ClientCtx) -> Result<()> {
        println!("Client connection closed for: {}", self.name);
        Ok(())
    }

    async fn pong(&self, _context: &ClientCtx) -> Result<()> {
        println!("Server pinged us!");
        Ok(())
    }

    async fn create_message(
        &self,
        _context: &ClientCtx,
        method: &str,
        params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        println!(
            "Server requested message creation via {}: {:?}",
            method, params
        );

        // Return a simple response
        // Extract the last message text
        let last_message_text = params
            .messages
            .last()
            .and_then(|m| match &m.content {
                schema::OneOrMany::One(block) => match block {
                    schema::SamplingMessageContentBlock::Text(text_content) => {
                        Some(text_content.text.as_str())
                    }
                    _ => None,
                },
                schema::OneOrMany::Many(blocks) => blocks.iter().find_map(|block| match block {
                    schema::SamplingMessageContentBlock::Text(text_content) => {
                        Some(text_content.text.as_str())
                    }
                    _ => None,
                }),
            })
            .unwrap_or("(no message)");

        Ok(schema::CreateMessageResult {
            message: schema::SamplingMessage {
                role: schema::Role::Assistant,
                content: schema::OneOrMany::One(schema::SamplingMessageContentBlock::Text(
                    schema::TextContent {
                        text: format!("Response to: {last_message_text}"),
                        annotations: None,
                        _meta: None,
                    },
                )),
                _meta: None,
            },
            model: "example-model".to_string(),
            stop_reason: None,
        })
    }

    async fn list_roots(&self, _context: &ClientCtx) -> Result<schema::ListRootsResult> {
        println!("Server requested roots list");

        Ok(schema::ListRootsResult {
            roots: vec![schema::Root {
                uri: "file:///home/user/project".to_string(),
                name: Some("My Project".to_string()),
                _meta: None,
            }],
            _meta: None,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    fmt::init();

    // Create a client with a custom connection handler
    let mut client = Client::new("example-client", "1.0.0").with_handler(MyClientHandler {
        name: "ExampleClient".to_string(),
    });

    // Connect to a server via TCP
    let server_info = client.connect_tcp("127.0.0.1:3000").await?;

    println!(
        "Connected to server: {} v{}",
        server_info.server_info.name, server_info.server_info.version
    );

    // The client is now ready to handle both:
    // 1. Client-initiated requests (tools, resources, etc.)
    // 2. Server-initiated requests (via ClientHandler trait)

    // Example: Call a tool
    match client.list_tools(None).await {
        Ok(tools) => {
            println!("Available tools: {:?}", tools.tools);
        }
        Err(e) => {
            eprintln!("Failed to list tools: {e}");
        }
    }

    // Keep the client running to handle server requests
    println!("Client running. Press Ctrl+C to exit.");
    ctrl_c().await?;

    Ok(())
}
