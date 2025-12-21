use serde::{Deserialize, Serialize};

/// Describes the name and version of an MCP implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    /// The name of the MCP implementation.
    pub name: String,
    pub version: String,

    /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Implementation {
    /// Create a new Implementation with the given name and version
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
        }
    }

    /// Set the title for the implementation
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}
