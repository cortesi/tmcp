use std::collections::HashMap;

use crate::{
    error::Result,
    schema::{self, *},
};

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

/// Create a JSONRPC error response
pub fn create_jsonrpc_error(
    id: RequestId,
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
) -> JSONRPCErrorResponse {
    JSONRPCErrorResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(id),
        error: ErrorObject {
            code: code as i32,
            message,
            data,
        },
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
                result: schema::JSONRpcResult {
                    _meta: None,
                    other: if let Some(obj) = json_value.as_object() {
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                    } else {
                        let mut map = HashMap::new();
                        map.insert("result".to_string(), json_value);
                        map
                    },
                },
            }))
        }
        Err(e) => {
            if let Some(jsonrpc_error) = e.to_jsonrpc_response(id.clone()) {
                JSONRPCMessage::Response(JSONRPCResponse::Error(jsonrpc_error))
            } else {
                JSONRPCMessage::Response(JSONRPCResponse::Error(create_jsonrpc_error(
                    id,
                    INTERNAL_ERROR as i64,
                    e.to_string(),
                    None,
                )))
            }
        }
    }
}
