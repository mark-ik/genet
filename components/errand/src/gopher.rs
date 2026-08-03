//! The gopher protocol (`gopher://`, port 70), via the
//! [`gopher-protocol`](https://crates.io/crates/gopher-protocol) crate.
//!
//! That crate owns the transaction and the RFC 1436 grammar, including the
//! Gopher+ successor; this module maps its reply onto errand's [`Response`].
//! Gopher has no status line, so every reply is a [`Status::Success`] whose
//! MIME the item type implies.

use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `gopher://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let reply = gopher_protocol::fetch(url.as_str())
        .await
        .map_err(map_error)?;
    Ok(Response {
        url: url.clone(),
        status: Status::Success,
        raw_status: None,
        meta: reply.mime,
        body: reply.body,
    })
}

fn map_error(error: gopher_protocol::ClientError) -> Error {
    use gopher_protocol::ClientError as Gopher;
    match error {
        Gopher::BadUrl(message) => Error::BadUrl(message),
        Gopher::Connect(message) => Error::Connect(message),
        Gopher::Io(message) => Error::Io(message),
        // Gopher+ transactions are not on errand's fetch path, so these two
        // cannot arise here; mapping them to Protocol keeps the match total
        // without inventing a status.
        Gopher::BadPlusHeader(message) | Gopher::PlusError(message) => Error::Protocol(message),
    }
}
