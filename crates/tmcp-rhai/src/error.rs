use std::sync::Arc;

use rhai::{EvalAltResult, ParseError, Position};
use serde::{Deserialize, Serialize};
use tmcp::schema::{CallToolResult, ContentBlock};

/// Script location metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptLocation {
    /// Line number in the script source.
    pub line: usize,
    /// Column number in the script source, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

/// Assertion metadata captured during script execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptAssertion {
    /// Whether the assertion passed.
    pub passed: bool,
    /// Assertion message.
    pub message: String,
    /// Location string for the assertion.
    pub location: String,
}

/// Timing metadata for a script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptTiming {
    /// Time spent compiling the script (ms).
    pub compile_ms: u64,
    /// Time spent executing the script (ms).
    pub exec_ms: u64,
    /// Total time spent in evaluation (ms).
    pub total_ms: u64,
}

impl ScriptTiming {
    /// Build a zeroed timing record.
    pub fn zero() -> Self {
        Self {
            compile_ms: 0,
            exec_ms: 0,
            total_ms: 0,
        }
    }
}

/// Serializable error details for script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptErrorInfo {
    /// Short error category.
    #[serde(rename = "type")]
    pub error_type: String,
    /// Human-readable error message.
    pub message: String,
    /// Location in the script, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<ScriptLocation>,
    /// Captured backtrace frames, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backtrace: Option<Vec<String>>,
    /// Optional source snippet or error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Structured error details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Image metadata emitted from script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptImageInfo {
    /// Image identifier.
    pub id: String,
    /// Index of the image content block in the tool result.
    pub content_index: usize,
    /// Image kind identifier.
    pub kind: String,
    /// Optional viewport identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewport_id: Option<String>,
    /// Optional target payload associated with the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    /// Optional rectangle payload associated with the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<serde_json::Value>,
    /// Additional structured metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Structured value returned from script evaluation.
#[derive(Debug, Clone, Default)]
pub struct ScriptValue {
    /// JSON value returned by the script.
    pub value: Option<serde_json::Value>,
    /// Image metadata emitted by the script.
    pub images: Option<Vec<ScriptImageInfo>>,
    /// Additional tool content blocks to append.
    pub content: Vec<ContentBlock>,
}

/// Serializable outcome of a script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEvalOutcome {
    /// Whether the script completed successfully.
    pub success: bool,
    /// JSON value returned by the script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// Image metadata emitted by the script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ScriptImageInfo>>,
    /// Collected log lines from the script.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
    /// Assertions captured during execution.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<ScriptAssertion>,
    /// Timing metadata for the evaluation.
    pub timing: ScriptTiming,
    /// Error details when evaluation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptErrorInfo>,
    #[serde(skip)]
    pub(crate) content: Vec<ContentBlock>,
}

impl ScriptEvalOutcome {
    /// Convert the outcome into a TMCP tool result payload.
    pub fn to_tool_result(&self) -> CallToolResult {
        let mut result = match CallToolResult::new().with_json_text(self) {
            Ok(res) => res,
            Err(err) => {
                let fallback = serde_json::json!({
                    "success": false,
                    "value": null,
                    "logs": [],
                    "assertions": [],
                    "timing": ScriptTiming::zero(),
                    "error": {
                        "type": "runtime",
                        "message": format!("Failed to serialize output: {err}"),
                    },
                });
                CallToolResult::new()
                    .with_json_text(&fallback)
                    .unwrap_or_else(|_| {
                        CallToolResult::new().with_text_content(
                            r#"{"success":false,"error":{"type":"runtime","message":"Failed to serialize output"}}"#
                        )
                    })
            }
        };
        for block in &self.content {
            result = result.with_content(block.clone());
        }
        result
    }

    /// Build a minimal error-only outcome.
    pub fn error_only(error: ScriptErrorInfo) -> Self {
        Self {
            success: false,
            value: None,
            images: None,
            logs: Vec::new(),
            assertions: Vec::new(),
            timing: ScriptTiming::zero(),
            error: Some(error),
            content: Vec::new(),
        }
    }
}

impl From<ScriptEvalOutcome> for CallToolResult {
    fn from(outcome: ScriptEvalOutcome) -> Self {
        outcome.to_tool_result()
    }
}

/// Result type for script execution.
pub type ScriptResult<T> = Result<T, ScriptError>;
/// Result type for structured script error information.
pub type ScriptInfoResult<T> = Result<T, Box<ScriptErrorInfo>>;

/// Errors that can occur during script evaluation.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ScriptError {
    /// The script exceeded the configured timeout.
    #[error("Script timed out after {ms}ms")]
    Timeout {
        /// Timeout duration in milliseconds.
        ms: u64,
    },
    /// The script failed to parse.
    #[error("Parse error: {0}")]
    Parse(ParseError),
    /// The script failed at runtime.
    #[error("Runtime error: {0}")]
    Runtime(Arc<EvalAltResult>),
    /// A custom error emitted by the caller.
    #[error("{0}")]
    Custom(String),
}

/// Convert a script error to a structured, serializable form.
pub fn format_error(err: &ScriptError) -> ScriptErrorInfo {
    match err {
        ScriptError::Timeout { ms } => ScriptErrorInfo {
            error_type: "timeout".to_string(),
            message: format!("Script timed out after {}ms", ms),
            location: None,
            backtrace: None,
            code: None,
            details: None,
        },
        ScriptError::Parse(err) => ScriptErrorInfo {
            error_type: "parse".to_string(),
            message: err.to_string(),
            location: format_location(err.position()),
            backtrace: None,
            code: None,
            details: None,
        },
        ScriptError::Runtime(err) => ScriptErrorInfo {
            error_type: "runtime".to_string(),
            message: err.to_string(),
            location: format_location(err.position()),
            backtrace: collect_backtrace(err),
            code: None,
            details: None,
        },
        ScriptError::Custom(msg) => ScriptErrorInfo {
            error_type: "custom".to_string(),
            message: msg.clone(),
            location: None,
            backtrace: None,
            code: None,
            details: None,
        },
    }
}

fn format_location(pos: Position) -> Option<ScriptLocation> {
    let line = pos.line()?;
    Some(ScriptLocation {
        line,
        column: pos.position(),
    })
}

fn collect_backtrace(error: &EvalAltResult) -> Option<Vec<String>> {
    let mut frames = Vec::new();
    if let Some(line) = error.position().line() {
        frames.push(format!("at <main> (line {line})"));
    }
    if frames.is_empty() {
        None
    } else {
        Some(frames)
    }
}
