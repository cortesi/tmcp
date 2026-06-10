use serde::{Deserialize, Serialize};

use crate::macros::{with_meta, with_open_meta};

/// The client's response to a roots/list request from the server.
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRootsResult {
    /// The roots the client exposes to the server.
    pub roots: Vec<Root>,
}

/// A root directory or file that the server can operate on.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    /// The URI identifying the root, conventionally a `file://` URI.
    pub uri: String,
    /// Optional human-readable name for the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
