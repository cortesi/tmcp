use serde::{Deserialize, Serialize};

use crate::macros::with_open_meta;

/// The argument a completion request is asking about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgumentInfo {
    /// The name of the argument being completed.
    pub name: String,
    /// The value entered so far for the argument.
    pub value: String,
}

/// The subject of a completion request: a resource template or a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Reference {
    /// A reference to a resource template.
    #[serde(rename = "ref/resource")]
    Resource(ResourceTemplateReference),
    /// A reference to a prompt.
    #[serde(rename = "ref/prompt")]
    Prompt(PromptReference),
}

/// Identifies a resource template by its URI template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTemplateReference {
    /// The URI template of the resource.
    pub uri: String,
}

/// Identifies a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptReference {
    /// Intended for programmatic or logical use, but used as a display name in
    /// past specs or fallback (if title isn't present).
    pub name: String,
    /// Intended for UI and end-user contexts — optimized to be human-readable
    /// and easily understood, even by those unfamiliar with domain-specific
    /// terminology.
    ///
    /// If not provided, the name should be used for display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The server's response to a completion/complete request.
#[with_open_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteResult {
    /// The completion values and pagination hints.
    pub completion: CompletionInfo,
}

/// Completion values offered for an argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionInfo {
    /// Suggested completion values, at most 100.
    pub values: Vec<String>,
    /// Total number of available matches, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// Whether more matches exist beyond those returned.
    #[serde(rename = "hasMore", skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}
