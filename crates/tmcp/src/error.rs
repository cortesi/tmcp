use std::{io, result::Result as StdResult};

use serde_json::Value;
use thiserror::Error;

use crate::schema::{
    CallToolResult, ErrorObject, INVALID_PARAMS, INVALID_REQUEST, JSONRPC_VERSION,
    JSONRPCErrorResponse, METHOD_NOT_FOUND, PARSE_ERROR, RequestId,
};

#[derive(Error, Debug, Clone)]
/// Error type for MCP operations.
pub enum Error {
    /// I/O error with a message.
    #[error("IO error: {message}")]
    Io {
        /// Error message details.
        message: String,
    },

    /// JSON serialization or parsing error.
    #[error("JSON serialization error: {message}")]
    JsonParse {
        /// Error message details.
        message: String,
    },

    /// Transport-layer error.
    #[error("Transport error: {0}")]
    Transport(String),

    /// Transport disconnected unexpectedly.
    #[error("Transport disconnected unexpectedly")]
    TransportDisconnected,

    /// Protocol-level error.
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// Invalid request error.
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Method not found error.
    #[error("Method not found: {0}")]
    MethodNotFound(String),

    /// Invalid parameters error.
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    InternalError(String),

    /// Connection closed.
    #[error("Connection closed")]
    ConnectionClosed,

    /// Handler error with type context.
    #[error("Handler error for {handler_type}: {message}")]
    HandlerError {
        /// Handler type name.
        handler_type: String,
        /// Error message.
        message: String,
    },

    /// Resource not found error.
    #[error("Resource not found: {uri}")]
    ResourceNotFound {
        /// Missing resource URI.
        uri: String,
    },

    /// Tool execution failed error.
    #[error("Tool execution failed for '{tool}': {message}")]
    ToolExecutionFailed {
        /// Tool name that failed.
        tool: String,
        /// Error message details.
        message: String,
    },

    /// Invalid message format error.
    #[error("Invalid message format: {message}")]
    InvalidMessageFormat {
        /// Error message details.
        message: String,
    },

    /// Tool not found error.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Tool group not found error.
    #[error("Tool group not found: {0}")]
    GroupNotFound(String),

    /// Tool group requires its parent to be active.
    #[error("Tool group '{group}' requires active parent '{parent}'")]
    GroupInactive {
        /// Child group name.
        group: String,
        /// Parent group name.
        parent: String,
    },

    /// Invalid configuration error.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Authorization failed error.
    #[error("Authorization failed: {0}")]
    AuthorizationFailed(String),

    /// Request timed out.
    #[error("Request timed out after {timeout_ms}ms: {request_id}")]
    Timeout {
        /// The request ID that timed out.
        request_id: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },

    /// Request was cancelled.
    #[error("Request cancelled: {request_id}")]
    Cancelled {
        /// The request ID that was cancelled.
        request_id: String,
    },
}

#[derive(Debug, Clone, Error)]
/// Error type for tool calls that should be surfaced as tool results.
#[error("{code}: {message}")]
pub struct ToolError {
    /// Error code identifier for the tool failure.
    pub code: &'static str,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured payload to return with the error.
    pub structured: Option<Value>,
}

/// Standard error code for invalid input parameters.
pub const TOOL_ERROR_INVALID_INPUT: &str = "INVALID_INPUT";
/// Standard error code for resource not found.
pub const TOOL_ERROR_NOT_FOUND: &str = "NOT_FOUND";
/// Standard error code for operation timeout.
pub const TOOL_ERROR_TIMEOUT: &str = "TIMEOUT";
/// Standard error code for internal errors.
pub const TOOL_ERROR_INTERNAL: &str = "INTERNAL";

impl ToolError {
    /// Build a new tool error with a code and message.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            structured: None,
        }
    }

    /// Attach structured content to the tool error.
    pub fn with_structured(mut self, structured: Value) -> Self {
        self.structured = Some(structured);
        self
    }

    /// Create an invalid input error for validation failures.
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(TOOL_ERROR_INVALID_INPUT, message)
    }

    /// Create a not found error when a resource or target cannot be located.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(TOOL_ERROR_NOT_FOUND, message)
    }

    /// Create a timeout error when an operation exceeds its time limit.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(TOOL_ERROR_TIMEOUT, message)
    }

    /// Create an internal error for unexpected failures.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(TOOL_ERROR_INTERNAL, message)
    }
}

impl From<ToolError> for CallToolResult {
    fn from(err: ToolError) -> Self {
        let mut result = Self::error(err.code, err.message);
        if let Some(structured) = err.structured {
            result = result.with_structured_content(structured);
        }
        result
    }
}

impl Error {
    /// Create a HandlerError with type context
    pub fn handler_error(handler_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self::HandlerError {
            handler_type: handler_type.into(),
            message: message.into(),
        }
    }

    /// Create a ToolExecutionFailed error
    pub fn tool_execution_failed(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ToolExecutionFailed {
            tool: tool.into(),
            message: message.into(),
        }
    }

    /// Convert error to a specific JSONRPC response if applicable
    pub(crate) fn to_jsonrpc_response(
        &self,
        request_id: RequestId,
    ) -> Option<JSONRPCErrorResponse> {
        let (code, message) = match self {
            Self::ToolNotFound(tool_name) => {
                (METHOD_NOT_FOUND, format!("Tool not found: {tool_name}"))
            }
            Self::MethodNotFound(method_name) => {
                (METHOD_NOT_FOUND, format!("Method not found: {method_name}"))
            }
            Self::InvalidParams(message) => {
                (INVALID_PARAMS, format!("Invalid parameters: {message}"))
            }
            Self::InvalidRequest(msg) => (INVALID_REQUEST, format!("Invalid request: {msg}")),
            Self::GroupNotFound(group) => (INVALID_PARAMS, format!("Group not found: {group}")),
            Self::GroupInactive { group, parent } => (
                INVALID_PARAMS,
                format!("Group '{group}' requires active parent '{parent}'"),
            ),
            Self::JsonParse { message } => {
                (PARSE_ERROR, format!("JSON serialization error: {message}"))
            }
            Self::InvalidMessageFormat { message } => {
                (PARSE_ERROR, format!("Invalid message format: {message}"))
            }
            // Return None for errors that should use the generic INTERNAL_ERROR handling
            _ => return None,
        };

        Some(JSONRPCErrorResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(request_id),
            error: ErrorObject {
                code,
                message,
                data: None,
            },
        })
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io {
            message: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::JsonParse {
            message: err.to_string(),
        }
    }
}

/// Result alias using the crate error type.
pub type Result<T> = StdResult<T, Error>;

/// Result alias for tool calls that return structured tool errors.
pub type ToolResult<T = CallToolResult> = StdResult<T, ToolError>;
