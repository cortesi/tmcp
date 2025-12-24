use serde::{Deserialize, Serialize};

/// An optionally-sized icon that can be displayed in a user interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    /// A standard URI pointing to an icon resource.
    pub src: String,
    /// Optional MIME type override if the source MIME type is missing or generic.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional array of strings that specify sizes at which the icon can be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<String>>,
    /// Optional specifier for the theme this icon is designed for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<IconTheme>,
}

impl Icon {
    /// Create a new icon with a source URI.
    pub fn new(src: impl Into<String>) -> Self {
        Self {
            src: src.into(),
            mime_type: None,
            sizes: None,
            theme: None,
        }
    }

    /// Set the MIME type override for the icon.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Set the supported sizes for the icon.
    pub fn with_sizes(mut self, sizes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.sizes = Some(sizes.into_iter().map(Into::into).collect());
        self
    }

    /// Set the theme for the icon.
    pub fn with_theme(mut self, theme: IconTheme) -> Self {
        self.theme = Some(theme);
        self
    }
}

/// Theme variant for icons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTheme {
    Light,
    Dark,
}
