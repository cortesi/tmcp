use std::sync::Arc;

use rhai::{EvalAltResult, ParseError, Position};
use serde::{Deserialize, Serialize};
use tmcp::schema::CallToolResult;

/// Serializable error details for script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptErrorInfo {
    /// Short error category.
    pub error_type: String,
    /// Human-readable error message.
    pub message: String,
    /// Location in the script, when available.
    pub location: Option<String>,
    /// Captured backtrace frames, when available.
    pub backtrace: Option<Vec<String>>,
    /// Optional source snippet.
    pub code: Option<String>,
    /// Structured error details.
    pub details: Option<serde_json::Value>,
}

/// Serializable outcome of a script evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptEvalOutcome {
    /// JSON value returned by the script.
    pub value: Option<serde_json::Value>,
    /// Collected log lines from `print`.
    pub logs: Vec<String>,
    /// Error details when evaluation failed.
    pub error: Option<ScriptErrorInfo>,
}

impl ScriptEvalOutcome {
    /// Convert the outcome into a TMCP tool result payload.
    pub fn to_tool_result(&self) -> CallToolResult {
        let json = match serde_json::to_string(self) {
            Ok(json) => json,
            Err(err) => {
                let fallback = serde_json::json!({
                    "value": null,
                    "logs": [],
                    "error": {
                        "error_type": "runtime",
                        "message": format!("Failed to serialize output: {err}"),
                    },
                });
                serde_json::to_string(&fallback).unwrap_or_else(|_| {
                    r#"{"value":null,"logs":[],"error":{"error_type":"runtime","message":"Failed to serialize output"}}"#.to_string()
                })
            }
        };
        CallToolResult::new().with_text_content(json)
    }
}

impl From<ScriptEvalOutcome> for CallToolResult {
    fn from(outcome: ScriptEvalOutcome) -> Self {
        outcome.to_tool_result()
    }
}

/// Result type for script execution.
pub type ScriptResult<T> = Result<T, ScriptError>;

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

fn format_location(pos: Position) -> Option<String> {
    if pos.is_none() {
        None
    } else {
        Some(format!("line {}", pos.line().unwrap_or(0)))
    }
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
