//! MCP protocol schema types.
//!
//! This module contains the data structures defined by the Model Context Protocol (MCP)
//! specification. These types represent the protocol's message formats, capabilities,
//! and data payloads.
//!
//! For detailed semantics of each type, refer to the
//! [MCP specification](https://spec.modelcontextprotocol.io/).
#![allow(missing_docs)]

/// JSON-RPC 2.0 message types and constants for the MCP protocol.
mod jsonrpc;
/// Request and notification types for client-server communication.
mod requests;

/// Capability types and helpers.
mod capabilities;
/// Completion request/response types.
mod completions;
/// Content payload types.
mod content;
/// Icon metadata types.
mod icons;
/// Implementation metadata.
mod implementation;
/// Initialization types.
mod initialization;
/// Logging types.
mod logging;
/// Prompt types.
mod prompts;
/// Resource types and helpers.
mod resources;
/// Root listing types.
mod roots;
/// Sampling types.
mod sampling;
/// Task types and helpers.
mod tasks;
/// Tool schema types.
mod tools;

pub use capabilities::*;
pub use completions::*;
pub use content::*;
pub use icons::*;
pub use implementation::*;
pub use initialization::*;
pub use jsonrpc::*;
pub use logging::*;
pub use prompts::*;
pub use requests::*;
pub use resources::*;
pub use roots::*;
pub use sampling::*;
pub use tasks::*;
pub use tools::*;
