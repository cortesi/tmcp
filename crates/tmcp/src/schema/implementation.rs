use serde::{Deserialize, Serialize};

use super::Icon;

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

    /// An optional human-readable description of what this implementation does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// An optional URL of the website for this implementation.
    #[serde(rename = "websiteUrl", skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,

    /// Optional set of sized icons that the client can display in a user interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

impl Implementation {
    /// Create a new Implementation with the given name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
            description: None,
            website_url: None,
            icons: None,
        }
    }

    /// Set the title for the implementation.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the description for the implementation.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the website URL for the implementation.
    pub fn with_website_url(mut self, website_url: impl Into<String>) -> Self {
        self.website_url = Some(website_url.into());
        self
    }

    /// Set the icon list for the implementation.
    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }
}
