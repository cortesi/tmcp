//! # tmcp-http
//!
//! The streamable HTTP transport of the Model Context Protocol (MCP) for the
//! `tmcp` runtime, with OAuth 2.1 for clients and bearer-token protection for
//! servers.
//!
//! [`ServerHttpExt`] serves a [`tmcp::Server`] over HTTP, or embeds it into an
//! existing Axum application. [`ClientHttpExt`] connects a [`tmcp::Client`] to
//! an HTTP endpoint, with or without OAuth. The crate has no features.

#![warn(missing_docs)]

/// OAuth and authorization helpers.
pub mod auth;
/// The HTTP builder of a server.
mod builder;
/// The HTTP connects of a client.
mod connect;
/// HTTP transport implementation.
mod http;

pub use builder::{EmbeddedHttpServer, HttpBuilder, ServerHttpExt};
pub use connect::ClientHttpExt;
pub use http::{CorsPolicy, HttpClientTransport};
// The protocol types, under the path that the transport modules use.
use tmcp::schema;
