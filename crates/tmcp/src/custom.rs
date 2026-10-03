use std::{io, time::Duration};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    error::{Error, Result},
    jsonrpc::request_params,
    schema::Request,
};

/// Default payload limit of a custom request, in bytes of JSON: 1 MiB.
///
/// The limit applies to the params of a custom request and to its result.
pub const CUSTOM_PAYLOAD_LIMIT: usize = 1024 * 1024;

/// A request from a server to its client with a method outside the MCP schema.
///
/// A server sends it with
/// [`ServerCtx::send_request`](crate::ServerCtx::send_request), and a client
/// answers it with
/// [`ClientHandler::custom_request`](crate::ClientHandler::custom_request).
/// Put the method in a namespace, such as `vendor/action`.
///
/// The request has the default deadline of the connection, and the payload
/// limit [`CUSTOM_PAYLOAD_LIMIT`] for its params and its result.
#[derive(Debug, Clone)]
pub struct CustomRequest {
    /// The method of the request.
    method: String,
    /// The params object of the request, with `_meta` if the sender set it.
    params: Option<Map<String, Value>>,
    /// The deadline, or `None` for the default of the connection.
    timeout: Option<Duration>,
    /// The largest params and result that the request accepts, in bytes.
    payload_limit: usize,
}

impl CustomRequest {
    /// Create a custom request with the params that `params` serializes to.
    ///
    /// Use `()` for a request without params.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRequest`] when `method` is the method of a
    /// standard MCP server request, and [`Error::InvalidParams`] when `params`
    /// does not serialize to a JSON object or null.
    pub fn new(method: impl Into<String>, params: impl Serialize) -> Result<Self> {
        let method = method.into();
        if is_server_request_method(&method) {
            return Err(Error::InvalidRequest(format!(
                "`{method}` is a standard MCP request, not a custom request"
            )));
        }
        let params = match serde_json::to_value(params)? {
            Value::Object(object) => Some(object),
            Value::Null => None,
            _ => {
                return Err(Error::InvalidParams(format!(
                    "the params of {method} must be a JSON object"
                )));
            }
        };
        Ok(Self {
            method,
            params,
            timeout: None,
            payload_limit: CUSTOM_PAYLOAD_LIMIT,
        })
    }

    /// Set the deadline of the request. A zero duration disables the
    /// deadline.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Wait for the result without a deadline.
    ///
    /// Use this for a request that waits for a person. Cancellation and the
    /// end of the connection still end the wait.
    #[must_use]
    pub fn without_timeout(self) -> Self {
        self.with_timeout(Duration::ZERO)
    }

    /// Set the largest params and result that the request accepts, in bytes
    /// of JSON.
    #[must_use]
    pub fn with_payload_limit(mut self, limit: usize) -> Self {
        self.payload_limit = limit;
        self
    }

    /// Return the method of the request.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Check the params against the payload limit, and split the request into
    /// its body, its deadline, and its payload limit.
    pub(crate) fn prepare(self) -> Result<(Request, Option<Duration>, usize)> {
        if let Some(params) = &self.params {
            check_payload(&self.method, params, self.payload_limit)?;
        }
        let request = Request {
            method: self.method,
            params: self.params.and_then(request_params),
        };
        Ok((request, self.timeout, self.payload_limit))
    }
}

/// Return whether a method names a standard request from a server to a
/// client.
pub fn is_server_request_method(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "sampling/createMessage"
            | "roots/list"
            | "elicitation/create"
            | "tasks/get"
            | "tasks/result"
            | "tasks/list"
            | "tasks/cancel"
    )
}

/// Refuse a payload whose JSON encoding is larger than `limit` bytes.
pub fn check_payload(method: &str, payload: &impl Serialize, limit: usize) -> Result<()> {
    let size = encoded_len(payload)?;
    if size > limit {
        return Err(Error::PayloadTooLarge {
            method: method.to_owned(),
            size,
            limit,
        });
    }
    Ok(())
}

/// Return the length of the JSON encoding of a value, without building it.
fn encoded_len(value: &impl Serialize) -> Result<usize> {
    /// A writer that counts bytes and keeps none.
    struct Counter(usize);

    impl io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::{CreateMessageParams, ServerRequest};

    #[test]
    fn each_standard_server_request_is_known() {
        let elicit = serde_json::from_value(json!({
            "message": "name?",
            "requestedSchema": { "type": "object", "properties": {} }
        }))
        .expect("elicit params");
        let requests = [
            ServerRequest::ping(),
            ServerRequest::create_message(CreateMessageParams::user_message("hi")),
            ServerRequest::list_roots(),
            ServerRequest::elicit(elicit),
            ServerRequest::get_task("t"),
            ServerRequest::get_task_payload("t"),
            ServerRequest::list_tasks(None),
            ServerRequest::cancel_task("t"),
        ];
        for request in requests {
            assert!(is_server_request_method(request.method()));
            assert!(CustomRequest::new(request.method(), ()).is_err());
        }
        assert!(!is_server_request_method("vendor/action"));
    }

    #[test]
    fn params_must_be_an_object() {
        assert!(CustomRequest::new("vendor/action", ()).is_ok());
        assert!(CustomRequest::new("vendor/action", json!({ "a": 1 })).is_ok());
        assert!(matches!(
            CustomRequest::new("vendor/action", json!([1, 2])),
            Err(Error::InvalidParams(_))
        ));
    }

    #[test]
    fn prepare_refuses_params_over_the_limit() {
        let request = CustomRequest::new("vendor/action", json!({ "data": "x".repeat(64) }))
            .expect("request")
            .with_payload_limit(32);
        let error = request.prepare().expect_err("over the limit");
        assert!(matches!(
            error,
            Error::PayloadTooLarge { size, limit: 32, .. } if size > 64
        ));
    }

    #[test]
    fn encoded_len_matches_the_encoding() {
        let value = json!({ "text": "héllo", "n": [1, 2, 3] });
        assert_eq!(
            encoded_len(&value).expect("length"),
            serde_json::to_vec(&value).expect("encoding").len()
        );
    }
}
