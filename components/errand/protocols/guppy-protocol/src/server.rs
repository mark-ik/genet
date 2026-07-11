//! The guppy server: one UDP socket, per-peer sessions, windowed
//! transmission with retransmit-until-acked, per spec v0.4.4.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use percent_encoding::percent_decode_str;
use tokio::net::UdpSocket;
use url::Url;

use super::packet::{ClientDatagram, encode_continuation, encode_first, parse_client_datagram};
use super::{GuppyResponse, MAX_REQUEST_BYTES, MAX_SEQ, MIN_SEQ};

/// One guppy request, as handed to a [`Handler`].
#[derive(Debug, Clone)]
pub struct Request {
    /// The requested URL, verbatim.
    pub url: Url,
    /// The URL's path (`/` for an empty path).
    pub path: String,
    /// The URL's query component, percent-decoded: the user's input from the
    /// prompt flow, if any.
    pub input: Option<String>,
    /// The client's socket address.
    pub peer: SocketAddr,
}

/// The application seam: turn a [`Request`] into a [`GuppyResponse`].
/// `Success` bodies are chunked, transmitted, and retransmitted by the
/// server; the other variants go out as single special packets.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> impl Future<Output = GuppyResponse> + Send;
}

impl<F, Fut> Handler for F
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = GuppyResponse> + Send,
{
    fn handle(&self, request: Request) -> impl Future<Output = GuppyResponse> + Send {
        self(request)
    }
}

/// Server tuning.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Body bytes per continuation packet. The spec recommends ≥ 512 for
    /// large responses. Default 512.
    pub chunk_size: usize,
    /// How many unacknowledged packets may be in flight per session (the
    /// spec allows transmitting ahead of acks). Default 16.
    pub window: usize,
    /// Retransmit unacknowledged packets after this long. Default 500ms.
    pub retransmit_after: Duration,
    /// Drop sessions that have not completed within this long. Default 30s.
    pub session_timeout: Duration,
    /// Refuse new sessions beyond this many concurrent peers (denial-of-
    /// service protection, per the spec's sessions section). Default 1024.
    pub max_sessions: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 512,
            window: 16,
            retransmit_after: Duration::from_millis(500),
            session_timeout: Duration::from_secs(30),
            max_sessions: 1024,
        }
    }
}

struct Session {
    /// Every packet of the response, in seq order: `(seq, wire bytes)`.
    packets: Vec<(u32, Vec<u8>)>,
    /// Index of the next packet never yet sent.
    next_unsent: usize,
    /// Seqs sent but not yet acknowledged.
    unacked: BTreeSet<u32>,
    last_send: Instant,
    created: Instant,
    /// Set when every packet is acknowledged; the session lingers briefly so
    /// duplicate request packets stay ignored, per the spec.
    done_at: Option<Instant>,
}

const DONE_LINGER: Duration = Duration::from_secs(5);

/// Serve guppy on `socket` through `handler` until `shutdown` resolves.
///
/// The handler runs inline on the receive loop (guppy serves small documents;
/// a handler that must do slow work should do it elsewhere and answer fast).
pub async fn serve(
    socket: UdpSocket,
    handler: impl Handler,
    config: ServerConfig,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
    let mut buffer = vec![0u8; 65_536];
    let mut tick = tokio::time::interval(config.retransmit_after / 2);
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = tick.tick() => {
                let now = Instant::now();
                sessions.retain(|_, session| {
                    let expired = now.duration_since(session.created) > config.session_timeout;
                    let lingered = session
                        .done_at
                        .is_some_and(|done| now.duration_since(done) > DONE_LINGER);
                    !(expired || lingered)
                });
                for (peer, session) in sessions.iter_mut() {
                    if session.done_at.is_none()
                        && now.duration_since(session.last_send) >= config.retransmit_after
                    {
                        retransmit(&socket, *peer, session).await;
                    }
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let (count, peer) = match received {
                    Ok(received) => received,
                    Err(error) => {
                        log::warn!("guppy: recv failed: {error}");
                        continue;
                    }
                };
                if count > MAX_REQUEST_BYTES {
                    continue;
                }
                let datagram = match parse_client_datagram(&buffer[..count]) {
                    Ok(datagram) => datagram,
                    Err(error) => {
                        log::debug!("guppy: ignoring malformed datagram from {peer}: {error}");
                        continue;
                    }
                };
                match datagram {
                    ClientDatagram::Ack(seq) => {
                        if let Some(session) = sessions.get_mut(&peer) {
                            acknowledge(&socket, peer, session, seq, &config).await;
                        }
                    }
                    ClientDatagram::Request(line) => {
                        // Spec: additional request packets in a live session
                        // are ignored.
                        if sessions.contains_key(&peer) {
                            continue;
                        }
                        if sessions.len() >= config.max_sessions {
                            log::warn!("guppy: session limit reached; dropping request from {peer}");
                            continue;
                        }
                        let Some(request) = parse_request(&line, peer) else {
                            let _ = socket.send_to(b"4 Malformed request URL.\r\n", peer).await;
                            continue;
                        };
                        let response = handler.handle(request).await;
                        match response {
                            GuppyResponse::Success { mime, body } => {
                                let mut session = build_session(&mime, &body, config.chunk_size);
                                open_window(&socket, peer, &mut session, &config).await;
                                sessions.insert(peer, session);
                            }
                            GuppyResponse::Prompt { text } => {
                                let _ = socket.send_to(format!("1 {text}\r\n").as_bytes(), peer).await;
                            }
                            GuppyResponse::Redirect { target } => {
                                let _ = socket.send_to(format!("3 {target}\r\n").as_bytes(), peer).await;
                            }
                            GuppyResponse::Error { message } => {
                                let _ = socket.send_to(format!("4 {message}\r\n").as_bytes(), peer).await;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn parse_request(line: &str, peer: SocketAddr) -> Option<Request> {
    let url = Url::parse(line.trim()).ok()?;
    if url.scheme() != "guppy" {
        return None;
    }
    let path = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    let input = url
        .query()
        .map(|query| percent_decode_str(query).decode_utf8_lossy().into_owned());
    Some(Request {
        url,
        path,
        input,
        peer,
    })
}

/// Chunk a success response into its packet sequence, starting at a random
/// sequence number low enough that the EOF stays within [`MAX_SEQ`].
fn build_session(mime: &str, body: &[u8], chunk_size: usize) -> Session {
    let chunk_size = chunk_size.max(1);
    // First packet carries the first chunk; the rest are continuations; one
    // more for EOF.
    let continuation_count = if body.len() > chunk_size {
        body.len().div_ceil(chunk_size) - 1
    } else {
        0
    };
    let total_packets = 1 + continuation_count as u32 + 1;
    let first_seq = random_seq(total_packets);

    let mut packets = Vec::with_capacity(total_packets as usize);
    let first_chunk = &body[..body.len().min(chunk_size)];
    packets.push((first_seq, encode_first(first_seq, mime, first_chunk)));
    let mut seq = first_seq;
    let mut offset = first_chunk.len();
    while offset < body.len() {
        seq += 1;
        let end = (offset + chunk_size).min(body.len());
        packets.push((seq, encode_continuation(seq, &body[offset..end])));
        offset = end;
    }
    // End-of-file: a continuation with no data.
    packets.push((seq + 1, encode_continuation(seq + 1, &[])));

    Session {
        packets,
        next_unsent: 0,
        unacked: BTreeSet::new(),
        last_send: Instant::now(),
        created: Instant::now(),
        done_at: None,
    }
}

/// A dependency-free random first sequence number in
/// `[MIN_SEQ, MAX_SEQ - total_packets]`.
fn random_seq(total_packets: u32) -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    );
    let span = MAX_SEQ - total_packets - MIN_SEQ;
    MIN_SEQ + (hasher.finish() % u64::from(span)) as u32
}

/// Send unsent packets until the window is full.
async fn open_window(
    socket: &UdpSocket,
    peer: SocketAddr,
    session: &mut Session,
    config: &ServerConfig,
) {
    while session.unacked.len() < config.window && session.next_unsent < session.packets.len() {
        let (seq, wire) = &session.packets[session.next_unsent];
        if socket.send_to(wire, peer).await.is_err() {
            break;
        }
        session.unacked.insert(*seq);
        session.next_unsent += 1;
    }
    session.last_send = Instant::now();
}

async fn acknowledge(
    socket: &UdpSocket,
    peer: SocketAddr,
    session: &mut Session,
    seq: u32,
    config: &ServerConfig,
) {
    // Duplicate acks are ignored per the spec (remove is idempotent).
    session.unacked.remove(&seq);
    if session.next_unsent == session.packets.len() && session.unacked.is_empty() {
        if session.done_at.is_none() {
            session.done_at = Some(Instant::now());
        }
        return;
    }
    open_window(socket, peer, session, config).await;
}

async fn retransmit(socket: &UdpSocket, peer: SocketAddr, session: &mut Session) {
    // Everything in flight and unacknowledged goes again (bounded by the
    // window size).
    let unacked: Vec<u32> = session.unacked.iter().copied().collect();
    for seq in unacked {
        if let Ok(index) = session
            .packets
            .binary_search_by_key(&seq, |(packet_seq, _)| *packet_seq)
        {
            let _ = socket.send_to(&session.packets[index].1, peer).await;
        }
    }
    session.last_send = Instant::now();
}

/// A static-directory [`Handler`]: gemtext-first, `index.gmi` per directory,
/// traversal-protected, prompt input ignored.
#[derive(Debug, Clone)]
pub struct FileHandler {
    root: PathBuf,
}

impl FileHandler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, request_path: &str) -> Option<PathBuf> {
        let decoded = percent_decode_str(request_path).decode_utf8().ok()?;
        let mut resolved = self.root.clone();
        for segment in decoded.split('/') {
            match segment {
                "" | "." => continue,
                ".." => return None,
                segment if segment.contains(['\\', ':']) => return None,
                segment => resolved.push(segment),
            }
        }
        if decoded.ends_with('/') {
            resolved.push("index.gmi");
        }
        Some(resolved)
    }
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "gmi" | "gemini" => "text/gemini",
        "txt" | "" => "text/plain",
        "md" => "text/markdown",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

impl Handler for FileHandler {
    async fn handle(&self, request: Request) -> GuppyResponse {
        let Some(mut path) = self.resolve(&request.path) else {
            return GuppyResponse::Error {
                message: "Bad path.".to_string(),
            };
        };
        if path.is_dir() {
            path.push("index.gmi");
        }
        match tokio::fs::read(&path).await {
            Ok(body) => GuppyResponse::Success {
                mime: mime_for(&path).to_string(),
                body,
            },
            Err(_) => GuppyResponse::Error {
                message: format!("{} not found", request.path),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FetchOptions, fetch};

    async fn spawn(
        handler: impl Handler,
        config: ServerConfig,
    ) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = serve(socket, handler, config, async {
                let _ = rx.await;
            })
            .await;
        });
        (addr, tx)
    }

    fn options_for(addr: SocketAddr) -> FetchOptions {
        FetchOptions {
            connect_addr: Some(addr),
            timeout: Duration::from_secs(5),
            retransmit_after: Duration::from_millis(200),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn single_packet_round_trip() {
        let (addr, _stop) = spawn(
            |_request: Request| async move {
                GuppyResponse::Success {
                    mime: "text/gemini".to_string(),
                    body: b"# Title 1\n".to_vec(),
                }
            },
            ServerConfig::default(),
        )
        .await;
        let response = fetch("guppy://example.test/a", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(
            response,
            GuppyResponse::Success {
                mime: "text/gemini".to_string(),
                body: b"# Title 1\n".to_vec()
            }
        );
    }

    #[tokio::test]
    async fn multi_chunk_bodies_reassemble_in_order() {
        let body: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let expected = body.clone();
        let (addr, _stop) = spawn(
            move |_request: Request| {
                let body = body.clone();
                async move {
                    GuppyResponse::Success {
                        mime: "application/octet-stream".to_string(),
                        body,
                    }
                }
            },
            ServerConfig {
                chunk_size: 64,
                window: 4,
                ..Default::default()
            },
        )
        .await;
        let response = fetch("guppy://example.test/big", &options_for(addr))
            .await
            .unwrap();
        match response {
            GuppyResponse::Success { body, .. } => assert_eq!(body, expected),
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_redirect_and_error_round_trip() {
        let (addr, _stop) = spawn(
            |request: Request| async move {
                match (request.path.as_str(), request.input) {
                    ("/greet", None) => GuppyResponse::Prompt {
                        text: "Your name".to_string(),
                    },
                    ("/greet", Some(name)) => GuppyResponse::Success {
                        mime: "text/gemini".to_string(),
                        body: format!("Hello {name}\n").into_bytes(),
                    },
                    ("/old", _) => GuppyResponse::Redirect {
                        target: "/new".to_string(),
                    },
                    _ => GuppyResponse::Error {
                        message: "not found".to_string(),
                    },
                }
            },
            ServerConfig::default(),
        )
        .await;

        assert_eq!(
            fetch("guppy://example.test/greet", &options_for(addr))
                .await
                .unwrap(),
            GuppyResponse::Prompt {
                text: "Your name".to_string()
            }
        );
        assert_eq!(
            fetch("guppy://example.test/greet?Guppy", &options_for(addr))
                .await
                .unwrap(),
            GuppyResponse::Success {
                mime: "text/gemini".to_string(),
                body: b"Hello Guppy\n".to_vec()
            }
        );
        assert_eq!(
            fetch("guppy://example.test/old", &options_for(addr))
                .await
                .unwrap(),
            GuppyResponse::Redirect {
                target: "/new".to_string()
            }
        );
        assert_eq!(
            fetch("guppy://example.test/ghost", &options_for(addr))
                .await
                .unwrap(),
            GuppyResponse::Error {
                message: "not found".to_string()
            }
        );
    }

    /// Drive the client against a raw socket that delivers packets out of
    /// order and duplicated — the spec's unreliable-network examples.
    #[tokio::test]
    async fn client_survives_out_of_order_and_duplicate_delivery() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 4096];
            // Wait for the request.
            let (_count, peer) = socket.recv_from(&mut buffer).await.unwrap();
            let first = encode_first(100, "text/plain", b"AA");
            let second = encode_continuation(101, b"BB");
            let eof = encode_continuation(102, b"");
            // Out of order, with duplicates: 101, 100, 101, 102(eof), 100.
            for wire in [&second, &first, &second, &eof, &first] {
                socket.send_to(wire, peer).await.unwrap();
            }
            // Absorb acks so the test socket doesn't fill any queue.
            loop {
                if socket.recv_from(&mut buffer).await.is_err() {
                    break;
                }
            }
        });

        let response = fetch("guppy://example.test/x", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(
            response,
            GuppyResponse::Success {
                mime: "text/plain".to_string(),
                body: b"AABB".to_vec()
            }
        );
    }

    #[tokio::test]
    async fn file_handler_serves_and_refuses_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("index.gmi"), "# Hi\n").unwrap();
        let (addr, _stop) = spawn(FileHandler::new(dir.path()), ServerConfig::default()).await;

        assert_eq!(
            fetch("guppy://example.test/", &options_for(addr))
                .await
                .unwrap(),
            GuppyResponse::Success {
                mime: "text/gemini".to_string(),
                body: b"# Hi\n".to_vec()
            }
        );

        // URL parsing normalizes ".."; hostile selectors arrive raw.
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(addr).await.unwrap();
        socket
            .send(b"guppy://example.test/%2e%2e/secret\r\n")
            .await
            .unwrap();
        let mut buffer = vec![0u8; 4096];
        let count = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(buffer[..count].starts_with(b"4 "), "traversal answered with an error");
    }

    #[test]
    fn build_session_chunks_and_terminates() {
        let session = build_session("text/plain", b"0123456789", 4);
        assert_eq!(session.packets.len(), 4, "3 data chunks + EOF");
        let seqs: Vec<u32> = session.packets.iter().map(|(seq, _)| *seq).collect();
        assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
        assert!(seqs[0] >= MIN_SEQ);
        // EOF packet is header-only.
        let (_, eof_wire) = session.packets.last().unwrap();
        assert!(eof_wire.ends_with(b"\r\n"));
    }

    #[test]
    fn random_seq_stays_in_range() {
        for _ in 0..100 {
            let seq = random_seq(1000);
            assert!(seq >= MIN_SEQ && seq <= MAX_SEQ - 1000);
        }
    }
}
