//! Streamable HTTP transport for MCP.
//!
//! This module contains both halves of the HTTP transport: the client
//! transport ([`client::HttpClientTransport`]) used by [`crate::Client`] to
//! talk to remote MCP servers, and the server transport
//! ([`server::HttpServerTransport`]) that exposes an MCP server over axum.

mod client;
mod endpoint;
mod server;
mod validation;

pub use client::HttpClientTransport;
pub use endpoint::is_loopback_http_url;
pub use server::{CorsPolicy, EmbeddedHttpRoutes, HttpServer};

/// Normalize an HTTP endpoint path to a canonical form.
///
/// Empty or root paths become `/`; other paths gain a leading slash and lose
/// any trailing slashes.
pub fn normalize_endpoint_path(endpoint_path: impl Into<String>) -> String {
    let endpoint_path = endpoint_path.into();
    let trimmed = endpoint_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }

    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}
