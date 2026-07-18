//! The spartan client: one request per connection, per spec §2/§3/§5.

use std::time::Duration;

use percent_encoding::percent_decode_str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use super::{SPARTAN_PORT, Status};

/// Options for a [`fetch`] or [`submit`].
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Per-step timeout (connect, write, read). Default 30s.
    pub timeout: Duration,
    /// Refuse response bodies larger than this. Default 16 MiB.
    pub max_body: usize,
    /// Connect here instead of resolving the URL's host (the URL's host is
    /// still sent in the request line). For tests and odd deployments.
    pub connect_addr: Option<std::net::SocketAddr>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body: 16 * 1024 * 1024,
            connect_addr: None,
        }
    }
}

/// A parsed spartan response: the status line's digit and META, plus the body
/// (present only on success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: Status,
    /// MIME type (success), redirect path (redirect), or error message.
    pub meta: String,
    pub body: Vec<u8>,
}

impl Response {
    /// For a redirect: the absolute path to re-request **on the same host**
    /// (the spec forbids cross-host redirects).
    pub fn redirect_path(&self) -> Option<&str> {
        (self.status == Status::Redirect).then_some(self.meta.as_str())
    }
}

/// Why a request failed before a response was parsed.
#[derive(Debug)]
pub enum ClientError {
    BadUrl(String),
    Io(String),
    Timeout(&'static str),
    Protocol(String),
    BodyTooLarge { max: usize },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadUrl(message) => write!(formatter, "bad spartan URL: {message}"),
            Self::Io(message) => write!(formatter, "spartan IO error: {message}"),
            Self::Timeout(step) => write!(formatter, "spartan {step} timed out"),
            Self::Protocol(message) => write!(formatter, "spartan protocol error: {message}"),
            Self::BodyTooLarge { max } => {
                write!(formatter, "spartan response body exceeds {max} bytes")
            },
        }
    }
}

impl std::error::Error for ClientError {}

/// Fetch a `spartan://` URL. Per spec §5, the URL's query component
/// (%-decoded) becomes the request's data block, so a plain fetch sends
/// content-length 0 and a `?query` URL uploads the query.
pub async fn fetch(url: &str, options: &FetchOptions) -> Result<Response, ClientError> {
    let (host, port, path, query_data) = split_url(url)?;
    request(
        &host,
        port,
        &path,
        query_data.as_deref().unwrap_or(&[]),
        options,
    )
    .await
}

/// Upload `data` as the request's data block (the `=:` prompt-line flow).
/// Any query component in the URL is ignored in favor of `data`.
pub async fn submit(
    url: &str,
    data: &[u8],
    options: &FetchOptions,
) -> Result<Response, ClientError> {
    let (host, port, path, _) = split_url(url)?;
    request(&host, port, &path, data, options).await
}

/// Map a `spartan://` URL to (request-line host, port, %-encoded path,
/// %-decoded query data) per spec §5. The `url` crate handles IDN → punycode.
fn split_url(input: &str) -> Result<(String, u16, String, Option<Vec<u8>>), ClientError> {
    let url = Url::parse(input).map_err(|error| ClientError::BadUrl(error.to_string()))?;
    if url.scheme() != "spartan" {
        return Err(ClientError::BadUrl(format!(
            "expected spartan:// scheme, got {}://",
            url.scheme()
        )));
    }
    let host = request_host(
        url.host_str()
            .ok_or_else(|| ClientError::BadUrl("URL has no host".to_string()))?,
    )?;
    let port = url.port().unwrap_or(SPARTAN_PORT);
    let path = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    let query_data = url
        .query()
        .map(|query| percent_decode_str(query).collect::<Vec<u8>>());
    Ok((host, port, path, query_data))
}

/// The host as it goes on the request line: IDNs as punycode (spec §2). The
/// `url` crate treats non-special schemes' hosts as opaque and %-encodes
/// non-ASCII, so decode and run IDNA ourselves. Bracketed IPv6 passes through.
fn request_host(raw: &str) -> Result<String, ClientError> {
    if raw.is_ascii() && !raw.contains('%') {
        return Ok(raw.to_string());
    }
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| ClientError::BadUrl("host is not UTF-8".to_string()))?;
    if decoded.is_ascii() {
        return Ok(decoded.into_owned());
    }
    idna::domain_to_ascii(&decoded)
        .map_err(|error| ClientError::BadUrl(format!("IDN conversion failed: {error}")))
}

async fn request(
    host: &str,
    port: u16,
    path: &str,
    data: &[u8],
    options: &FetchOptions,
) -> Result<Response, ClientError> {
    let request_line = format!("{host} {path} {}\r\n", data.len());

    let connect = async {
        match options.connect_addr {
            Some(addr) => TcpStream::connect(addr).await,
            None => TcpStream::connect((host, port)).await,
        }
    };
    let mut stream = tokio::time::timeout(options.timeout, connect)
        .await
        .map_err(|_| ClientError::Timeout("connect"))?
        .map_err(|error| ClientError::Io(error.to_string()))?;

    let write = async {
        stream.write_all(request_line.as_bytes()).await?;
        if !data.is_empty() {
            stream.write_all(data).await?;
        }
        stream.flush().await
    };
    tokio::time::timeout(options.timeout, write)
        .await
        .map_err(|_| ClientError::Timeout("request write"))?
        .map_err(|error| ClientError::Io(error.to_string()))?;

    // The response body ends at connection close, so read to EOF under the
    // size cap. The cap counts header + body; close enough for protection.
    let read = async {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let count = stream.read(&mut chunk).await?;
            if count == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..count]);
            if raw.len() > options.max_body {
                return Ok::<_, std::io::Error>(None);
            }
        }
        Ok(Some(raw))
    };
    let raw = tokio::time::timeout(options.timeout, read)
        .await
        .map_err(|_| ClientError::Timeout("response read"))?
        .map_err(|error| ClientError::Io(error.to_string()))?
        .ok_or(ClientError::BodyTooLarge {
            max: options.max_body,
        })?;

    parse_response(&raw)
}

/// Split a raw response into its `<digit> <meta>\r\n` status line and body.
pub(crate) fn parse_response(raw: &[u8]) -> Result<Response, ClientError> {
    let split = raw
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| ClientError::Protocol("status line has no CRLF".to_string()))?;
    let line = std::str::from_utf8(&raw[..split])
        .map_err(|_| ClientError::Protocol("status line is not UTF-8".to_string()))?;
    let body = &raw[split + 2..];

    let (code, meta) = match line.split_once(' ') {
        Some((code, meta)) => (code, meta.trim().to_string()),
        None => (line, String::new()),
    };
    let code: u8 = code
        .parse()
        .map_err(|_| ClientError::Protocol(format!("bad status line: {line:?}")))?;
    let status = Status::from_code(code)
        .ok_or_else(|| ClientError::Protocol(format!("unknown status digit: {code}")))?;

    Ok(Response {
        status,
        meta,
        // Only a success carries a body; the spec gives 3/4/5 a bare status line.
        body: if status == Status::Success {
            body.to_vec()
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_map_to_request_components_per_spec_table() {
        // The spec §5.1 reference table.
        let (host, port, path, data) = split_url("spartan://example.com").unwrap();
        assert_eq!(
            (host.as_str(), port, path.as_str()),
            ("example.com", 300, "/")
        );
        assert!(data.is_none());

        let (_, port, ..) = split_url("spartan://example.com:3000/").unwrap();
        assert_eq!(port, 3000);

        // The spec table prints "xn--exampl-dma.com", but that suffix belongs
        // to its café example; IDNA for "examplé" is gva (position matters in
        // punycode). Python's codec agrees. A spec-table erratum, not a bug.
        let (host, ..) = split_url("spartan://examplé.com/").unwrap();
        assert_eq!(host, "xn--exampl-gva.com", "IDN hosts convert to punycode");

        let (_, _, path, _) = split_url("spartan://example.com/my%20file.txt").unwrap();
        assert_eq!(path, "/my%20file.txt", "paths stay %-encoded");

        let (_, _, _, data) = split_url("spartan://example.com?a=1&b=2").unwrap();
        assert_eq!(data.unwrap(), b"a=1&b=2");

        let (_, _, _, data) = split_url("spartan://example.com?hello%20world").unwrap();
        assert_eq!(
            data.unwrap(),
            b"hello world",
            "query %-decodes into the data block"
        );
    }

    #[test]
    fn responses_parse_all_four_statuses() {
        let success = parse_response(b"2 text/gemini\r\n# Hi\n").unwrap();
        assert_eq!(success.status, Status::Success);
        assert_eq!(success.meta, "text/gemini");
        assert_eq!(success.body, b"# Hi\n");

        let redirect = parse_response(b"3 /new/path/\r\n").unwrap();
        assert_eq!(redirect.redirect_path(), Some("/new/path/"));
        assert!(redirect.body.is_empty());

        assert_eq!(
            parse_response(b"4 not found\r\n").unwrap().status,
            Status::ClientError
        );
        assert_eq!(
            parse_response(b"5 boom\r\n").unwrap().status,
            Status::ServerError
        );
    }

    #[test]
    fn malformed_responses_are_protocol_errors() {
        assert!(parse_response(b"2 text/plain").is_err(), "no CRLF");
        assert!(parse_response(b"9 what\r\n").is_err(), "unknown digit");
        assert!(parse_response(b"abc\r\n").is_err(), "non-numeric");
    }
}
