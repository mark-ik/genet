//! The Guppy protocol (`guppy://`, UDP port 6775), via the
//! [`guppy-protocol`](https://crates.io/crates/guppy-protocol) crate.
//!
//! Guppy (spec v0.4.4) answers a single-datagram URL request with either a
//! special packet — `1 <prompt>` / `3 <url>` / `4 <error>` — or a chunked,
//! per-packet-acknowledged success whose sequence numbers start at a random
//! value. `guppy-protocol` owns that transaction (acks, retransmission,
//! reassembly); this module maps its response onto errand's [`Response`].
//!
//! (An earlier in-house implementation here predated the crate and did not
//! match spec v0.4.4 — it expected seq-0 headers with two-digit gemini-style
//! codes. Replaced 2026-07-04.)

use guppy_protocol::{FetchOptions, GuppyResponse};
use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `guppy://` URL over UDP.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let response = guppy_protocol::fetch(url.as_str(), &FetchOptions::default())
        .await
        .map_err(map_error)?;
    Ok(map_response(url, response))
}

fn map_response(url: &Url, response: GuppyResponse) -> Response {
    // Guppy has no numeric codes for success; the specials carry their real
    // single digit (1 prompt / 3 redirect / 4 error).
    let (status, raw_status, meta, body) = match response {
        GuppyResponse::Success { mime, body } => (Status::Success, None, mime, body),
        GuppyResponse::Prompt { text } => (Status::Input, Some(1), text, Vec::new()),
        GuppyResponse::Redirect { target } => (Status::Redirect, Some(3), target, Vec::new()),
        GuppyResponse::Error { message } => (Status::Failure, Some(4), message, Vec::new()),
    };
    Response {
        url: url.clone(),
        status,
        raw_status,
        meta,
        body,
    }
}

fn map_error(error: guppy_protocol::ClientError) -> Error {
    use guppy_protocol::ClientError as GuppyError;
    match error {
        GuppyError::BadUrl(message) => Error::BadUrl(message),
        GuppyError::RequestTooLong { request_bytes, max } => Error::Protocol(format!(
            "guppy request is {request_bytes} bytes (max {max})"
        )),
        GuppyError::Io(message) => Error::Io(message),
        GuppyError::Timeout => Error::Timeout,
        GuppyError::Protocol(message) => Error::Protocol(message),
        GuppyError::BodyTooLarge { max } => {
            Error::Protocol(format!("guppy response exceeds {max} bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scheme;

    fn u() -> Url {
        Url::parse("guppy://example.org/").unwrap()
    }

    #[test]
    fn success_maps_mime_and_body() {
        let response = map_response(
            &u(),
            GuppyResponse::Success {
                mime: "text/gemini".to_string(),
                body: b"# Hi\n".to_vec(),
            },
        );
        assert_eq!(response.status, Status::Success);
        assert_eq!(response.raw_status, None);
        assert_eq!(response.mime(), Some("text/gemini"));
        assert_eq!(response.body, b"# Hi\n");
    }

    #[test]
    fn specials_map_to_input_redirect_failure() {
        let prompt = map_response(
            &u(),
            GuppyResponse::Prompt {
                text: "Your name".to_string(),
            },
        );
        assert_eq!(prompt.status, Status::Input);
        assert_eq!(prompt.raw_status, Some(1));
        assert_eq!(prompt.meta, "Your name");

        let redirect = map_response(
            &u(),
            GuppyResponse::Redirect {
                target: "/b".to_string(),
            },
        );
        assert_eq!(redirect.status, Status::Redirect);
        assert_eq!(redirect.meta, "/b");

        let failure = map_response(
            &u(),
            GuppyResponse::Error {
                message: "not found".to_string(),
            },
        );
        assert_eq!(failure.status, Status::Failure);
        assert!(failure.body.is_empty());
    }

    #[test]
    fn guppy_scheme_routes_to_port_6775() {
        assert_eq!(Scheme::Guppy.default_port(), 6775);
    }
}
