//! The gopher protocol (`gopher://`, port 70).
//!
//! A gopher URL is `gopher://host/<type><selector>`: the first path character is
//! the item type, the rest is the selector sent verbatim. The request is just
//! the selector and a CRLF; a type-7 search appends the query after a TAB. There
//! is no status line, so every reply is a [`Status::Success`] whose MIME is
//! inferred from the item type.

use url::Url;

use crate::plain::exchange;
use crate::{Error, Response, Scheme, Status};

/// Fetch a `gopher://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::BadUrl("gopher URL has no host".into()))?;
    let port = url.port().unwrap_or_else(|| Scheme::Gopher.default_port());
    let (item_type, selector) = split_path(url);

    let mut request = selector;
    // A type-7 item is a search server: the query rides after a TAB.
    if let Some(query) = url.query() {
        request.push('\t');
        request.push_str(query);
    }
    request.push_str("\r\n");

    let body = exchange(host, port, request.as_bytes()).await?;
    Ok(Response {
        url: url.clone(),
        status: Status::Success,
        raw_status: None,
        meta: mime_for(item_type).to_string(),
        body,
    })
}

/// Split a gopher path into its item-type character and selector. An empty path
/// is the root menu (type `1`, empty selector).
fn split_path(url: &Url) -> (char, String) {
    let path = url.path();
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(item_type) => (item_type, chars.as_str().to_string()),
        None => ('1', String::new()),
    }
}

/// A best-effort MIME type for a gopher item type. Menus get an
/// `application/gopher-menu` type so a consumer can route them to a gophermap
/// renderer; unknown types fall back to opaque bytes.
fn mime_for(item_type: char) -> &'static str {
    match item_type {
        '0' => "text/plain",
        '1' | '7' => "application/gopher-menu",
        'h' => "text/html",
        'g' => "image/gif",
        'I' | ':' => "image/*",
        's' | '<' => "audio/*",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(u: &str) -> (char, String) {
        split_path(&Url::parse(u).unwrap())
    }

    #[test]
    fn root_path_is_a_menu() {
        assert_eq!(split("gopher://example.org/"), ('1', String::new()));
        assert_eq!(split("gopher://example.org"), ('1', String::new()));
    }

    #[test]
    fn type_and_selector_split_at_the_first_char() {
        assert_eq!(
            split("gopher://example.org/0/about.txt"),
            ('0', "/about.txt".into())
        );
        assert_eq!(split("gopher://example.org/1/dir"), ('1', "/dir".into()));
    }

    #[test]
    fn mime_inference() {
        assert_eq!(mime_for('0'), "text/plain");
        assert_eq!(mime_for('1'), "application/gopher-menu");
        assert_eq!(mime_for('9'), "application/octet-stream");
    }
}
