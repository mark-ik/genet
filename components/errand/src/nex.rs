//! The Nex protocol (`nex://`, port 1900, <https://nex.nightfall.city>), via
//! the [`nex-protocol`](https://crates.io/crates/nex-protocol) crate.
//!
//! Nex is the minimal smolweb protocol: plaintext TCP, no TLS, no status
//! line. The request is the URL path followed by CRLF; the response is raw
//! bytes until the server closes. There is no header, so every reply is a
//! [`Status::Success`] with no MIME type. The caller (or the nematic
//! `NexEngine`) distinguishes a directory listing from a content response by
//! inspecting the body.

use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `nex://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let body = nex_protocol::fetch(url.as_str(), &Default::default())
        .await
        .map_err(map_error)?;
    Ok(Response {
        url: url.clone(),
        status: Status::Success,
        raw_status: None,
        meta: String::new(),
        body,
    })
}

fn map_error(error: nex_protocol::ClientError) -> Error {
    use nex_protocol::ClientError as Nex;
    match error {
        Nex::BadUrl(message) => Error::BadUrl(message),
        Nex::Io(message) => Error::Io(message),
        Nex::Timeout(_) => Error::Timeout,
        Nex::BodyTooLarge { max } => Error::Protocol(format!("nex response exceeds {max} bytes")),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Scheme, Status};

    #[test]
    fn nex_scheme_routes_to_port_1900() {
        assert_eq!(Scheme::Nex.default_port(), 1900);
    }

    #[tokio::test]
    #[ignore = "hits the live network; run with `cargo test -- --ignored`"]
    async fn live_nex_smoke() {
        let r = crate::fetch("nex://nightfall.city/")
            .await
            .expect("fetch nex root");
        assert_eq!(r.status, Status::Success);
        assert!(!r.body.is_empty());
    }
}
