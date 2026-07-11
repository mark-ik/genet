//! The finger protocol (`finger://`, port 79, RFC 1288).
//!
//! The request is a username (or empty for a listing) and a CRLF; the reply is
//! free-form text. `finger://host/user` and `finger://user@host` both name a
//! user. There is no status line, so the reply is always [`Status::Success`] as
//! `text/plain`.

use url::Url;

use crate::plain::exchange;
use crate::{Error, Response, Scheme, Status};

/// Fetch a `finger://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::BadUrl("finger URL has no host".into()))?;
    let port = url.port().unwrap_or_else(|| Scheme::Finger.default_port());
    let user = query_target(url);

    let request = format!("{user}\r\n");
    let body = exchange(host, port, request.as_bytes()).await?;
    Ok(Response {
        url: url.clone(),
        status: Status::Success,
        raw_status: None,
        meta: "text/plain".to_string(),
        body,
    })
}

/// The user to query: the path if present (`finger://host/user`), else the
/// userinfo (`finger://user@host`), else empty for a host listing.
fn query_target(url: &Url) -> String {
    let from_path = url.path().trim_start_matches('/');
    if !from_path.is_empty() {
        from_path.to_string()
    } else {
        url.username().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(u: &str) -> String {
        query_target(&Url::parse(u).unwrap())
    }

    #[test]
    fn user_from_path_or_userinfo_or_empty() {
        assert_eq!(target("finger://example.org/alice"), "alice");
        assert_eq!(target("finger://bob@example.org"), "bob");
        assert_eq!(target("finger://example.org/"), "");
    }
}
