//! The spartan server: a TCP accept loop over a pluggable [`Handler`].

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::Status;

/// One parsed spartan request: the request line's three components plus the
/// uploaded data block (empty for a plain fetch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The host the client addressed (spec §2: no port). Lets one server
    /// serve several hosts; single-host handlers may ignore it.
    pub host: String,
    /// The absolute, %-encoded request path (always begins with `/`).
    pub path: String,
    /// The uploaded data block; empty when content-length was 0.
    pub data: Vec<u8>,
    /// The client's socket address.
    pub peer: SocketAddr,
}

/// What a handler answers: one of the four spec responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpartanResponse {
    /// `2 <mime>` + body.
    Success { mime: String, body: Vec<u8> },
    /// `3 <absolute-path>` (same host only, per spec §3).
    Redirect { path: String },
    /// `4 <message>`.
    ClientError { message: String },
    /// `5 <message>`.
    ServerError { message: String },
}

impl SpartanResponse {
    pub fn status(&self) -> Status {
        match self {
            Self::Success { .. } => Status::Success,
            Self::Redirect { .. } => Status::Redirect,
            Self::ClientError { .. } => Status::ClientError,
            Self::ServerError { .. } => Status::ServerError,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let (meta, body): (&str, &[u8]) = match self {
            Self::Success { mime, body } => (mime, body),
            Self::Redirect { path } => (path, &[]),
            Self::ClientError { message } => (message, &[]),
            Self::ServerError { message } => (message, &[]),
        };
        let mut out = format!("{} {meta}\r\n", self.status().code()).into_bytes();
        out.extend_from_slice(body);
        out
    }
}

/// The application seam: turn a [`Request`] into a [`SpartanResponse`].
/// Implemented for async closures.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> impl Future<Output = SpartanResponse> + Send;
}

impl<F, Fut> Handler for F
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = SpartanResponse> + Send,
{
    fn handle(&self, request: Request) -> impl Future<Output = SpartanResponse> + Send {
        self(request)
    }
}

/// Server limits and timeouts.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Maximum request-line length. The spec's grammar is unbounded; this is
    /// the server protecting itself. Default 4096.
    pub max_request_line: usize,
    /// Maximum accepted content-length for uploads. Requests declaring more
    /// are answered `4` without reading the block. Default 4 MiB.
    pub max_upload: usize,
    /// Per-connection IO timeout. Default 30s.
    pub timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_request_line: 4096,
            max_upload: 4 * 1024 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Accept connections on `listener` and serve them through `handler` until
/// `shutdown` resolves. Each connection is one request-response, per spec §1.
pub async fn serve(
    listener: TcpListener,
    handler: impl Handler,
    config: ServerConfig,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let handler = Arc::new(handler);
    let config = Arc::new(config);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let handler = handler.clone();
                    let config = config.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle_connection(stream, peer, handler, &config).await {
                            log::debug!("spartan: connection from {peer} failed: {error}");
                        }
                    });
                }
                Err(error) => log::warn!("spartan: accept failed: {error}"),
            },
        }
    }
    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<impl Handler>,
    config: &ServerConfig,
) -> std::io::Result<()> {
    let response = match read_request(&mut stream, peer, config).await? {
        Ok(request) => handler.handle(request).await,
        Err(message) => SpartanResponse::ClientError { message },
    };
    tokio::time::timeout(config.timeout, stream.write_all(&response.encode()))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "response write"))??;
    stream.shutdown().await
}

/// Read and parse one request. Protocol violations come back as
/// `Ok(Err(message))` so the caller can answer `4`; IO failures are `Err`.
async fn read_request(
    stream: &mut TcpStream,
    peer: SocketAddr,
    config: &ServerConfig,
) -> std::io::Result<Result<Request, String>> {
    // Request line: bytes up to CRLF, capped.
    let mut line = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let count = tokio::time::timeout(config.timeout, stream.read(&mut byte))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "request read"))??;
        if count == 0 {
            break;
        }
        line.push(byte[0]);
        if line.ends_with(b"\r\n") {
            break;
        }
        if line.len() >= config.max_request_line {
            return Ok(Err("Request line too long.".to_string()));
        }
    }
    let Ok(line) = std::str::from_utf8(&line) else {
        return Ok(Err("Request line is not ASCII.".to_string()));
    };
    let line = line.trim_end_matches(['\r', '\n']);

    // host SP path SP content-length
    let mut parts = line.split(' ');
    let (Some(host), Some(path), Some(length), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(Err("Malformed request line.".to_string()));
    };
    if host.is_empty() || host.contains(':') {
        // Spec §2: the port number is not included in the host component.
        // (Bracketed IPv6 hosts contain ':' but no port marker we can tell
        // apart cheaply; accept them.)
        if !(host.starts_with('[') && host.ends_with(']')) {
            return Ok(Err("Malformed host.".to_string()));
        }
    }
    if !path.starts_with('/') {
        return Ok(Err("Path must be absolute.".to_string()));
    }
    let Ok(length) = length.parse::<usize>() else {
        return Ok(Err("Malformed content-length.".to_string()));
    };
    if length > config.max_upload {
        return Ok(Err(format!(
            "Data block exceeds this server's limit of {} bytes.",
            config.max_upload
        )));
    }

    let mut data = vec![0u8; length];
    if length > 0 {
        tokio::time::timeout(config.timeout, stream.read_exact(&mut data))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "data block read"))??;
    }

    Ok(Ok(Request {
        host: host.to_string(),
        path: path.to_string(),
        data,
        peer,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FetchOptions, fetch, submit};
    use crate::{FileHandler, Status};

    async fn spawn(handler: impl Handler) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = serve(listener, handler, ServerConfig::default(), async {
                let _ = rx.await;
            })
            .await;
        });
        (addr, tx)
    }

    fn options_for(addr: SocketAddr) -> FetchOptions {
        FetchOptions {
            connect_addr: Some(addr),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn round_trip_fetch_and_upload() {
        let (addr, _stop) = spawn(|request: Request| async move {
            if request.data.is_empty() {
                SpartanResponse::Success {
                    mime: "text/gemini".to_string(),
                    body: format!("# You asked for {}\n", request.path).into_bytes(),
                }
            } else {
                SpartanResponse::Success {
                    mime: "text/plain".to_string(),
                    body: request.data,
                }
            }
        })
        .await;

        let response = fetch("spartan://example.test/about", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(response.status, Status::Success);
        assert_eq!(response.meta, "text/gemini");
        assert_eq!(response.body, b"# You asked for /about\n");

        let echoed = submit(
            "spartan://example.test/echo",
            b"Hello world!",
            &options_for(addr),
        )
        .await
        .unwrap();
        assert_eq!(echoed.body, b"Hello world!");
    }

    #[tokio::test]
    async fn query_component_uploads_as_the_data_block() {
        let (addr, _stop) = spawn(|request: Request| async move {
            SpartanResponse::Success {
                mime: "text/plain".to_string(),
                body: request.data,
            }
        })
        .await;
        let response = fetch("spartan://example.test/?hello%20world", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(response.body, b"hello world", "spec §5 query mapping");
    }

    #[tokio::test]
    async fn redirects_and_errors_come_back_bodyless() {
        let (addr, _stop) = spawn(|request: Request| async move {
            match request.path.as_str() {
                "/old" => SpartanResponse::Redirect {
                    path: "/new".to_string(),
                },
                _ => SpartanResponse::ClientError {
                    message: "no such page".to_string(),
                },
            }
        })
        .await;
        let redirect = fetch("spartan://example.test/old", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(redirect.redirect_path(), Some("/new"));
        let missing = fetch("spartan://example.test/ghost", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(missing.status, Status::ClientError);
        assert_eq!(missing.meta, "no such page");
    }

    #[tokio::test]
    async fn relative_paths_are_rejected_with_4() {
        let (addr, _stop) = spawn(|_request: Request| async move {
            SpartanResponse::ServerError {
                message: "handler should not run".to_string(),
            }
        })
        .await;
        // Raw request with a relative path.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"example.test relative/path 0\r\n")
            .await
            .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        assert!(raw.starts_with(b"4 "), "got: {raw:?}");
    }

    #[tokio::test]
    async fn file_handler_serves_gemtext_and_refuses_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("index.gmi"), "# Welcome\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "plain\n").unwrap();

        let (addr, _stop) = spawn(FileHandler::new(dir.path())).await;

        let index = fetch("spartan://example.test/", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(index.meta, "text/gemini");
        assert_eq!(index.body, b"# Welcome\n");

        let text = fetch("spartan://example.test/notes.txt", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(text.meta, "text/plain");

        let escape = fetch(
            "spartan://example.test/%2e%2e/%2e%2e/etc/passwd",
            &options_for(addr),
        )
        .await
        .unwrap();
        assert_eq!(escape.status, Status::ClientError, "traversal refused");

        let upload = submit(
            "spartan://example.test/notes.txt",
            b"data",
            &options_for(addr),
        )
        .await
        .unwrap();
        assert_eq!(
            upload.status,
            Status::ClientError,
            "static handler refuses uploads"
        );
    }
}
