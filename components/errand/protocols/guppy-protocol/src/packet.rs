//! Guppy packet wire forms: `<header>\r\n[<data>]`.

/// A parsed server→client packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// `<seq> <mimetype>\r\n<data>` — the first packet of a success.
    First {
        seq: u32,
        mime: String,
        data: Vec<u8>,
    },
    /// `<seq>\r\n<data>` — a continuation; empty data marks end-of-file.
    Continuation { seq: u32, data: Vec<u8> },
    /// `1 <prompt>\r\n`
    Prompt { text: String },
    /// `3 <url>\r\n`
    Redirect { target: String },
    /// `4 <error>\r\n`
    Error { message: String },
}

/// Parse a server→client packet.
///
/// Disambiguation, per the spec: success sequence numbers are ≥ 6, but the
/// spec warns clients not to confuse e.g. seq 39 with a redirect — the
/// distinction is that special packets have the literal single digit `1`,
/// `3`, or `4` as their whole first token, and success first-packets carry a
/// non-empty MIME token after the space, while continuations have a bare
/// numeric header.
pub fn parse_packet(datagram: &[u8]) -> Result<Packet, String> {
    let split = datagram
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| "packet has no CRLF".to_string())?;
    let header = std::str::from_utf8(&datagram[..split])
        .map_err(|_| "packet header is not UTF-8".to_string())?;
    let data = datagram[split + 2..].to_vec();

    match header.split_once(' ') {
        Some(("1", rest)) => Ok(Packet::Prompt {
            text: rest.trim().to_string(),
        }),
        Some(("3", rest)) => Ok(Packet::Redirect {
            target: rest.trim().to_string(),
        }),
        Some(("4", rest)) => Ok(Packet::Error {
            message: rest.trim().to_string(),
        }),
        Some((seq, mime)) => {
            let seq: u32 = seq
                .parse()
                .map_err(|_| format!("bad sequence number: {seq:?}"))?;
            if mime.trim().is_empty() {
                return Err("first packet has an empty MIME type".to_string());
            }
            Ok(Packet::First {
                seq,
                mime: mime.trim().to_string(),
                data,
            })
        },
        None => {
            let seq: u32 = header
                .trim()
                .parse()
                .map_err(|_| format!("bad packet header: {header:?}"))?;
            Ok(Packet::Continuation { seq, data })
        },
    }
}

/// Encode the first packet of a success.
pub(crate) fn encode_first(seq: u32, mime: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("{seq} {mime}\r\n").into_bytes();
    out.extend_from_slice(data);
    out
}

/// Encode a continuation (empty `data` encodes the end-of-file packet).
pub(crate) fn encode_continuation(seq: u32, data: &[u8]) -> Vec<u8> {
    let mut out = format!("{seq}\r\n").into_bytes();
    out.extend_from_slice(data);
    out
}

/// Encode an acknowledgement (client→server): `<seq>\r\n`.
pub(crate) fn encode_ack(seq: u32) -> Vec<u8> {
    format!("{seq}\r\n").into_bytes()
}

/// Parse a client→server datagram: an acknowledgement (a bare integer line
/// with no data) or a request (a URL line).
pub(crate) enum ClientDatagram {
    Ack(u32),
    Request(String),
}

pub(crate) fn parse_client_datagram(datagram: &[u8]) -> Result<ClientDatagram, String> {
    let split = datagram
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| "datagram has no CRLF".to_string())?;
    let line =
        std::str::from_utf8(&datagram[..split]).map_err(|_| "datagram is not UTF-8".to_string())?;
    if datagram.len() == split + 2 {
        if let Ok(seq) = line.trim().parse::<u32>() {
            return Ok(ClientDatagram::Ack(seq));
        }
    }
    Ok(ClientDatagram::Request(line.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_packets_parse() {
        // From the spec's examples section.
        assert_eq!(
            parse_packet(b"566837578 text/gemini\r\n# Title 1\n").unwrap(),
            Packet::First {
                seq: 566_837_578,
                mime: "text/gemini".to_string(),
                data: b"# Title 1\n".to_vec()
            }
        );
        assert_eq!(
            parse_packet(b"566837579\r\nParagraph 1").unwrap(),
            Packet::Continuation {
                seq: 566_837_579,
                data: b"Paragraph 1".to_vec()
            }
        );
        assert_eq!(
            parse_packet(b"566837581\r\n").unwrap(),
            Packet::Continuation {
                seq: 566_837_581,
                data: Vec::new()
            }
        );
        assert_eq!(
            parse_packet(b"1 Your name\r\n").unwrap(),
            Packet::Prompt {
                text: "Your name".to_string()
            }
        );
        assert_eq!(
            parse_packet(b"3 /b\r\n").unwrap(),
            Packet::Redirect {
                target: "/b".to_string()
            }
        );
        assert_eq!(
            parse_packet(b"4 No search keywords specified\r\n").unwrap(),
            Packet::Error {
                message: "No search keywords specified".to_string()
            }
        );
    }

    #[test]
    fn low_sequence_numbers_with_mime_are_success_not_special() {
        // The spec's explicit warning: don't confuse seq 39/41 with 3/4.
        assert!(matches!(
            parse_packet(b"39 text/plain\r\nhi").unwrap(),
            Packet::First { seq: 39, .. }
        ));
        assert!(matches!(
            parse_packet(b"41 text/plain\r\nhi").unwrap(),
            Packet::First { seq: 41, .. }
        ));
    }

    #[test]
    fn client_datagrams_split_into_acks_and_requests() {
        assert!(matches!(
            parse_client_datagram(b"566837578\r\n").unwrap(),
            ClientDatagram::Ack(566_837_578)
        ));
        assert!(matches!(
            parse_client_datagram(b"guppy://localhost/a\r\n").unwrap(),
            ClientDatagram::Request(url) if url == "guppy://localhost/a"
        ));
    }

    #[test]
    fn encodings_round_trip() {
        let first = encode_first(42, "text/gemini", b"body");
        assert!(matches!(
            parse_packet(&first).unwrap(),
            Packet::First { seq: 42, .. }
        ));
        let eof = encode_continuation(43, b"");
        assert_eq!(
            parse_packet(&eof).unwrap(),
            Packet::Continuation {
                seq: 43,
                data: Vec::new()
            }
        );
        assert_eq!(encode_ack(42), b"42\r\n");
    }
}
