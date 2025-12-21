//! OAuth-enabled client example for MCP over HTTP.

use std::{error::Error, sync::Arc};

use clap::Parser;
use tenx_mcp::{
    Client, ServerAPI,
    auth::{OAuth2CallbackServer, OAuth2Client, OAuth2Config},
};
use tracing::{Level, info, subscriber};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
/// CLI arguments for the OAuth client example.
struct Args {
    /// MCP server endpoint URL
    #[arg(short, long)]
    endpoint: String,

    /// OAuth client ID
    #[arg(short, long)]
    client_id: String,

    /// OAuth client secret (optional for public clients)
    #[arg(short = 's', long)]
    client_secret: Option<String>,

    /// OAuth authorization URL
    #[arg(short, long)]
    auth_url: String,

    /// OAuth token URL
    #[arg(short, long)]
    token_url: String,

    /// OAuth redirect URL (default: http://localhost:8080/callback)
    #[arg(short, long, default_value = "http://localhost:8080/callback")]
    redirect_url: String,

    /// OAuth scopes (comma-separated)
    #[arg(long, default_value = "")]
    scopes: String,

    /// Resource identifier for the MCP server
    #[arg(short = 'r', long)]
    resource: String,

    /// Callback server port
    #[arg(short = 'p', long, default_value = "8080")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Set up logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let args = Args::parse();

    // Parse scopes
    let scopes: Vec<String> = if args.scopes.is_empty() {
        vec![]
    } else {
        args.scopes
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    };

    // Create OAuth configuration
    let oauth_config = OAuth2Config {
        client_id: args.client_id,
        client_secret: args.client_secret,
        auth_url: args.auth_url,
        token_url: args.token_url,
        redirect_url: args.redirect_url,
        resource: args.resource,
        scopes,
    };

    // Create OAuth client
    let mut oauth_client = OAuth2Client::new(oauth_config)?;

    // Get authorization URL
    let (auth_url, _csrf_token) = oauth_client.get_authorization_url();

    info!("Opening browser for authorization...");
    info!("If the browser doesn't open, visit: {}", auth_url);

    // Try to open the browser
    if let Err(e) = webbrowser::open(auth_url.as_str()) {
        eprintln!("Failed to open browser: {e}");
        eprintln!("Please visit the URL manually: {auth_url}");
    }

    // Start callback server
    let callback_server = OAuth2CallbackServer::new(args.port);
    info!("Waiting for OAuth callback on port {}...", args.port);

    let (code, state) = callback_server.wait_for_callback().await?;

    info!("Received authorization code, exchanging for token...");

    // Exchange code for token
    let _token = oauth_client.exchange_code(code, state).await?;
    info!("Successfully obtained access token");

    // Create MCP client with OAuth
    let oauth_client_arc = Arc::new(oauth_client);
    let mut client = Client::new("oauth-example", "1.0.0");

    info!(
        "Connecting to MCP server at {} with OAuth...",
        args.endpoint
    );
    let init_result = client
        .connect_http_with_oauth(&args.endpoint, oauth_client_arc)
        .await?;

    info!("Connected successfully!");
    info!("Server capabilities: {:?}", init_result.capabilities);
    info!("Server info: {:?}", init_result.server_info);

    // Example: List available tools
    let tools = client.list_tools(None).await?;
    info!("Available tools:");
    for tool in tools.tools {
        info!(
            "  - {}: {}",
            tool.name,
            tool.description.unwrap_or_default()
        );
    }

    // Example: List available resources
    let resources = client.list_resources(None).await?;
    info!("Available resources:");
    for resource in resources.resources {
        info!(
            "  - {}: {}",
            resource.uri,
            resource.description.unwrap_or_default()
        );
    }

    Ok(())
}
