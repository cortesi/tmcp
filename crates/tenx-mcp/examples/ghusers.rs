//! GitHub MCP client example demonstrating OAuth authentication.
//!
//! This example connects to GitHub's MCP server using OAuth authentication.
//! To use this example, you need to create a GitHub OAuth App:
//!
//! 1. Go to https://github.com/settings/developers
//! 2. Click "New OAuth App"
//! 3. Fill in the application details:
//!    - Application name: Choose any name
//!    - Homepage URL: Can be any valid URL
//!    - Authorization callback URL: http://localhost:8080/callback
//!      (or use a different port with the -p flag)
//! 4. After creating the app, you'll get a Client ID and Client Secret
//! 5. Run this example with:
//!    cargo run --example ghusers -- -c YOUR_CLIENT_ID -s YOUR_CLIENT_SECRET

use std::{error::Error, sync::Arc};

use clap::Parser;
use tenx_mcp::{
    Client, ServerAPI,
    auth::{OAuth2CallbackServer, OAuth2Client, OAuth2Config, OAuth2Token},
};
use tracing::{Level, debug, subscriber};
use tracing_subscriber::FmtSubscriber;

/// GitHub MCP endpoint for the Copilot service.
const GITHUB_MCP_ENDPOINT: &str = "https://api.githubcopilot.com/mcp/";
/// GitHub OAuth authorization endpoint.
const GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
/// GitHub OAuth token endpoint.
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Wrap text to fit within the specified width, with an optional indent for continuation lines
fn wrap_text(text: &str, width: usize, indent: &str) -> String {
    let words = text.split_whitespace();
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in words {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.len() + word.len() < width {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = format!("{indent}{word}");
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines.join("\n")
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Connect to GitHub MCP server with OAuth", long_about = None)]
/// CLI arguments for the GitHub OAuth example.
struct Args {
    /// GitHub OAuth App Client ID
    #[arg(short, long)]
    client_id: String,

    /// GitHub OAuth App Client Secret
    #[arg(short = 's', long)]
    client_secret: String,

    /// OAuth callback port (default: 8080)
    #[arg(short = 'p', long, default_value = "8080")]
    port: u16,

    /// Use existing access token (skip OAuth flow)
    #[arg(short = 't', long)]
    access_token: Option<String>,

    /// Enable verbose logging
    #[arg(short = 'v', long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // Set up logging based on verbose flag
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::WARN
    };
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    // Create OAuth client
    let oauth_client = if let Some(access_token) = args.access_token {
        // Use provided access token
        debug!("Using provided access token");

        let config = OAuth2Config {
            client_id: args.client_id,
            client_secret: Some(args.client_secret),
            auth_url: GITHUB_AUTH_URL.to_string(),
            token_url: GITHUB_TOKEN_URL.to_string(),
            redirect_url: format!("http://localhost:{}/callback", args.port),
            resource: GITHUB_MCP_ENDPOINT.to_string(),
            scopes: vec!["read:user".to_string()], // GitHub scope for reading user data
        };

        let oauth_client = OAuth2Client::new(config)?;

        // Set the provided token
        let token = OAuth2Token {
            access_token,
            refresh_token: None,
            expires_at: None, // GitHub tokens don't expire by default
        };
        oauth_client.set_token(token).await;

        oauth_client
    } else {
        // Perform full OAuth flow
        debug!("Starting OAuth flow...");

        let config = OAuth2Config {
            client_id: args.client_id,
            client_secret: Some(args.client_secret),
            auth_url: GITHUB_AUTH_URL.to_string(),
            token_url: GITHUB_TOKEN_URL.to_string(),
            redirect_url: format!("http://localhost:{}/callback", args.port),
            resource: GITHUB_MCP_ENDPOINT.to_string(),
            scopes: vec!["read:user".to_string()],
        };

        let mut oauth_client = OAuth2Client::new(config)?;

        // Get authorization URL
        let (auth_url, _csrf_token) = oauth_client.get_authorization_url();

        println!("Opening browser for GitHub authorization...");
        println!("If the browser doesn't open, visit: {auth_url}");

        // Try to open the browser
        if let Err(e) = webbrowser::open(auth_url.as_str()) {
            eprintln!("Failed to open browser: {e}");
            eprintln!("Please visit the URL manually: {auth_url}");
        }

        // Start callback server
        let callback_server = OAuth2CallbackServer::new(args.port);
        debug!("Waiting for OAuth callback on port {}...", args.port);

        let (code, state) = callback_server.wait_for_callback().await?;

        debug!("Received authorization code, exchanging for token...");

        // Exchange code for token
        let token = oauth_client.exchange_code(code, state).await?;
        debug!("Successfully obtained access token: {}", token.access_token);

        oauth_client
    };

    // Create MCP client with OAuth
    let oauth_client_arc = Arc::new(oauth_client);
    let mut client = Client::new("ghusers-example", "1.0.0");

    debug!(
        "Connecting to GitHub MCP server at {}...",
        GITHUB_MCP_ENDPOINT
    );

    let init_result = client
        .connect_http_with_oauth(GITHUB_MCP_ENDPOINT, oauth_client_arc.clone())
        .await?;

    println!(
        "Connected to {} v{}",
        init_result.server_info.name, init_result.server_info.version
    );

    // List available tools
    println!("\nAvailable tools:");
    let tools = client.list_tools(None).await?;

    if tools.tools.is_empty() {
        println!("No tools available");
    } else {
        for tool in &tools.tools {
            println!("\n{}", tool.name);
            if let Some(desc) = &tool.description {
                let wrapped = wrap_text(desc, 72, "  ");
                println!("  {wrapped}");
            }

            // Parse and display input parameters
            if let Some(properties) = &tool.input_schema.properties
                && !properties.is_empty()
            {
                println!("  Parameters:");

                let required = tool.input_schema.required.as_deref().unwrap_or(&[]);

                for (name, schema) in properties {
                    let param_type = schema
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown");
                    let description = schema
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let is_required = required.contains(name);

                    let param_header = format!(
                        "    - {} ({}){}: ",
                        name,
                        param_type,
                        if is_required { ", required" } else { "" }
                    );

                    if description.is_empty() {
                        println!("{}", param_header.trim_end_matches(": "));
                    } else {
                        let wrapped_desc =
                            wrap_text(description, 72 - param_header.len(), "      ");
                        println!("{param_header}{wrapped_desc}");
                    }
                }
            }
        }
    }

    // Get the access token for future use
    if let Ok(token) = oauth_client_arc.get_valid_token().await
        && args.verbose
    {
        println!("\nAccess token (save for future use): {token}");
        println!("You can skip the OAuth flow next time by using: --access-token {token}");
    }

    Ok(())
}
