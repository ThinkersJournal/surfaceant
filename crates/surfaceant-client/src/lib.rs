// SPDX-License-Identifier: MIT OR Apache-2.0

//! SurfaceAnt client skeleton. Today it only decodes relay frames with the shared
//! [`surfaceant_proto`] types, which proves the Worker and the client compile against one definition.

pub use surfaceant_proto::{Event, Frame, PROTOCOL_VERSION};

/// Decode a relay text frame; an `event` yields `(frame, raw body)`.
pub fn decode(text: &str) -> Result<(Frame, Option<Vec<u8>>), surfaceant_proto::DecodeError> {
    let frame = Frame::from_text(text)?;
    let body = match &frame {
        Frame::Event(e) => Some(e.body()?),
        _ => None,
    };
    Ok((frame, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_an_event_to_raw_bytes() {
        let e = Event {
            seq: 1,
            delivery_id: "d".into(),
            received_at: "2026-11-14T00:00:00Z".into(),
            method: "POST".into(),
            path_suffix: String::new(),
            query: String::new(),
            headers: vec![],
            body_b64: String::new(),
        }
        .with_body(b"\xff\x00raw");
        let (frame, body) = decode(&Frame::Event(e).to_text()).unwrap();
        assert!(matches!(frame, Frame::Event(_)));
        assert_eq!(body.unwrap(), b"\xff\x00raw");
        assert_eq!(decode(&Frame::Ping.to_text()).unwrap().1, None);
    }
}
