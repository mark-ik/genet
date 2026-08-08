//! Titan (`titan://`, port 1965), via the
//! [`gemini-protocol`](https://crates.io/crates/gemini-protocol) crate.
//!
//! Titan is gemini's upload companion: the same TLS on the same port, with a
//! request line carrying the body's size, MIME type, and an optional token,
//! answered by an ordinary gemini response. That crate owns the transaction;
//! this module maps its response onto errand's [`Response`].

use url::Url;

use crate::gemini::map_error;
use crate::{Error, Response};

/// Navigate to a `titan://` URL by sending a zero-byte upload.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let response = gemini_protocol::titan::fetch(url)
        .await
        .map_err(map_error)?;
    Ok(crate::gemini::into_errand_response(url, response))
}

/// Upload `body` to `url` with the given `mime` type and optional `token`.
/// Returns the server's gemini-format response.
pub async fn upload(
    url: &Url,
    body: &[u8],
    mime: &str,
    token: Option<&str>,
) -> Result<Response, Error> {
    let response = gemini_protocol::titan::upload(url, body, mime, token)
        .await
        .map_err(map_error)?;
    Ok(crate::gemini::into_errand_response(url, response))
}
