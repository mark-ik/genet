//! The gemini protocol (`gemini://`, port 1965), via the
//! [`gemini-protocol`](https://crates.io/crates/gemini-protocol) crate.
//!
//! That crate owns the exchange, the gemtext grammar, and the real
//! trust-on-first-use pinning; this module maps its response onto errand's
//! cross-protocol [`Response`].
//!
//! The one deliberate flattening: gemini distinguishes temporary (`4x`) from
//! permanent (`5x`) failure, and errand's [`Status`] has a single `Failure`
//! because not every smolweb protocol draws that line. The exact code survives
//! on `Response::raw_status`, so nothing is lost, and a caller that wants the
//! distinction can read it there or use `gemini-protocol` directly.

use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `gemini://` URL over TLS, with trust-on-first-use pinning.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let response = gemini_protocol::client::fetch_url(url)
        .await
        .map_err(map_error)?;
    Ok(into_errand_response(url, response))
}

/// Run a gemini request/response over an already-connected stream.
///
/// Transport-independent: an already-encrypted carrier needs no TLS, so a
/// Reticulum link drives this same path with the TLS and TOFU layer absent.
pub async fn exchange<S>(url: &Url, stream: &mut S) -> Result<Response, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let response = gemini_protocol::exchange(url, stream)
        .await
        .map_err(map_error)?;
    Ok(into_errand_response(url, response))
}

pub(crate) fn into_errand_response(url: &Url, response: gemini_protocol::Response) -> Response {
    use gemini_protocol::Status as Gemini;
    Response {
        url: url.clone(),
        status: match response.status {
            Gemini::Input => Status::Input,
            Gemini::Success => Status::Success,
            Gemini::Redirect => Status::Redirect,
            Gemini::TemporaryFailure | Gemini::PermanentFailure => Status::Failure,
            Gemini::CertificateRequired => Status::CertRequired,
        },
        raw_status: Some(response.code),
        meta: response.meta,
        body: response.body,
    }
}

pub(crate) fn map_error(error: gemini_protocol::ClientError) -> Error {
    use gemini_protocol::ClientError as Gemini;
    match error {
        Gemini::BadUrl(message) => Error::BadUrl(message),
        Gemini::Connect(message) => Error::Connect(message),
        Gemini::Io(message) => Error::Io(message),
        Gemini::Protocol(message) => Error::Protocol(message),
        Gemini::CertificateChanged {
            host,
            pinned,
            seen,
        } => Error::CertificateChanged {
            host,
            pinned,
            seen,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: gemini_protocol::Status, code: u8) -> Response {
        into_errand_response(
            &Url::parse("gemini://example.org/").unwrap(),
            gemini_protocol::Response {
                status,
                code,
                meta: "text/gemini".into(),
                body: Vec::new(),
            },
        )
    }

    #[test]
    fn both_failure_classes_flatten_but_the_code_survives() {
        let temporary = response(gemini_protocol::Status::TemporaryFailure, 44);
        assert_eq!(temporary.status, Status::Failure);
        assert_eq!(temporary.raw_status, Some(44));

        let permanent = response(gemini_protocol::Status::PermanentFailure, 51);
        assert_eq!(permanent.status, Status::Failure);
        assert_eq!(permanent.raw_status, Some(51));
    }

    #[test]
    fn the_other_classes_map_one_to_one() {
        assert_eq!(
            response(gemini_protocol::Status::Success, 20).status,
            Status::Success
        );
        assert_eq!(
            response(gemini_protocol::Status::Input, 10).status,
            Status::Input
        );
        assert_eq!(
            response(gemini_protocol::Status::Redirect, 31).status,
            Status::Redirect
        );
        assert_eq!(
            response(gemini_protocol::Status::CertificateRequired, 60).status,
            Status::CertRequired
        );
    }
}
