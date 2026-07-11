//! The gemini protocol (`gemini://`, port 1965).
//!
//! The exchange is small: open TLS (trust-on-first-use, with cert pinning),
//! send the absolute URL followed by `\r\n`, then read a `<status> <meta>\r\n`
//! header and, for a 2x success, the body that follows. The server closes the
//! connection at the end, so the body is whatever remains after the header
//! line.
//!
//! Trust is real TOFU: the host's pinned fingerprint (from the installed
//! [`crate::TofuStore`]) is checked during the handshake, a first contact is
//! pinned after it completes, and a changed certificate surfaces as
//! [`Error::CertificateChanged`] before the request is ever sent.

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use crate::{tls, tofu, Error, Response, Scheme, Status};

/// The largest request line gemini permits, in bytes (the spec caps the URL at
/// 1024; the trailing CRLF rides within that budget here).
const MAX_REQUEST: usize = 1024;

/// Fetch a `gemini://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::BadUrl("gemini URL has no host".into()))?;
    let port = url.port().unwrap_or_else(|| Scheme::Gemini.default_port());

    let request = format!("{url}\r\n");
    if request.len() > MAX_REQUEST {
        return Err(Error::Protocol(format!("request exceeds {MAX_REQUEST} bytes")));
    }

    // Look the host's pin up before connecting (so the verifier stays
    // 'static), then wrap TCP in a pinning TLS handshake.
    let store = tofu::trust_store();
    let pinned = store.fingerprint(host);
    let (connector, seen) = tls::pinning_connector(pinned);

    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| Error::Connect(format!("tcp {host}:{port}: {e}")))?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| Error::Connect(format!("server name {host}: {e}")))?;
    let mut tls = match connector.connect(server_name, tcp).await {
        Ok(tls) => tls,
        Err(e) => {
            // A pin mismatch surfaces richly; the verifier recorded what it
            // saw before rejecting the handshake.
            if let (Some(pinned), Some(seen)) = (pinned, *seen.lock().unwrap()) {
                if pinned != seen {
                    return Err(Error::CertificateChanged {
                        host: host.to_string(),
                        pinned: tofu::hex(&pinned),
                        seen: tofu::hex(&seen),
                    });
                }
            }
            return Err(Error::Connect(format!("tls handshake: {e}")));
        }
    };

    // Clean handshake: pin the fingerprint on first contact.
    if pinned.is_none() {
        if let Some(fingerprint) = *seen.lock().unwrap() {
            store.pin(host, fingerprint);
        }
    }

    // Send the request line, then read the whole response to EOF (the server
    // closes the stream when done).
    tls.write_all(request.as_bytes())
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)
        .await
        .map_err(|e| Error::Io(e.to_string()))?;

    parse(url, &raw)
}

/// Split a gemini response into its `<status> <meta>\r\n` header and the body.
pub(crate) fn parse(url: &Url, raw: &[u8]) -> Result<Response, Error> {
    let split = raw
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or_else(|| Error::Protocol("response header has no CRLF".into()))?;
    let header = std::str::from_utf8(&raw[..split])
        .map_err(|_| Error::Protocol("response header is not UTF-8".into()))?;
    let body = raw[split + 2..].to_vec();

    // The header is two status digits, a space, then a meta string (which may be
    // empty). The two digits classify the response; the first digit is the class.
    let bytes = header.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return Err(Error::Protocol(format!("bad gemini status: {header:?}")));
    }
    let code = (bytes[0] - b'0') * 10 + (bytes[1] - b'0');
    let meta = header.get(2..).unwrap_or("").trim_start().to_string();

    let status = match bytes[0] {
        b'1' => Status::Input,
        b'2' => Status::Success,
        b'3' => Status::Redirect,
        b'4' | b'5' => Status::Failure,
        b'6' => Status::CertRequired,
        _ => return Err(Error::Protocol(format!("unknown gemini status class: {code}"))),
    };

    Ok(Response {
        url: url.clone(),
        status,
        raw_status: Some(code),
        meta,
        // Only a success carries a body; for other statuses meta is the payload.
        body: if status == Status::Success { body } else { Vec::new() },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u() -> Url {
        Url::parse("gemini://example.org/").unwrap()
    }

    #[test]
    fn parses_success_header_and_body() {
        let raw = b"20 text/gemini; charset=utf-8\r\n# Hello\nworld\n";
        let r = parse(&u(), raw).unwrap();
        assert_eq!(r.status, Status::Success);
        assert_eq!(r.raw_status, Some(20));
        assert_eq!(r.mime(), Some("text/gemini"));
        assert_eq!(r.body, b"# Hello\nworld\n");
    }

    #[test]
    fn redirect_meta_is_the_target_and_body_is_dropped() {
        let raw = b"31 gemini://example.org/moved\r\nignored";
        let r = parse(&u(), raw).unwrap();
        assert_eq!(r.status, Status::Redirect);
        assert_eq!(r.meta, "gemini://example.org/moved");
        assert!(r.body.is_empty());
    }

    #[test]
    fn failure_class_maps_to_failure() {
        let r = parse(&u(), b"51 not found\r\n").unwrap();
        assert_eq!(r.status, Status::Failure);
        assert_eq!(r.raw_status, Some(51));
        assert_eq!(r.meta, "not found");
    }

    #[test]
    fn cert_required_class() {
        let r = parse(&u(), b"60 client cert required\r\n").unwrap();
        assert_eq!(r.status, Status::CertRequired);
    }

    #[test]
    fn empty_meta_is_fine() {
        let r = parse(&u(), b"20 \r\nbody").unwrap();
        assert_eq!(r.mime(), None);
        assert_eq!(r.body, b"body");
    }

    #[test]
    fn missing_crlf_is_a_protocol_error() {
        assert!(matches!(parse(&u(), b"20 text/gemini"), Err(Error::Protocol(_))));
    }

    #[test]
    fn non_numeric_status_is_a_protocol_error() {
        assert!(matches!(parse(&u(), b"xx nope\r\n"), Err(Error::Protocol(_))));
    }

    #[tokio::test]
    #[ignore = "hits the live network; run with `cargo test -- --ignored`"]
    async fn live_capsule_smoke() {
        let r = crate::fetch("gemini://geminiprotocol.net/")
            .await
            .expect("fetch the gemini project capsule");
        assert_eq!(r.status, Status::Success, "meta was {:?}", r.meta);
        assert!(r.body.len() > 100, "expected a real page, got {} bytes", r.body.len());
    }

    /// The full TOFU loop against a real capsule: first fetch pins, second
    /// fetch matches the pin, and a corrupted pin is caught as a changed
    /// certificate. Uses an in-memory store installed for the process.
    #[tokio::test]
    #[ignore = "hits the live network; run with `cargo test -- --ignored`"]
    async fn live_tofu_pins_then_detects_a_change() {
        use crate::TofuStore;
        use std::sync::Arc;

        let store = Arc::new(crate::InMemoryTofu::new());
        crate::set_trust_store(store.clone());

        // First contact pins the capsule's certificate.
        let first = crate::fetch("gemini://geminiprotocol.net/").await.unwrap();
        assert_eq!(first.status, Status::Success);
        let pinned = store
            .fingerprint("geminiprotocol.net")
            .expect("first contact pins the cert");

        // Second fetch presents the same cert: the pin matches, so it works.
        let second = crate::fetch("gemini://geminiprotocol.net/").await.unwrap();
        assert_eq!(second.status, Status::Success);
        assert_eq!(store.fingerprint("geminiprotocol.net"), Some(pinned));

        // Corrupt the pin to simulate a changed cert: the next fetch must
        // refuse before sending the request.
        store.pin("geminiprotocol.net", [0u8; 32]);
        let err = crate::fetch("gemini://geminiprotocol.net/").await.unwrap_err();
        assert!(
            matches!(err, Error::CertificateChanged { .. }),
            "a changed cert must surface as CertificateChanged, got {err:?}"
        );

        // Restore a permissive store so other ignored tests are unaffected.
        crate::set_trust_store(Arc::new(crate::PermissiveTofu));
    }
}
