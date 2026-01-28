//! Example for registering an OAuth client dynamically.

use std::error::Error;

use clap::Parser;
use tmcp::auth::{ClientMetadata, DynamicRegistrationClient, OAuth2Config};
use tracing::{Level, info, subscriber};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Register an OAuth client dynamically",
    long_about = None
)]
/// Command-line arguments.
struct Args {
    /// Registration endpoint URL
    #[arg(short, long)]
    registration_endpoint: String,

    /// Client name
    #[arg(short = 'n', long, default_value = "MCP Test Client")]
    client_name: String,

    /// Redirect URI
    #[arg(short = 'u', long, default_value = "http://localhost:8080/callback")]
    redirect_uri: String,

    /// OAuth authorization URL (for the registered client)
    #[arg(short, long)]
    auth_url: String,

    /// OAuth token URL (for the registered client)
    #[arg(short, long)]
    token_url: String,

    /// Resource URL (MCP server endpoint)
    #[arg(short = 'r', long)]
    resource: String,

    /// OAuth scopes (comma-separated)
    #[arg(long, default_value = "")]
    scopes: String,

    /// Access token for protected registration endpoints
    #[arg(short = 'a', long)]
    access_token: Option<String>,

    /// Additional contacts (comma-separated emails)
    #[arg(short = 'c', long)]
    contacts: Option<String>,

    /// Client URI (website)
    #[arg(long)]
    client_uri: Option<String>,

    /// Token endpoint auth method
    #[arg(long, default_value = "client_secret_basic")]
    token_endpoint_auth_method: String,
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

    // Parse contacts
    let contacts: Option<Vec<String>> = args
        .contacts
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect());

    // Create registration client
    let registration_client = DynamicRegistrationClient::default();

    // Create client metadata
    let mut metadata = ClientMetadata::new(&args.client_name, &args.redirect_uri)
        .with_resource(&args.resource)
        .with_scopes(&scopes)
        .with_token_endpoint_auth_method(&args.token_endpoint_auth_method)
        .with_software_info("tmcp", env!("CARGO_PKG_VERSION"));

    if let Some(contacts) = contacts {
        metadata = metadata.with_contacts(contacts);
    }

    if let Some(client_uri) = args.client_uri {
        metadata = metadata.with_client_uri(client_uri);
    }

    info!("Registering OAuth client...");
    info!("Registration endpoint: {}", args.registration_endpoint);
    info!("Client metadata:");
    info!("{}", serde_json::to_string_pretty(&metadata)?);

    // Perform registration
    match registration_client
        .register(
            &args.registration_endpoint,
            metadata,
            args.access_token.as_deref(),
        )
        .await
    {
        Ok(response) => {
            info!("\n✅ Registration successful!");
            info!("");
            info!("Client ID: {}", response.client_id);

            if let Some(secret) = &response.client_secret {
                info!("Client Secret: {}", secret);
            }

            if let Some(issued_at) = response.client_id_issued_at {
                let issued = chrono::DateTime::from_timestamp(issued_at as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| format!("{issued_at} (timestamp)"));
                info!("Issued at: {}", issued);
            }

            if let Some(expires_at) = response.client_secret_expires_at {
                if expires_at == 0 {
                    info!("Secret expires: Never");
                } else {
                    let expires = chrono::DateTime::from_timestamp(expires_at as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| format!("{expires_at} (timestamp)"));
                    info!("Secret expires: {}", expires);
                }
            }

            // Create OAuth2Config from the registration
            let config = OAuth2Config::from_registration(
                response,
                args.auth_url,
                args.token_url,
                args.resource,
            );

            info!("\n📋 OAuth2 Configuration:");
            info!("You can now use these credentials to connect to the MCP server:");
            info!("");
            info!("tmcp::OAuth2Config {{");
            info!("    client_id: \"{}\".to_string(),", config.client_id);
            if let Some(secret) = &config.client_secret {
                info!("    client_secret: Some(\"{}\".to_string()),", secret);
            } else {
                info!("    client_secret: None,");
            }
            info!("    auth_url: \"{}\".to_string(),", config.auth_url);
            info!("    token_url: \"{}\".to_string(),", config.token_url);
            info!("    redirect_url: \"{}\".to_string(),", config.redirect_url);
            info!("    resource: \"{}\".to_string(),", config.resource);
            info!(
                "    scopes: vec![{}],",
                config
                    .scopes
                    .iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            info!("}}");

            info!("\n💡 Next steps:");
            info!("1. Save these credentials securely");
            info!("2. Use them with tmcp::OAuth2Client::new(config)");
            info!("3. Connect to your MCP server with OAuth authentication");
        }
        Err(e) => {
            eprintln!("\n❌ Registration failed: {e}");
            eprintln!();
            eprintln!("Common issues:");
            eprintln!("- Invalid registration endpoint URL");
            eprintln!("- Registration endpoint requires authentication (use --access-token)");
            eprintln!("- Client metadata doesn't meet server requirements");
            eprintln!("- Server doesn't support dynamic registration");
            return Err(e.into());
        }
    }

    Ok(())
}
