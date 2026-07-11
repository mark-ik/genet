//! The guppy client: request datagram out, ack-and-reassemble the response.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::net::UdpSocket;
use url::Url;

use super::packet::{Packet, encode_ack, parse_packet};
use super::{GUPPY_PORT, GuppyResponse, MAX_REQUEST_BYTES};

/// Options for a [`fetch`].
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Overall deadline for the whole transaction. Default 30s.
    pub timeout: Duration,
    /// Retransmit the request / re-acknowledge if nothing arrives for this
    /// long. Default 1s.
    pub retransmit_after: Duration,
    /// Refuse reassembled bodies larger than this (also bounds the buffer
    /// held for out-of-order chunks). Default 16 MiB.
    pub max_body: usize,
    /// Send to this address instead of resolving the URL's host. For tests
    /// and odd deployments.
    pub connect_addr: Option<std::net::SocketAddr>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retransmit_after: Duration::from_secs(1),
            max_body: 16 * 1024 * 1024,
            connect_addr: None,
        }
    }
}

/// Why a fetch failed before a [`GuppyResponse`] was obtained.
#[derive(Debug)]
pub enum ClientError {
    BadUrl(String),
    RequestTooLong { request_bytes: usize, max: usize },
    Io(String),
    /// The overall deadline elapsed. Per the spec, only the EOF packet
    /// distinguishes a complete response from a truncated one, so a timeout
    /// means the response cannot be trusted.
    Timeout,
    Protocol(String),
    BodyTooLarge { max: usize },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadUrl(message) => write!(formatter, "bad guppy URL: {message}"),
            Self::RequestTooLong { request_bytes, max } => write!(
                formatter,
                "guppy request is {request_bytes} bytes (max {max})"
            ),
            Self::Io(message) => write!(formatter, "guppy IO error: {message}"),
            Self::Timeout => write!(formatter, "guppy transaction timed out"),
            Self::Protocol(message) => write!(formatter, "guppy protocol error: {message}"),
            Self::BodyTooLarge { max } => {
                write!(formatter, "guppy response exceeds {max} bytes")
            }
        }
    }
}

impl std::error::Error for ClientError {}

/// Fetch a `guppy://` URL. User input goes in the URL's query component,
/// percent-encoded (the input-prompt flow: a [`GuppyResponse::Prompt`] answer
/// means "repeat the request with input attached").
pub async fn fetch(url: &str, options: &FetchOptions) -> Result<GuppyResponse, ClientError> {
    let parsed = Url::parse(url).map_err(|error| ClientError::BadUrl(error.to_string()))?;
    if parsed.scheme() != "guppy" {
        return Err(ClientError::BadUrl(format!(
            "expected guppy:// scheme, got {}://",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ClientError::BadUrl("URL has no host".to_string()))?;
    let port = parsed.port().unwrap_or(GUPPY_PORT);

    let request = format!("{url}\r\n");
    if request.len() > MAX_REQUEST_BYTES {
        return Err(ClientError::RequestTooLong {
            request_bytes: request.len(),
            max: MAX_REQUEST_BYTES,
        });
    }

    // The spec's session rule: one source port for the whole transaction —
    // one bound socket per fetch.
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|error| ClientError::Io(format!("bind: {error}")))?;
    match options.connect_addr {
        Some(addr) => socket.connect(addr).await,
        None => socket.connect((host, port)).await,
    }
    .map_err(|error| ClientError::Io(format!("connect: {error}")))?;

    tokio::time::timeout(options.timeout, transact(&socket, &request, options))
        .await
        .map_err(|_| ClientError::Timeout)?
}

/// Reassembly state for one success response.
struct Reassembly {
    first_seq: u32,
    mime: String,
    /// Data chunks keyed by seq (the first packet's chunk included).
    chunks: BTreeMap<u32, Vec<u8>>,
    /// Highest seq such that every seq in `first_seq..=contiguous_end` is
    /// present.
    contiguous_end: u32,
    /// Total buffered bytes (bounds memory against hostile senders).
    buffered: usize,
    /// The empty continuation's seq, once seen.
    eof_seq: Option<u32>,
}

impl Reassembly {
    fn insert(&mut self, seq: u32, data: Vec<u8>) {
        self.buffered += data.len();
        self.chunks.entry(seq).or_insert(data);
        while self.chunks.contains_key(&(self.contiguous_end + 1)) {
            self.contiguous_end += 1;
        }
    }

    fn complete(&self) -> bool {
        self.eof_seq == Some(self.contiguous_end + 1)
    }

    fn into_response(self) -> GuppyResponse {
        let mut body = Vec::with_capacity(self.buffered);
        for (_, chunk) in self.chunks {
            body.extend_from_slice(&chunk);
        }
        GuppyResponse::Success {
            mime: self.mime,
            body,
        }
    }
}

async fn transact(
    socket: &UdpSocket,
    request: &str,
    options: &FetchOptions,
) -> Result<GuppyResponse, ClientError> {
    let send = |bytes: Vec<u8>| async move {
        socket
            .send(&bytes)
            .await
            .map_err(|error| ClientError::Io(format!("send: {error}")))
    };

    send(request.as_bytes().to_vec()).await?;

    let mut state: Option<Reassembly> = None;
    let mut buffer = vec![0u8; 65_536];

    loop {
        if let Some(reassembly) = &state {
            if reassembly.complete() {
                return Ok(state.take().expect("checked").into_response());
            }
            if reassembly.buffered > options.max_body {
                return Err(ClientError::BodyTooLarge {
                    max: options.max_body,
                });
            }
        }

        let received =
            tokio::time::timeout(options.retransmit_after, socket.recv(&mut buffer)).await;
        let count = match received {
            Ok(Ok(count)) => count,
            Ok(Err(error)) => return Err(ClientError::Io(format!("recv: {error}"))),
            Err(_) => {
                // Nothing arrived for a while. Per the spec: re-transmit the
                // request if the response hasn't started, else re-acknowledge
                // the stall point (lost acks are the usual cause).
                match &state {
                    None => {
                        send(request.as_bytes().to_vec()).await?;
                    }
                    Some(reassembly) => {
                        send(encode_ack(reassembly.contiguous_end)).await?;
                        if let Some(eof) = reassembly.eof_seq {
                            send(encode_ack(eof)).await?;
                        }
                    }
                }
                continue;
            }
        };

        let packet = match parse_packet(&buffer[..count]) {
            Ok(packet) => packet,
            Err(error) => {
                log::debug!("guppy: ignoring malformed packet: {error}");
                continue;
            }
        };

        match packet {
            // Special packets are only meaningful as the whole response;
            // after a success has started they are stray and ignored.
            Packet::Prompt { text } if state.is_none() => {
                return Ok(GuppyResponse::Prompt { text });
            }
            Packet::Redirect { target } if state.is_none() => {
                return Ok(GuppyResponse::Redirect { target });
            }
            Packet::Error { message } if state.is_none() => {
                return Ok(GuppyResponse::Error { message });
            }
            Packet::Prompt { .. } | Packet::Redirect { .. } | Packet::Error { .. } => {}
            Packet::First { seq, mime, data } => {
                // Always ack, duplicates included (a duplicate means our ack
                // was lost).
                send(encode_ack(seq)).await?;
                if state.is_none() {
                    let mut reassembly = Reassembly {
                        first_seq: seq,
                        mime,
                        chunks: BTreeMap::new(),
                        contiguous_end: seq,
                        buffered: 0,
                        eof_seq: None,
                    };
                    reassembly.buffered = data.len();
                    reassembly.chunks.insert(seq, data);
                    state = Some(reassembly);
                }
            }
            Packet::Continuation { seq, data } => {
                // A continuation before the first packet cannot be anchored;
                // cache-less clients are allowed to ignore it (the server
                // retransmits).
                let Some(reassembly) = &mut state else {
                    continue;
                };
                if seq <= reassembly.first_seq {
                    continue;
                }
                send(encode_ack(seq)).await?;
                if data.is_empty() {
                    reassembly.eof_seq = Some(seq);
                } else {
                    reassembly.insert(seq, data);
                }
            }
        }
    }
}
