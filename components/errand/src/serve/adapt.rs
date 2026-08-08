//! Binding a [`Source`] to each protocol's handler.
//!
//! This is the denormalization: one source, answered in each protocol's own
//! terms. Most of the interest is in how differently they express the same
//! outcomes, which is exactly what a faithful projection has to respect.
//!
//! | | not found | redirect | wants input |
//! |---|---|---|---|
//! | gemini | status `51` | status `30` | status `10` |
//! | gopher | a `3` error item in a menu | a link in a menu | an info line |
//! | finger | a line of text | a line of text | a line of text |
//!
//! Gemini has a status for every case. Gopher has no status channel at all, so
//! everything becomes an item in a menu. Finger has neither, so everything is
//! prose. None of these is a workaround; each is what the protocol actually
//! offers, and inventing more would be extending formats we do not own.
//!
//! **Percent-decoding is not applied.** A request path is used as sent, so a
//! filename containing an encoded space will not resolve yet. Decoding has to
//! happen before the traversal guard walks the path, never after, so it is
//! left for a change that can be tested as one piece.

use std::sync::Arc;

use super::project::{to_gemtext, to_gophermap, to_plain_text};
use super::{Item, Listing, Source, SourceRequest};
use crate::Scheme;

/// Answer gemini requests from `source`.
pub fn gemini<S: Source>(source: Arc<S>) -> impl gemini_protocol::server::Handler {
    move |request: gemini_protocol::server::Request| {
        let source = source.clone();
        async move {
            use gemini_protocol::server::Reply;

            let found = source
                .get(&SourceRequest {
                    scheme: Scheme::Gemini,
                    path: request.path().to_string(),
                    query: request.query().map(str::to_string),
                })
                .await;

            match found {
                Some(Item::Document { mime, body }) => Reply::success(mime, body),
                Some(Item::Listing(listing)) => Reply::gemtext(to_gemtext(&listing)),
                Some(Item::NeedsInput { prompt }) => Reply::input(prompt),
                Some(Item::Redirect { target }) => Reply::redirect(target),
                None => Reply::not_found("not found"),
            }
        }
    }
}

/// Answer gopher requests from `source`.
///
/// `host` and `port` are stamped into every menu item, because gopher item
/// lines carry their own rather than being relative to the request.
pub fn gopher<S: Source>(
    source: Arc<S>,
    host: impl Into<String>,
    port: u16,
) -> impl gopher_protocol::server::Handler {
    let host = host.into();
    move |request: gopher_protocol::server::Request| {
        let source = source.clone();
        let host = host.clone();
        async move {
            let found = source
                .get(&SourceRequest {
                    scheme: Scheme::Gopher,
                    path: normalize_selector(&request.selector),
                    query: request.search.clone(),
                })
                .await;

            match found {
                Some(Item::Document { body, .. }) => body,
                Some(Item::Listing(listing)) => to_gophermap(&listing, &host, port).into_bytes(),
                // Gopher has no status channel, so every other outcome is a
                // menu carrying an item that says what happened.
                Some(Item::NeedsInput { prompt }) => {
                    to_gophermap(&note(&prompt), &host, port).into_bytes()
                },
                Some(Item::Redirect { target }) => {
                    let mut listing = note("This has moved.");
                    listing.entries.push(super::Entry::new(
                        "Follow",
                        target,
                        super::EntryKind::Directory,
                    ));
                    to_gophermap(&listing, &host, port).into_bytes()
                },
                None => error_menu("not found", &host, port),
            }
        }
    }
}

/// Answer finger requests from `source`.
///
/// A finger query names a user, not a path, so the user becomes `/<user>` and
/// an empty query becomes `/`. Finger carries no MIME type and no status, so
/// everything arrives as text.
pub fn finger<S: Source>(source: Arc<S>) -> impl finger_protocol::server::Handler {
    move |request: finger_protocol::server::Request| {
        let source = source.clone();
        async move {
            let path = match &request.query.user {
                Some(user) => format!("/{user}"),
                None => "/".to_string(),
            };
            let found = source
                .get(&SourceRequest {
                    scheme: Scheme::Finger,
                    path,
                    query: None,
                })
                .await;

            match found {
                Some(Item::Document { body, .. }) => body,
                Some(Item::Listing(listing)) => to_plain_text(&listing).into_bytes(),
                Some(Item::NeedsInput { prompt }) => format!("{prompt}\n").into_bytes(),
                Some(Item::Redirect { target }) => {
                    format!("This has moved to {target}\n").into_bytes()
                },
                None => b"No such user.\n".to_vec(),
            }
        }
    }
}

/// A gopher selector as a source path. Gopher selectors are conventionally
/// written without a leading slash, so one is added to match every other
/// protocol's shape.
fn normalize_selector(selector: &str) -> String {
    if selector.is_empty() {
        "/".to_string()
    } else if selector.starts_with('/') {
        selector.to_string()
    } else {
        format!("/{selector}")
    }
}

/// A listing that is only a message.
fn note(text: &str) -> Listing {
    Listing {
        title: None,
        preamble: vec![text.to_string()],
        entries: Vec::new(),
    }
}

/// Gopher's way of saying no: a menu containing a type-`3` error item.
fn error_menu(reason: &str, host: &str, port: u16) -> Vec<u8> {
    format!("3{reason}\t\t{host}\t{port}\r\n.\r\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gopher_selector_becomes_a_rooted_path() {
        assert_eq!(normalize_selector(""), "/");
        assert_eq!(normalize_selector("/notes"), "/notes");
        assert_eq!(normalize_selector("notes"), "/notes");
    }

    #[test]
    fn gophers_refusal_is_an_error_item_inside_a_valid_menu() {
        let bytes = error_menu("not found", "example.test", 70);
        let text = String::from_utf8(bytes).unwrap();

        // It must still parse as a menu: a client that cannot read the refusal
        // has been handed garbage, not an error.
        let items = gopher_protocol::parse_menu(&text);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, gopher_protocol::GopherKind::Error);
        assert_eq!(items[0].display, "not found");
        assert!(text.ends_with(".\r\n"));
    }

    #[test]
    fn a_note_carries_its_text_as_info_lines() {
        let map = to_gophermap(&note("Say something."), "h", 70);
        let items = gopher_protocol::parse_menu(&map);
        assert_eq!(items[0].kind, gopher_protocol::GopherKind::Info);
        assert_eq!(items[0].display, "Say something.");
    }
}
