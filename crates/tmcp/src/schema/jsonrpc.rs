use std::{
    collections::HashSet,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::macros::with_meta;

/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// A released MCP protocol version in `YYYY-MM-DD` form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProtocolVersion(String);

impl ProtocolVersion {
    /// Return the protocol version string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ProtocolVersion {
    type Err = ProtocolVersionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if valid_release_date(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(ProtocolVersionError {
                value: value.to_owned(),
            })
        }
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Failure to parse an MCP protocol release date.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid MCP protocol version `{value}`; expected a valid YYYY-MM-DD release date")]
pub struct ProtocolVersionError {
    /// Invalid input value.
    value: String,
}

impl ProtocolVersionError {
    /// Return the invalid input value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// An ordered, non-empty set of supported MCP protocol versions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupportedProtocolVersions {
    /// Versions in preference order.
    versions: Vec<ProtocolVersion>,
}

impl SupportedProtocolVersions {
    /// Create a supported-version set in preference order.
    ///
    /// # Errors
    ///
    /// Returns an error if `versions` is empty or contains a duplicate.
    pub fn new(
        versions: impl IntoIterator<Item = ProtocolVersion>,
    ) -> Result<Self, SupportedProtocolVersionsError> {
        let versions: Vec<_> = versions.into_iter().collect();
        if versions.is_empty() {
            return Err(SupportedProtocolVersionsError::Empty);
        }

        let mut unique = HashSet::with_capacity(versions.len());
        for version in &versions {
            if !unique.insert(version) {
                return Err(SupportedProtocolVersionsError::Duplicate(version.clone()));
            }
        }
        Ok(Self { versions })
    }

    /// Return the first client-preferred version.
    pub fn preferred(&self) -> &ProtocolVersion {
        &self.versions[0]
    }

    /// Return the server's latest supported version.
    pub fn latest(&self) -> &ProtocolVersion {
        &self.versions[0]
    }

    /// Return whether this set contains `version`.
    pub fn contains(&self, version: &ProtocolVersion) -> bool {
        self.versions.contains(version)
    }
}

impl Default for SupportedProtocolVersions {
    fn default() -> Self {
        Self {
            versions: ["2025-11-25", "2025-06-18", "2025-03-26"]
                .into_iter()
                .map(|version| version.parse().expect("default MCP version must be valid"))
                .collect(),
        }
    }
}

/// Failure to construct a supported MCP protocol version set.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SupportedProtocolVersionsError {
    /// The configured version set was empty.
    #[error("supported MCP protocol versions cannot be empty")]
    Empty,
    /// The configured version set contained a duplicate.
    #[error("duplicate supported MCP protocol version `{0}`")]
    Duplicate(ProtocolVersion),
}

/// Return whether `value` is a valid Gregorian date in `YYYY-MM-DD` form.
fn valid_release_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }

    let year = value[..4].parse::<u16>().ok();
    let month = value[5..7].parse::<u8>().ok();
    let day = value[8..].parse::<u8>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }

    let days = match month {
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

/// Refers to any valid JSON-RPC object that can be decoded off the wire, or
/// encoded to be sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCMessage {
    /// A request expecting a response.
    Request(JSONRPCRequest),
    /// A notification with no response.
    Notification(JSONRPCNotification),
    /// A response to a request.
    Response(JSONRPCResponse),
}

/// A progress token, used to associate progress notifications with the original
/// request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    /// String progress token.
    String(String),
    /// Numeric progress token.
    Number(i64),
}

/// An opaque token used to represent a cursor for pagination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct Cursor(pub String);

impl From<&str> for Cursor {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for Cursor {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&String> for Cursor {
    fn from(s: &String) -> Self {
        Self(s.clone())
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Common params for any request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestParams {
    /// Request metadata reserved by the protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<RequestMeta>,
    /// Method-specific parameters.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// Metadata attached to a request via the reserved `_meta` parameter.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestMeta {
    /// If specified, the caller is requesting out-of-band progress
    /// notifications for this request (as represented by
    /// notifications/progress). The value of this parameter is an opaque token
    /// that will be attached to any subsequent notifications. The receiver is
    /// not obligated to provide these notifications.
    #[serde(rename = "progressToken", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,
    /// Additional metadata fields preserved from the wire.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The method and params of a JSON-RPC request, without the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// The method being invoked.
    pub method: String,
    /// Parameters for the method, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<RequestParams>,
}

/// Common params for any notification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationParams {
    /// This parameter name is reserved by MCP to allow clients and servers to
    /// attach additional metadata to their notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _meta: Option<Map<String, Value>>,
    /// Method-specific parameters.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The method and params of a JSON-RPC notification, without the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// The notification method.
    pub method: String,
    /// Parameters for the notification, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<NotificationParams>,
}

/// The payload of a successful JSON-RPC response.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResult {
    /// This result property is reserved by the protocol to allow clients and
    /// servers to attach additional metadata to their responses.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A uniquely identifying ID for a request in JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    /// String request ID.
    String(String),
    /// Numeric request ID.
    Number(i64),
}

impl Display for RequestId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{s}"),
            Self::Number(n) => write!(f, "{n}"),
        }
    }
}

/// A request that expects a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCRequest {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier correlating the request with its response.
    pub id: RequestId,
    /// The method and params of the request.
    #[serde(flatten)]
    pub request: Request,
}

/// A notification which does not expect a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCNotification {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// The method and params of the notification.
    #[serde(flatten)]
    pub notification: Notification,
}

/// A successful (non-error) response to a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCResultResponse {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier of the request this responds to.
    pub id: RequestId,
    /// The result payload.
    pub result: JSONRPCResult,
}

/// A response to a request that indicates an error occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JSONRPCErrorResponse {
    /// JSON-RPC protocol version, always "2.0".
    pub jsonrpc: String,
    /// Identifier of the request this responds to, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Details of the error.
    pub error: ErrorObject,
}

/// A response to a request, containing either the result or error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JSONRPCResponse {
    /// A successful response.
    Result(JSONRPCResultResponse),
    /// An error response.
    Error(JSONRPCErrorResponse),
}

// Standard JSON-RPC error codes
/// JSON-RPC parse error code.
pub const PARSE_ERROR: i32 = -32700;
/// JSON-RPC invalid request error code.
pub const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC method not found error code.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC invalid params error code.
pub const INVALID_PARAMS: i32 = -32602;
/// JSON-RPC internal error code.
pub const INTERNAL_ERROR: i32 = -32603;

/// MCP-specific JSON-RPC error code indicating a requested resource was not found.
pub const RESOURCE_NOT_FOUND: i32 = -32002;

/// Implementation-specific JSON-RPC error code indicating URL elicitation is required.
pub const URL_ELICITATION_REQUIRED: i32 = -32042;
/// Implementation-specific JSON-RPC error code indicating authorization failed.
pub const AUTHORIZATION_FAILED: i32 = -32041;

/// The error payload of a JSON-RPC error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    /// The error type that occurred.
    pub code: i32,
    /// A short description of the error. The message SHOULD be limited to a
    /// concise single sentence.
    pub message: String,
    /// Additional information about the error. The value of this member is
    /// defined by the sender (e.g. detailed error information, nested
    /// errors etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A response that indicates success but carries no data.
pub type EmptyResult = JSONRPCResult;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_validates_parse_and_deserialize() {
        let version: ProtocolVersion = "2025-06-18".parse().expect("valid version");
        assert_eq!(version.as_str(), "2025-06-18");
        assert_eq!(serde_json::to_value(&version).unwrap(), "2025-06-18");
        assert_eq!(
            serde_json::from_str::<ProtocolVersion>(r#""2024-02-29""#)
                .unwrap()
                .as_str(),
            "2024-02-29"
        );

        for invalid in ["2025-6-18", "2025-02-29", "0000-01-01", "not-a-date"] {
            let error = invalid.parse::<ProtocolVersion>().unwrap_err();
            assert_eq!(error.value(), invalid);
            assert!(
                serde_json::from_value::<ProtocolVersion>(Value::String(invalid.to_owned()))
                    .is_err()
            );
        }
    }

    #[test]
    fn supported_versions_are_ordered_non_empty_and_unique() {
        let defaults = SupportedProtocolVersions::default();
        assert_eq!(defaults.preferred().as_str(), "2025-11-25");
        assert_eq!(defaults.latest().as_str(), "2025-11-25");
        assert!(defaults.contains(&"2025-03-26".parse().unwrap()));

        assert_eq!(
            SupportedProtocolVersions::new([]).unwrap_err(),
            SupportedProtocolVersionsError::Empty
        );
        let duplicate: ProtocolVersion = "2025-06-18".parse().unwrap();
        assert_eq!(
            SupportedProtocolVersions::new([duplicate.clone(), duplicate.clone()]).unwrap_err(),
            SupportedProtocolVersionsError::Duplicate(duplicate)
        );
    }
}
