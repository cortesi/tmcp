//! Example demonstrating how to connect using generic AsyncRead/AsyncWrite streams
//!
//! This example shows how to use the connect_stream() method with various
//! types of streams, not just process stdio.

use tmcp::{Client, Result};
use tokio::io::{AsyncRead, AsyncWrite, duplex};
use tracing::{Level, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    // Example 1: Using duplex streams (useful for testing)
    example_duplex_streams().await?;

    // Example 2: Using custom streams
    example_custom_streams().await?;

    Ok(())
}

/// Demonstrate connecting with in-memory duplex streams.
async fn example_duplex_streams() -> Result<()> {
    info!("Example 1: Connecting with duplex streams");

    // Create bidirectional duplex streams
    // In a real scenario, these might be connected to another process or service
    let (client_reader, _server_writer) = duplex(8192);
    let (_server_reader, client_writer) = duplex(8192);

    let mut client = Client::new("stream-example", "0.1.0");

    // Connect using the streams
    client.connect_stream(client_reader, client_writer).await?;

    info!("Connected via duplex streams");

    // Note: This example won't actually work without a server on the other end
    // It's just demonstrating the API

    Ok(())
}

/// Demonstrate connecting with custom stream implementations.
async fn example_custom_streams() -> Result<()> {
    info!("Example 2: Connecting with custom stream types");

    // You can use any types that implement AsyncRead + AsyncWrite
    // For example, you might have:
    // - Network streams (TcpStream, UnixStream)
    // - File-based streams
    // - Encrypted streams
    // - Compressed streams
    // - Custom protocol wrappers

    // Here's a hypothetical example with a custom stream type
    let reader = create_custom_reader();
    let writer = create_custom_writer();

    let mut client = Client::new("stream-example", "0.1.0");
    client.connect_stream(reader, writer).await?;

    info!("Connected via custom streams");

    Ok(())
}

// Placeholder functions to demonstrate the concept
/// Create a dummy AsyncRead implementation for the example.
fn create_custom_reader() -> Box<dyn AsyncRead + Send + Sync + Unpin> {
    // In a real implementation, this might return:
    // - A TLS stream reader
    // - A compressed stream reader
    // - A custom protocol reader
    // etc.
    let (reader, _) = duplex(8192);
    Box::new(reader)
}

/// Create a dummy AsyncWrite implementation for the example.
fn create_custom_writer() -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
    // In a real implementation, this might return:
    // - A TLS stream writer
    // - A compressed stream writer
    // - A custom protocol writer
    // etc.
    let (_, writer) = duplex(8192);
    Box::new(writer)
}
