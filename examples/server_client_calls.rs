//! Example demonstrating server-initiated calls to the client
//!
//! This example shows how a server can use `ServerCtx` to make requests
//! back to the connected client. These are useful for:
//!
//! - **list_roots**: Discover filesystem roots the client has exposed
//! - **create_message**: Request LLM sampling from the client (if supported)
//! - **elicit**: Ask the client for user input
//! - **ping**: Verify the connection is alive
//!
//! The server exposes tools that demonstrate each capability, allowing you
//! to test these features with any MCP client.
//!
//! # Usage
//!
//! ```bash
//! # Start the server
//! cargo run --example server_client_calls -- --host 127.0.0.1 --port 3000
//!
//! # Connect with an MCP client and call the tools
//! ```

use std::env;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tmcp::{
    Arguments, Error, Result, Server, ServerCtx, ServerHandler,
    schema::{
        self, CallToolResult, ClientCapabilities, CreateMessageParams, ElicitRequestParams,
        Implementation, InitializeResult, ListToolsResult, Tool, ToolSchema,
    },
};
use tokio::signal::ctrl_c;
use tracing::{error, info};
use tracing_subscriber::fmt;

/// Parameters for the ask_llm tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct AskLlmParams {
    /// The prompt to send to the LLM.
    prompt: String,
}

/// Parameters for the ask_user tool.
#[derive(Debug, Deserialize, JsonSchema)]
struct AskUserParams {
    /// The question to ask the user.
    question: String,
}

/// Server that demonstrates calling back to the client
struct ClientCallsServer;

#[async_trait]
impl ServerHandler for ClientCallsServer {
    async fn initialize(
        &self,
        _context: &ServerCtx,
        _protocol_version: String,
        capabilities: ClientCapabilities,
        client_info: Implementation,
    ) -> Result<InitializeResult> {
        info!(
            "Client connected: {} v{}",
            client_info.name, client_info.version
        );

        // Log what the client supports
        if capabilities.roots.is_some() {
            info!("  - Client supports roots");
        }
        if capabilities.sampling.is_some() {
            info!("  - Client supports sampling (LLM requests)");
        }
        if capabilities.elicitation.is_some() {
            info!("  - Client supports elicitation (user input)");
        }

        Ok(InitializeResult::new("server-client-calls-demo")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_tools(true))
    }

    async fn list_tools(
        &self,
        _context: &ServerCtx,
        _cursor: Option<schema::Cursor>,
    ) -> Result<ListToolsResult> {
        // Tools with no parameters use ToolSchema::empty() for clarity
        // Tools with parameters use Tool::from_schema<T>() for type-safe schema generation
        Ok(ListToolsResult::new()
            .with_tool(
                Tool::new("ping_client", ToolSchema::empty())
                    .with_description("Ping the connected client to verify the connection"),
            )
            .with_tool(
                Tool::new("list_roots", ToolSchema::empty())
                    .with_description("List filesystem roots exposed by the client"),
            )
            .with_tool(
                Tool::from_schema::<AskLlmParams>("ask_llm")
                    .with_description("Request the client to generate an LLM response (sampling)"),
            )
            .with_tool(
                Tool::from_schema::<AskUserParams>("ask_user")
                    .with_description("Ask the client to get input from the user (elicitation)"),
            ))
    }

    async fn call_tool(
        &self,
        context: &ServerCtx,
        name: String,
        arguments: Option<Arguments>,
        _task: Option<schema::TaskMetadata>,
    ) -> Result<CallToolResult> {
        match name.as_str() {
            "ping_client" => {
                // Ping the client to verify the connection is alive
                info!("Pinging client...");
                context.ping().await?;
                info!("Client responded to ping");

                Ok(CallToolResult::new().with_text_content("Ping successful!"))
            }

            "list_roots" => {
                // List the filesystem roots the client has exposed
                info!("Requesting roots from client...");

                match context.list_roots().await {
                    Ok(result) => {
                        let root_names: Vec<&str> =
                            result.roots.iter().map(|r| r.name.as_str()).collect();
                        info!("Client exposed {} roots: {:?}", result.roots.len(), root_names);

                        if result.roots.is_empty() {
                            Ok(CallToolResult::new()
                                .with_text_content("Client has not exposed any filesystem roots"))
                        } else {
                            let roots_json = serde_json::to_string_pretty(&result.roots)
                                .unwrap_or_else(|_| "[]".to_string());
                            Ok(CallToolResult::new()
                                .with_text_content(format!("Client roots:\n{}", roots_json)))
                        }
                    }
                    Err(e) => {
                        error!("Failed to list roots: {}", e);
                        Ok(CallToolResult::new()
                            .with_text_content(format!("Error listing roots: {}", e))
                            .with_is_error(true))
                    }
                }
            }

            "ask_llm" => {
                // Request the client to generate an LLM response
                // Use into_params() for type-safe deserialization matching from_schema
                let params: AskLlmParams = arguments.unwrap_or_default().into_params()?;

                info!("Requesting LLM sampling from client: {}", params.prompt);

                let msg_params =
                    CreateMessageParams::user_message(&params.prompt).with_max_tokens(500);

                match context.create_message(msg_params).await {
                    Ok(result) => {
                        info!("Received LLM response");
                        let response = match &result.content {
                            schema::MessageContent::Text(text) => text.text.clone(),
                            schema::MessageContent::Image(_) => "[Image response]".to_string(),
                            schema::MessageContent::Audio(_) => "[Audio response]".to_string(),
                        };
                        Ok(CallToolResult::new().with_text_content(format!(
                            "LLM Response (model: {:?}):\n{}",
                            result.model, response
                        )))
                    }
                    Err(e) => {
                        error!("LLM sampling failed: {}", e);
                        Ok(CallToolResult::new()
                            .with_text_content(format!(
                                "LLM sampling not supported or failed: {}",
                                e
                            ))
                            .with_is_error(true))
                    }
                }
            }

            "ask_user" => {
                // Ask the client to get input from the user
                // Use into_params() for type-safe deserialization matching from_schema
                let params: AskUserParams = arguments.unwrap_or_default().into_params()?;

                info!("Requesting user input from client: {}", params.question);

                let elicit_params = ElicitRequestParams {
                    message: params.question,
                    requested_schema: None,
                    _meta: None,
                };

                match context.elicit(elicit_params).await {
                    Ok(result) => {
                        info!("Received user response: {:?}", result.action);
                        let response = match result.content {
                            Some(content) => serde_json::to_string_pretty(&content)
                                .unwrap_or_else(|_| "null".to_string()),
                            None => "No content provided".to_string(),
                        };
                        Ok(CallToolResult::new().with_text_content(format!(
                            "User response (action: {:?}):\n{}",
                            result.action, response
                        )))
                    }
                    Err(e) => {
                        error!("Elicitation failed: {}", e);
                        Ok(CallToolResult::new()
                            .with_text_content(format!(
                                "Elicitation not supported or failed: {}",
                                e
                            ))
                            .with_is_error(true))
                    }
                }
            }

            _ => Err(Error::ToolNotFound(name)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt::init();

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();

    let (host, port) = if args.len() >= 3 {
        (
            args.iter()
                .position(|a| a == "--host")
                .map(|i| args[i + 1].clone())
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            args.iter()
                .position(|a| a == "--port")
                .map(|i| args[i + 1].parse().expect("Invalid port"))
                .unwrap_or(3000u16),
        )
    } else {
        ("127.0.0.1".to_string(), 3000)
    };

    let addr = format!("{host}:{port}");

    info!("Starting server-client-calls demo on {}", addr);
    info!("Connect with an MCP client and try the following tools:");
    info!("  - ping_client: Verify the connection");
    info!("  - list_roots: See what filesystem roots the client exposes");
    info!("  - ask_llm: Request LLM sampling (requires client support)");
    info!("  - ask_user: Request user input (requires client support)");
    info!("");

    let handle = Server::new(|| ClientCallsServer).serve_tcp(addr).await?;

    // Wait for Ctrl+C signal
    ctrl_c().await?;
    info!("Shutting down server");

    handle.stop().await?;

    Ok(())
}
