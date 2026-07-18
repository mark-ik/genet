//! The nex server: read one selector line, answer with bytes, close.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::percent_decode_str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One nex request: the selector path, exactly as sent (CR/LF trimmed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub path: String,
    pub peer: SocketAddr,
}

/// The application seam: turn a [`Request`] into response bytes. Nex has no
/// status codes, so "not found" is whatever text the handler chooses.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> impl Future<Output = Vec<u8>> + Send;
}

impl<F, Fut> Handler for F
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Vec<u8>> + Send,
{
    fn handle(&self, request: Request) -> impl Future<Output = Vec<u8>> + Send {
        self(request)
    }
}

/// Server limits and timeouts.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Maximum selector-line length (the spec is silent; this is the server
    /// protecting itself). Default 2048.
    pub max_request_line: usize,
    /// Per-connection IO timeout. Default 30s.
    pub timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_request_line: 2048,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Accept connections on `listener` and serve them through `handler` until
/// `shutdown` resolves.
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
                            log::debug!("nex: connection from {peer} failed: {error}");
                        }
                    });
                }
                Err(error) => log::warn!("nex: accept failed: {error}"),
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
    // Selector: bytes up to LF (the spec's example is a telnet session, so
    // accept bare LF as well as CRLF), capped.
    let mut line = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let count = tokio::time::timeout(config.timeout, stream.read(&mut byte))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "selector read"))??;
        if count == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() >= config.max_request_line {
            // Over-long selector: nex has no error channel; just close.
            return stream.shutdown().await;
        }
    }
    let path = String::from_utf8_lossy(&line)
        .trim_end_matches('\r')
        .to_string();

    let body = handler.handle(Request { path, peer }).await;
    tokio::time::timeout(config.timeout, stream.write_all(&body))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "response write"))??;
    stream.shutdown().await
}

/// A static-directory [`Handler`]. Directory selectors (empty or trailing
/// `/`) serve a generated listing, or the directory's `index.nex` file when
/// present. Traversal out of the root is refused with a plain-text message.
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
        Some(resolved)
    }

    async fn listing_for(&self, dir: &PathBuf, request_path: &str) -> Vec<u8> {
        let mut names = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let mut name = entry.file_name().to_string_lossy().into_owned();
                if entry
                    .file_type()
                    .await
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
                {
                    name.push('/');
                }
                names.push(name);
            }
        }
        names.sort();
        let mut out = format!("{request_path}\n\n");
        for name in names {
            out.push_str("=> ");
            out.push_str(&name);
            out.push('\n');
        }
        out.into_bytes()
    }
}

impl Handler for FileHandler {
    async fn handle(&self, request: Request) -> Vec<u8> {
        let Some(path) = self.resolve(&request.path) else {
            return b"bad path\n".to_vec();
        };
        if crate::is_directory_path(&request.path) || path.is_dir() {
            let index = path.join("index.nex");
            if let Ok(body) = tokio::fs::read(&index).await {
                return body;
            }
            return self
                .listing_for(
                    &path,
                    if request.path.is_empty() {
                        "/"
                    } else {
                        &request.path
                    },
                )
                .await;
        }
        match tokio::fs::read(&path).await {
            Ok(body) => body,
            Err(_) => format!("{} not found\n", request.path).into_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{FetchOptions, fetch};
    use crate::{ListingLine, parse_listing};

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
    async fn round_trip_echoes_the_selector() {
        let (addr, _stop) = spawn(|request: Request| async move {
            format!("you asked for {:?}\n", request.path).into_bytes()
        })
        .await;
        let body = fetch("nex://example.test/hello.txt", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(body, b"you asked for \"/hello.txt\"\n");
    }

    #[tokio::test]
    async fn file_handler_serves_files_listings_and_refuses_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("about.txt"), "hi\n").unwrap();
        std::fs::create_dir(dir.path().join("nexlog")).unwrap();
        std::fs::write(dir.path().join("nexlog").join("one.txt"), "post\n").unwrap();

        let (addr, _stop) = spawn(FileHandler::new(dir.path())).await;

        let file = fetch("nex://example.test/about.txt", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(file, b"hi\n");

        let listing = fetch("nex://example.test/", &options_for(addr))
            .await
            .unwrap();
        let lines = parse_listing(&String::from_utf8(listing).unwrap());
        assert!(lines.contains(&ListingLine::Link {
            url: "about.txt".to_string(),
            label: None
        }));
        assert!(lines.contains(&ListingLine::Link {
            url: "nexlog/".to_string(),
            label: None
        }));

        // URL parsing normalizes dot segments away, so a hostile selector has
        // to be sent raw — which is exactly what a hostile client would do.
        let escape = crate::fetch_path("example.test", 1900, "/../secret", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(escape, b"bad path\n");
    }

    #[tokio::test]
    async fn index_nex_takes_precedence_over_generated_listings() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("index.nex"), "welcome to my site\n").unwrap();
        let (addr, _stop) = spawn(FileHandler::new(dir.path())).await;
        let body = fetch("nex://example.test/", &options_for(addr))
            .await
            .unwrap();
        assert_eq!(body, b"welcome to my site\n");
    }
}
