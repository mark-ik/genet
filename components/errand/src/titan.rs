//! The Titan protocol (`titan://`, port 1965, <https://transjovian.org/titan>).
//!
//! Titan is the upload/write companion to Gemini. The client opens a TLS
//! connection (same TOFU verifier as Gemini), sends a request line of the form
//!
//!   `<titan-url>;size=<n>[;mime=<type>][;token=<token>]\r\n`
//!
//! immediately followed by `<n>` bytes of body, then reads a Gemini-format
//! response header (`<code> <meta>\r\n`) and optional body.
//!
//! ## Navigation (`fetch`)
//!
//! When the host navigates to a `titan://` URL without a payload (e.g. the user
//! clicks a titan:// link), `fetch` sends a zero-byte upload. The server
//! typically replies with a redirect (`30`/`31`) to the read location or a
//! failure; whatever it returns is parsed as a Gemini response.
//!
//! ## Upload (`upload`)
//!
//! For actual writes, call [`upload`] directly with the body bytes, MIME type,
//! and optional token.

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use crate::gemini::parse as parse_gemini;
use crate::tls::connector;
use crate::{Error, Response, Scheme};

/// Navigate to a `titan://` URL by sending a zero-byte upload and returning the
/// server's Gemini-format response.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    upload_inner(url, &[], "", None).await
}

/// Upload `body` to `url` with the given `mime` type and optional `token`.
/// Returns the server's Gemini-format response.
///
/// The request line is `<url>;size=<n>[;mime=<type>][;token=<token>]\r\n`
/// followed immediately by the body bytes.
pub async fn upload(
    url: &Url,
    body: &[u8],
    mime: &str,
    token: Option<&str>,
) -> Result<Response, Error> {
    upload_inner(url, body, mime, token).await
}

async fn upload_inner(
    url: &Url,
    body: &[u8],
    mime: &str,
    token: Option<&str>,
) -> Result<Response, Error> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::BadUrl("titan URL has no host".into()))?;
    let port = url.port().unwrap_or_else(|| Scheme::Titan.default_port());

    // Build the request line.
    let mut request = format!("{url};size={}", body.len());
    if !mime.is_empty() {
        request.push_str(";mime=");
        request.push_str(mime);
    }
    if let Some(tok) = token {
        request.push_str(";token=");
        request.push_str(tok);
    }
    request.push_str("\r\n");

    // Open TLS (same TOFU connector as gemini — they share port 1965).
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| Error::Connect(format!("tcp {host}:{port}: {e}")))?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| Error::Connect(format!("server name {host}: {e}")))?;
    let mut tls = connector()
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Connect(format!("tls handshake: {e}")))?;

    // Send request line + body.
    tls.write_all(request.as_bytes())
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    if !body.is_empty() {
        tls.write_all(body)
            .await
            .map_err(|e| Error::Io(e.to_string()))?;
    }

    // Read and parse the Gemini-format response.
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)
        .await
        .map_err(|e| Error::Io(e.to_string()))?;

    parse_gemini(url, &raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titan_shares_gemini_port() {
        assert_eq!(Scheme::Titan.default_port(), Scheme::Gemini.default_port());
    }
}
