use std::str;

use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, error};

use crate::{
    error::{Error, Result},
    schema::{JSONRPCMessage, JSONRPCNotification, JSONRPCRequest, JSONRPCResponse},
};

/// JSON-RPC codec for encoding/decoding messages over a stream
/// Uses newline-delimited JSON format
pub struct JsonRpcCodec;

impl JsonRpcCodec {
    /// Create a new JSON-RPC codec.
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonRpcCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for JsonRpcCodec {
    type Error = Error;
    type Item = JSONRPCMessage;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        // Look for newline delimiter
        let Some(n) = src.iter().position(|b| *b == b'\n') else {
            // Not enough data
            return Ok(None);
        };

        // Split off the line including the newline
        let line = src.split_to(n + 1);

        // Skip empty lines
        if line.len() <= 1 {
            return Ok(None);
        }

        // Parse JSON, excluding the trailing newline
        let json_bytes = &line[..line.len() - 1];

        debug!(
            "Decoding JSON-RPC message: {:?}",
            str::from_utf8(json_bytes)
        );

        let message: JSONRPCMessage = serde_json::from_slice(json_bytes).map_err(|e| {
            error!("Failed to parse JSON-RPC message: {}", e);
            if let Ok(text) = str::from_utf8(json_bytes) {
                Error::InvalidMessageFormat {
                    message: format!("Invalid JSON: {e} (content: {text})"),
                }
            } else {
                Error::InvalidMessageFormat {
                    message: format!("Invalid JSON: {e} (non-UTF8 content)"),
                }
            }
        })?;
        Ok(Some(message))
    }
}

impl Encoder<JSONRPCMessage> for JsonRpcCodec {
    type Error = Error;

    fn encode(&mut self, item: JSONRPCMessage, dst: &mut BytesMut) -> Result<()> {
        let json = serde_json::to_vec(&item)?;
        dst.reserve(json.len() + 1);
        dst.put_slice(&json);
        dst.put_u8(b'\n');
        debug!("Encoded JSON-RPC message: {:?}", str::from_utf8(&json));
        Ok(())
    }
}

impl Encoder<JSONRPCRequest> for JsonRpcCodec {
    type Error = Error;

    fn encode(&mut self, item: JSONRPCRequest, dst: &mut BytesMut) -> Result<()> {
        self.encode(JSONRPCMessage::Request(item), dst)
    }
}

impl Encoder<JSONRPCResponse> for JsonRpcCodec {
    type Error = Error;

    fn encode(&mut self, item: JSONRPCResponse, dst: &mut BytesMut) -> Result<()> {
        self.encode(JSONRPCMessage::Response(item), dst)
    }
}

impl Encoder<JSONRPCNotification> for JsonRpcCodec {
    type Error = Error;

    fn encode(&mut self, item: JSONRPCNotification, dst: &mut BytesMut) -> Result<()> {
        self.encode(JSONRPCMessage::Notification(item), dst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{JSONRPC_VERSION, Request, RequestId};

    #[test]
    fn test_encode_decode_request() {
        let mut codec = JsonRpcCodec::new();
        let mut buf = BytesMut::new();

        let request = JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::String("test-1".to_string()),
            request: Request {
                method: "initialize".to_string(),
                params: None,
            },
        };

        // Encode
        codec.encode(request, &mut buf).unwrap();

        // Decode
        let decoded = codec.decode(&mut buf).unwrap().unwrap();

        match decoded {
            JSONRPCMessage::Request(req) => {
                assert_eq!(req.id, RequestId::String("test-1".to_string()));
                assert_eq!(req.request.method, "initialize");
            }
            _ => panic!("Expected request message"),
        }
    }
}
