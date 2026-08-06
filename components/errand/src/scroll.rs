//! The scroll protocol (`scroll://`, port 5699), via the
//! [`scroll-protocol`](https://crates.io/crates/scroll-protocol) crate.
//!
//! That crate owns the exchange, the scrolltext grammar, and the language
//! negotiation; this module maps its response onto errand's cross-protocol
//! [`Response`].
//!
//! Two deliberate flattenings, both recoverable. [`fetch`] sends **no language
//! preference** (the server picks its default), and the success metadata
//! (author, publish date, modification date) does not fit errand's normalized
//! `Response` — but the UDC classification survives on
//! [`Response::raw_status`], because scroll carries it in the status code
//! itself. A caller that wants languages, metadata, or abstracts uses
//! [`fetch_with`] (exported as `errand::scroll_fetch`), which returns the
//! protocol crate's own types, exactly as `titan_upload` and `misfin_send`
//! already do for their protocols.

use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `scroll://` URL with no language preference.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let response = scroll_protocol::fetch(url.as_str(), &[], false)
        .await
        .map_err(map_error)?;
    Ok(into_errand_response(url, response))
}

/// Fetch with the full scroll surface: language preferences (BCP47, most
/// preferred first) and, when `metadata` is set, the resource's abstract
/// instead of its body. Returns the protocol crate's own response, with
/// author, dates, and UDC class intact.
pub async fn fetch_with(
    url: &str,
    languages: &[&str],
    metadata: bool,
) -> Result<scroll_protocol::Response, Error> {
    scroll_protocol::fetch(url, languages, metadata)
        .await
        .map_err(map_error)
}

fn into_errand_response(url: &Url, response: scroll_protocol::Response) -> Response {
    use scroll_protocol::{Header, Status as Scroll};
    match response.header {
        Header::Success(header) => Response {
            url: url.clone(),
            status: Status::Success,
            // The UDC class rides here: scroll's 24 is "general", 27 is
            // "arts", and the second digit is the classification.
            raw_status: Some(header.code),
            meta: header.mimetype,
            body: response.body,
        },
        Header::Meta { code, status, meta } => Response {
            url: url.clone(),
            status: match status {
                Scroll::Input => Status::Input,
                Scroll::Success => Status::Success,
                Scroll::Redirect => Status::Redirect,
                Scroll::TemporaryFailure | Scroll::PermanentFailure => Status::Failure,
                Scroll::CertificateRequired => Status::CertRequired,
            },
            raw_status: Some(code),
            meta,
            body: Vec::new(),
        },
    }
}

fn map_error(error: scroll_protocol::ClientError) -> Error {
    use scroll_protocol::ClientError as Scroll;
    match error {
        Scroll::BadUrl(message) => Error::BadUrl(message),
        Scroll::Connect(message) => Error::Connect(message),
        Scroll::Io(message) => Error::Io(message),
        Scroll::Protocol(message) => Error::Protocol(message),
        Scroll::CertificateChanged { host, pinned, seen } => {
            Error::CertificateChanged { host, pinned, seen }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scroll_protocol::{Header, finish_success};

    #[test]
    fn a_success_keeps_its_udc_code_and_mimetype() {
        let response = into_errand_response(
            &Url::parse("scroll://example.net/").unwrap(),
            scroll_protocol::Response {
                header: Header::Success(finish_success(
                    27,
                    "text/scroll".into(),
                    "Author",
                    "2025-07-23T20:50:51Z",
                    "",
                )),
                body: b"# Title\n".to_vec(),
            },
        );
        assert_eq!(response.status, Status::Success);
        assert_eq!(response.raw_status, Some(27), "the UDC class survives");
        assert_eq!(response.mime(), Some("text/scroll"));
        assert_eq!(response.body, b"# Title\n");
    }

    #[test]
    fn the_other_classes_map_like_geminis() {
        let response = into_errand_response(
            &Url::parse("scroll://example.net/").unwrap(),
            scroll_protocol::Response {
                header: Header::Meta {
                    code: 51,
                    status: scroll_protocol::Status::PermanentFailure,
                    meta: "gone".into(),
                },
                body: Vec::new(),
            },
        );
        assert_eq!(response.status, Status::Failure);
        assert_eq!(response.raw_status, Some(51));
    }
}
