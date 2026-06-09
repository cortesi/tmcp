use std::str;

use bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, error};

use crate::{
    error::{Error, Result},
    schema::{JSONRPCMessage, JSONRPCNotification, JSONRPCRequest, JSONRPCResponse},
};

/// Maximum accepted frame length in bytes.
///
/// A peer that streams data without ever sending a newline would otherwise
/// grow the receive buffer without bound.
const MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

/// One decoded frame from the wire.
///
/// Malformed lines are surfaced as a frame rather than a decoder error
/// because `tokio_util::codec::Framed` permanently terminates the stream
/// after a decoder error; a single garbage line must not sever the session.
#[derive(Debug)]
pub enum Frame {
    /// A successfully parsed JSON-RPC message.
    Message(JSONRPCMessage),
    /// A line that was not valid JSON, already consumed from the buffer.
    Malformed(String),
}

/// JSON-RPC codec for encoding/decoding messages over a stream.
///
/// Uses newline-delimited JSON format. A line that fails to parse is consumed
/// and yielded as [`Frame::Malformed`], so callers can answer with a JSON-RPC
/// parse error and keep the connection alive; framing stays consistent because
/// resynchronization happens at the next newline. Frames exceeding
/// [`MAX_FRAME_LENGTH`] are reported as the fatal [`Error::InvalidMessageFormat`].
#[derive(Default)]
pub struct JsonRpcCodec {
    /// Index from which to resume scanning for the next newline, so
    /// fragmented delivery is not rescanned from the start each time.
    next_scan_index: usize,
}

impl Decoder for JsonRpcCodec {
    type Error = Error;
    type Item = Frame;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        loop {
            // Look for the newline delimiter, resuming where the last scan
            // left off.
            let Some(offset) = src[self.next_scan_index..].iter().position(|b| *b == b'\n') else {
                if src.len() > MAX_FRAME_LENGTH {
                    return Err(Error::InvalidMessageFormat {
                        message: format!(
                            "Frame exceeds maximum length of {MAX_FRAME_LENGTH} bytes"
                        ),
                    });
                }
                self.next_scan_index = src.len();
                return Ok(None);
            };
            let newline = self.next_scan_index + offset;
            self.next_scan_index = 0;

            // Split off the line including the newline
            let line = src.split_to(newline + 1);

            // Skip empty lines
            if line.len() <= 1 {
                continue;
            }

            // Parse JSON, excluding the trailing newline
            let json_bytes = &line[..line.len() - 1];

            debug!(
                "Decoding JSON-RPC message: {:?}",
                str::from_utf8(json_bytes)
            );

            return match serde_json::from_slice::<JSONRPCMessage>(json_bytes) {
                Ok(message) => Ok(Some(Frame::Message(message))),
                Err(e) => {
                    error!("Failed to parse JSON-RPC message: {}", e);
                    Ok(Some(Frame::Malformed(format!("Invalid JSON: {e}"))))
                }
            };
        }
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
        let mut codec = JsonRpcCodec::default();
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
            Frame::Message(JSONRPCMessage::Request(req)) => {
                assert_eq!(req.id, RequestId::String("test-1".to_string()));
                assert_eq!(req.request.method, "initialize");
            }
            _ => panic!("Expected request message"),
        }
    }

    #[test]
    fn test_decode_skips_empty_lines() {
        let mut codec = JsonRpcCodec::default();
        // Buffer with leading newline and a valid message
        let mut buf = BytesMut::from("\n{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n");

        // Should skip the first newline and return the message
        let decoded = codec.decode(&mut buf).unwrap();
        assert!(decoded.is_some(), "Should decode message after empty line");
    }

    #[test]
    fn malformed_line_is_consumed_and_decoding_resumes() {
        let mut codec = JsonRpcCodec::default();
        let mut buf =
            BytesMut::from("this is not json\n{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n");

        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert!(matches!(frame, Frame::Malformed(_)));

        // The bad line was consumed; the next decode yields the valid message.
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert!(
            matches!(frame, Frame::Message(_)),
            "decoding resumes after a malformed line"
        );
    }

    #[test]
    fn fragmented_input_is_not_rescanned_from_start() {
        let mut codec = JsonRpcCodec::default();
        let mut buf = BytesMut::from("{\"jsonrpc\":\"2.0\",");

        assert!(codec.decode(&mut buf).unwrap().is_none());
        assert_eq!(codec.next_scan_index, buf.len());

        buf.extend_from_slice(b"\"method\":\"ping\"}\n");
        let decoded = codec.decode(&mut buf).unwrap();
        assert!(decoded.is_some());
        assert_eq!(codec.next_scan_index, 0);
    }

    #[test]
    fn oversized_frame_without_newline_is_fatal() {
        let mut codec = JsonRpcCodec::default();
        let mut buf = BytesMut::new();
        buf.resize(MAX_FRAME_LENGTH + 1, b'a');

        let error = codec.decode(&mut buf).expect_err("oversized frame");
        assert!(matches!(error, Error::InvalidMessageFormat { .. }));
    }
}
