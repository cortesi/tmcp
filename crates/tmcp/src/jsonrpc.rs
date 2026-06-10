use std::collections::HashMap;

use serde::de::DeserializeOwned;

use crate::{
    error::{Error, Result},
    schema::{self, *},
};

/// Parse a typed inbound request from its JSON-RPC method and params.
///
/// The wire `params` object and its `_meta` are folded back into one object
/// (alongside `method`) so the method-tagged enums can deserialize it.
pub fn parse_typed_request<T>(method: &str, params: Option<RequestParams>) -> Result<T>
where
    T: DeserializeOwned,
{
    let mut object = serde_json::Map::new();
    object.insert(
        "method".to_string(),
        serde_json::Value::String(method.to_string()),
    );
    if let Some(params) = params {
        if let Some(meta) = params._meta {
            object.insert("_meta".to_string(), serde_json::to_value(meta)?);
        }
        object.extend(params.other);
    }
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|err| classify_parse_error(method, &err))
}

/// Parse a typed inbound notification from its JSON-RPC envelope.
///
/// `_meta` is preserved and folded back into the object so typed
/// notifications that model it can capture it.
pub fn parse_typed_notification<T>(notification: Notification) -> Result<T>
where
    T: DeserializeOwned,
{
    let method = notification.method;
    let mut object = serde_json::Map::new();
    object.insert(
        "method".to_string(),
        serde_json::Value::String(method.clone()),
    );
    if let Some(params) = notification.params {
        if let Some(meta) = params._meta {
            object.insert("_meta".to_string(), serde_json::to_value(meta)?);
        }
        object.extend(params.other);
    }
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|err| classify_parse_error(&method, &err))
}

/// Classify a deserialization failure as method-not-found or invalid params.
///
/// The method-tagged enums report an unknown method as serde's "unknown
/// variant" error naming the method itself; matching on the quoted method
/// name avoids misclassifying unknown variants nested inside valid params.
fn classify_parse_error(method: &str, err: &serde_json::Error) -> Error {
    if err
        .to_string()
        .contains(&format!("unknown variant `{method}`"))
    {
        Error::MethodNotFound(method.to_string())
    } else {
        Error::InvalidParams(format!("Invalid parameters for {method}: {err}"))
    }
}

/// Create a JSONRPC notification from a typed notification.
pub fn create_jsonrpc_notification<T>(notification: &T) -> JSONRPCNotification
where
    T: serde::Serialize + NotificationTrait,
{
    let method = notification.method();
    let params = serde_json::to_value(notification)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .and_then(|mut obj| {
            obj.remove("method");
            let meta = obj
                .remove("_meta")
                .and_then(|value| value.as_object().cloned())
                .map(|map| map.into_iter().collect::<HashMap<_, _>>());
            if obj.is_empty() && meta.is_none() {
                None
            } else {
                Some(NotificationParams {
                    _meta: meta,
                    other: obj.into_iter().collect(),
                })
            }
        });

    JSONRPCNotification {
        jsonrpc: JSONRPC_VERSION.to_string(),
        notification: Notification { method, params },
    }
}

/// Trait to identify notification types and their methods
pub trait NotificationTrait: serde::Serialize {
    /// Return the JSON-RPC method name for this notification.
    fn method(&self) -> String;
}

// Implement NotificationTrait for server notifications
impl NotificationTrait for schema::ServerNotification {
    fn method(&self) -> String {
        match self {
            Self::ToolListChanged { .. } => "notifications/tools/list_changed".to_string(),
            Self::ResourceListChanged { .. } => "notifications/resources/list_changed".to_string(),
            Self::PromptListChanged { .. } => "notifications/prompts/list_changed".to_string(),
            Self::ElicitationComplete { .. } => "notifications/elicitation/complete".to_string(),
            Self::TaskStatus { .. } => "notifications/tasks/status".to_string(),
            Self::ResourceUpdated { .. } => "notifications/resources/updated".to_string(),
            Self::LoggingMessage { .. } => "notifications/message".to_string(),
            Self::Progress { .. } => "notifications/progress".to_string(),
            Self::Cancelled { .. } => "notifications/cancelled".to_string(),
        }
    }
}

// Implement NotificationTrait for client notifications
impl NotificationTrait for schema::ClientNotification {
    fn method(&self) -> String {
        match self {
            Self::Initialized { .. } => "notifications/initialized".to_string(),
            Self::RootsListChanged { .. } => "notifications/roots/list_changed".to_string(),
            Self::TaskStatus { .. } => "notifications/tasks/status".to_string(),
            Self::Cancelled { .. } => "notifications/cancelled".to_string(),
            Self::Progress { .. } => "notifications/progress".to_string(),
        }
    }
}

/// Convert a Result<T> to a JSONRPC response
pub fn result_to_jsonrpc_response<T>(id: RequestId, result: Result<T>) -> JSONRPCMessage
where
    T: serde::Serialize,
{
    match result {
        Ok(value) => {
            let json_value = serde_json::to_value(value).unwrap_or(serde_json::json!({}));
            JSONRPCMessage::Response(JSONRPCResponse::Result(JSONRPCResultResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                result: schema::JSONRPCResult {
                    _meta: None,
                    other: match json_value {
                        serde_json::Value::Object(map) => map.into_iter().collect(),
                        other => HashMap::from([("result".to_string(), other)]),
                    },
                },
            }))
        }
        Err(e) => {
            let error = e
                .to_jsonrpc_response(id.clone())
                .unwrap_or_else(|| JSONRPCErrorResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(id),
                    error: ErrorObject {
                        code: INTERNAL_ERROR,
                        message: e.to_string(),
                        data: None,
                    },
                });
            JSONRPCMessage::Response(JSONRPCResponse::Error(error))
        }
    }
}
