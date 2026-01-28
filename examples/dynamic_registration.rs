//! MCP client example with dynamic OAuth registration.

use std::{error::Error, sync::Arc};

use clap::Parser;
use tmcp::{
    Client,
    auth::{
        ClientMetadata, DynamicRegistrationClient, OAuth2CallbackServer, OAuth2Client, OAuth2Config,
    },
};
use tracing::{Level, info, subscriber};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "MCP client with dynamic OAuth registration",
    long_about = None
)]
/// Command-line arguments.
struct Args {
    /// MCP server endpoint URL
    #[arg(short, long)]
    endpoint: String,

    /// OAuth authorization URL
    #[arg(short, long)]
    auth_url: String,

    /// OAuth token URL
    #[arg(short, long)]
    token_url: String,

    /// Registration endpoint (optional, will try to discover)
    #[arg(short = 'r', long)]
    registration_endpoint: Option<String>,

    /// Client name for registration
    #[arg(short = 'n', long, default_value = "MCP Dynamic Client")]
    client_name: String,

    /// OAuth callback port
    #[arg(short = 'p', long, default_value = "8080")]
    port: u16,

    /// OAuth scopes (comma-separated)
    #[arg(long, default_value = "")]
    scopes: String,

    /// Skip dynamic registration and use provided credentials
    #[arg(long)]
    client_id: Option<String>,

    /// Client secret (required if client_id is provided)
    #[arg(long)]
    client_secret: Option<String>,
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

    let redirect_url = format!("http://localhost:{}/callback", args.port);

    // Create or register OAuth client
    let mut oauth_client = if let (Some(client_id), Some(client_secret)) =
        (args.client_id, args.client_secret)
    {
        // Use provided credentials
        info!("Using provided OAuth credentials");

        let config = OAuth2Config {
            client_id,
            client_secret: Some(client_secret),
            auth_url: args.auth_url.clone(),
            token_url: args.token_url.clone(),
            redirect_url: redirect_url.clone(),
            resource: args.endpoint.clone(),
            scopes: scopes.clone(),
        };

        OAuth2Client::new(config)?
    } else {
        // Perform dynamic registration
        info!("Performing dynamic client registration...");

        // First, try direct registration
        match OAuth2Client::register_dynamic(
            args.auth_url.clone(),
            args.token_url.clone(),
            args.endpoint.clone(),
            args.client_name.clone(),
            redirect_url.clone(),
            scopes.clone(),
            args.registration_endpoint.clone(),
        )
        .await
        {
            Ok(client) => {
                info!("Successfully registered OAuth client dynamically");
                client
            }
            Err(e) => {
                // If dynamic registration fails, try manual registration
                info!("Dynamic registration failed: {}", e);
                info!("Attempting manual registration...");

                let registration_client = DynamicRegistrationClient::default();

                // Create metadata for manual registration
                let metadata = ClientMetadata::new(&args.client_name, &redirect_url)
                    .with_resource(&args.endpoint)
                    .with_scopes(&scopes)
                    .with_client_uri("https://github.com/your-org/your-client")
                    .with_contacts(vec!["admin@example.com".to_string()])
                    .with_software_info("tmcp-dynamic", env!("CARGO_PKG_VERSION"));

                info!("Registration metadata:");
                info!("{}", serde_json::to_string_pretty(&metadata)?);

                // If we have a registration endpoint, try it
                if let Some(reg_endpoint) = &args.registration_endpoint {
                    match registration_client
                        .register(reg_endpoint, metadata, None)
                        .await
                    {
                        Ok(response) => {
                            info!("Registration successful!");
                            info!("Client ID: {}", response.client_id);
                            if let Some(secret) = &response.client_secret {
                                info!("Client Secret: {}", secret);
                            }
                            info!("Save these credentials for future use");

                            // Create OAuth client from registration
                            let config = OAuth2Config::from_registration(
                                response,
                                args.auth_url.clone(),
                                args.token_url.clone(),
                                args.endpoint.clone(),
                            );

                            OAuth2Client::new(config)?
                        }
                        Err(e) => {
                            return Err(format!("Manual registration also failed: {e}").into());
                        }
                    }
                } else {
                    return Err("No registration endpoint available. Please provide OAuth credentials with --client-id and --client-secret".into());
                }
            }
        }
    };

    // Now perform OAuth flow
    info!("Starting OAuth authorization flow...");

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
    let mut client = Client::new("dynamic-registration-example", "1.0.0");

    info!("Connecting to MCP server at {}...", args.endpoint);
    let init_result = client
        .connect_http_with_oauth(&args.endpoint, oauth_client_arc)
        .await?;

    info!("Connected successfully!");
    info!(
        "Server info: {} v{}",
        init_result.server_info.name, init_result.server_info.version
    );
    info!("Protocol version: {}", init_result.protocol_version);

    // List available tools
    info!("\nListing available tools...");
    let tools = client.list_tools(None).await?;
    info!("Found {} tools", tools.tools.len());
    for tool in tools.tools.iter().take(5) {
        info!(
            "  - {}: {}",
            tool.name,
            tool.description
                .as_ref()
                .unwrap_or(&"(no description)".to_string())
        );
    }
    if tools.tools.len() > 5 {
        info!("  ... and {} more", tools.tools.len() - 5);
    }

    Ok(())
}
