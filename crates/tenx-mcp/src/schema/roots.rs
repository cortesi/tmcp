#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

use crate::macros::with_meta;

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRootsResult {
    pub roots: Vec<Root>,
}

#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
