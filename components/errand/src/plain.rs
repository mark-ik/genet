//! The plaintext TCP exchange shared by gopher, finger, and spartan: connect,
//! send one request, read the whole reply to EOF. (Gemini differs only in
//! wrapping the stream in TLS, so it has its own path.)

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Error;

/// Open a plaintext TCP connection to `host:port`, send `request`, and read the
/// whole response to EOF (smolweb servers close the stream when done).
pub(crate) async fn exchange(host: &str, port: u16, request: &[u8]) -> Result<Vec<u8>, Error> {
    let mut stream = TcpStream::connect((host, port))
        .await
        .map_err(|e| Error::Connect(format!("tcp {host}:{port}: {e}")))?;
    stream
        .write_all(request)
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok(buf)
}
