// SPDX-License-Identifier: MIT OR Apache-2.0

//! Wire frames of the SurfaceAnt relay protocol (events mode), shared by the Worker and the client.
//!
//! Every frame is one JSON object in one WebSocket text frame, tagged by `type`. The handshake frames
//! (`hello`, `welcome`) carry the protocol version `v`; see `docs/DESIGN.md` section 3.

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

/// The protocol version spoken by this crate.
pub const PROTOCOL_VERSION: u32 = 1;

/// One relay protocol frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// Client to relay, first frame: the highest `seq` the client has acknowledged.
    Hello {
        /// Protocol version.
        v: u32,
        /// Highest acknowledged event sequence number (0 for none).
        last_acked_seq: i64,
    },
    /// Relay to client, reply to `hello`.
    Welcome {
        /// Protocol version.
        v: u32,
        /// Largest accepted event body, in bytes.
        max_event_bytes: u64,
        /// How long an event is buffered for an offline client, in seconds.
        ttl_seconds: u64,
        /// The relay's clock, RFC 3339.
        server_time: String,
    },
    /// Relay to client: one forwarded request.
    Event(Event),
    /// Client to relay: cumulative acknowledgement of every event up to and including `seq`.
    Ack {
        /// Highest event sequence number processed.
        seq: i64,
    },
    /// Keep-alive request (either direction).
    Ping,
    /// Keep-alive reply.
    Pong,
    /// Relay to client; the relay closes the connection afterwards.
    Error {
        /// Machine-readable code, for example `replaced`.
        code: String,
        /// Human-readable detail.
        message: String,
    },
}

/// A request forwarded through the relay, untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Relay-assigned order within the channel.
    pub seq: i64,
    /// De-duplication key (the sender's delivery id when it has one).
    pub delivery_id: String,
    /// When the relay stored the event, RFC 3339.
    pub received_at: String,
    /// HTTP method.
    pub method: String,
    /// Path after `/hooks/<channel-id>`, with a leading `/` or empty.
    pub path_suffix: String,
    /// Raw query string without `?`, or empty.
    pub query: String,
    /// Headers as `(name, value)` pairs, in arrival order, duplicates kept.
    pub headers: Vec<(String, String)>,
    /// The raw request body, base64 (standard alphabet, padded).
    pub body_b64: String,
}

/// A frame could not be decoded.
#[derive(Debug)]
pub enum DecodeError {
    /// Not valid JSON for any frame.
    Json(serde_json::Error),
    /// `body_b64` is not valid base64.
    Base64(base64::DecodeError),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "invalid frame: {e}"),
            Self::Base64(e) => write!(f, "invalid body_b64: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Frame {
    /// Serialise to the JSON text sent in one WebSocket text frame.
    pub fn to_text(&self) -> String {
        // Every field is a plain string, number or sequence of them: serialisation cannot fail.
        serde_json::to_string(self).expect("frame serialises")
    }

    /// Parse one WebSocket text frame.
    pub fn from_text(text: &str) -> Result<Self, DecodeError> {
        serde_json::from_str(text).map_err(DecodeError::Json)
    }
}

impl Event {
    /// Build an event around the raw `body`, encoding it for the wire.
    pub fn with_body(mut self, body: &[u8]) -> Self {
        self.body_b64 = STANDARD.encode(body);
        self
    }

    /// The raw body exactly as the sender posted it.
    pub fn body(&self) -> Result<Vec<u8>, DecodeError> {
        STANDARD.decode(&self.body_b64).map_err(DecodeError::Base64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Event {
        Event {
            seq: 7,
            delivery_id: "d-1".into(),
            received_at: "2026-11-14T00:00:00Z".into(),
            method: "POST".into(),
            path_suffix: String::new(),
            query: String::new(),
            headers: vec![
                ("X-Hub-Signature-256".into(), "sha256=ab".into()),
                ("Set-Cookie".into(), "a=1".into()),
                ("Set-Cookie".into(), "b=2".into()),
            ],
            body_b64: String::new(),
        }
    }

    #[test]
    fn every_frame_round_trips() {
        let frames = [
            Frame::Hello {
                v: PROTOCOL_VERSION,
                last_acked_seq: 0,
            },
            Frame::Welcome {
                v: PROTOCOL_VERSION,
                max_event_bytes: 1 << 20,
                ttl_seconds: 86_400,
                server_time: "2026-11-14T00:00:00Z".into(),
            },
            Frame::Event(sample().with_body(b"x")),
            Frame::Ack { seq: 7 },
            Frame::Ping,
            Frame::Pong,
            Frame::Error {
                code: "replaced".into(),
                message: "new connection".into(),
            },
        ];
        for f in frames {
            assert_eq!(Frame::from_text(&f.to_text()).unwrap(), f);
        }
    }

    #[test]
    fn body_is_byte_identical_for_non_utf8_and_trailing_newline() {
        for body in [
            &b"\xff\xfe\x00\x80"[..],
            b"{\"a\":1}\n",
            b"",
            &[0u8; 1024][..],
        ] {
            let e = sample().with_body(body);
            let back = match Frame::from_text(&Frame::Event(e).to_text()).unwrap() {
                Frame::Event(e) => e,
                other => panic!("not an event: {other:?}"),
            };
            assert_eq!(back.body().unwrap(), body);
        }
    }

    #[test]
    fn header_order_and_duplicates_survive() {
        let e = sample();
        let Frame::Event(back) = Frame::from_text(&Frame::Event(e.clone()).to_text()).unwrap()
        else {
            panic!("not an event")
        };
        assert_eq!(back.headers, e.headers);
    }

    #[test]
    fn wire_shape_is_tagged_snake_case() {
        assert_eq!(Frame::Ping.to_text(), r#"{"type":"ping"}"#);
        assert_eq!(Frame::Ack { seq: 3 }.to_text(), r#"{"type":"ack","seq":3}"#);
    }

    #[test]
    fn negative_controls_are_rejected() {
        assert!(Frame::from_text(r#"{"type":"nope"}"#).is_err());
        assert!(Frame::from_text("not json").is_err());
        let mut e = sample();
        e.body_b64 = "!!!not base64".into();
        assert!(e.body().is_err());
    }
}
