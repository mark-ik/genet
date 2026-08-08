//! Serving: errand's other direction.
//!
//! errand's fetch side takes N protocols and **normalizes** them into one
//! [`Response`](crate::Response). This side takes one [`Source`] and
//! **denormalizes** it into N protocol shapes. Same routing table, same
//! vocabulary, opposite direction.
//!
//! ## Why the source is a trait and not a directory
//!
//! A directory of files is one source. A projection of a community's shared
//! documents is another, and it is the one this exists for. Taking a path
//! would mean bolting the second case on afterwards, which is the harder
//! order.
//!
//! ## Why a listing is structural, not bytes
//!
//! The composition worth having is that **one source is projected into each
//! protocol's native format**: a gopher client is answered with a gophermap
//! and a gemini client with gemtext link lines, from the same [`Listing`].
//! If a source returned pre-rendered bytes it would have to pick a format,
//! and every protocol but that one would be served a translation.
//!
//! This is the publishing half of the rule that a format we do not own is
//! projected into faithfully and never extended.

pub mod adapt;
pub mod dir;
pub mod project;

pub use dir::Directory;

use std::future::Future;

use crate::Scheme;

/// One request against a source, in protocol-neutral terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRequest {
    /// Which protocol asked. A source may vary its answer by scheme, but it
    /// does not have to, and most should not.
    pub scheme: Scheme,
    /// The path, with a leading `/`.
    pub path: String,
    /// Search terms, from a gopher type-7 request or a gemini query string.
    pub query: Option<String>,
}

/// What kind of thing an entry points at.
///
/// Deliberately coarse: it carries what every protocol in the family can
/// express, which is the difference between "another listing" and "a
/// document", plus a few media hints gopher item types distinguish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Another listing.
    Directory,
    /// A text document.
    Text,
    /// A search endpoint, which a client should prompt for terms before
    /// following. Gopher's type 7, gemini's `1x`.
    Search,
    /// An image.
    Image,
    /// A sound file.
    Sound,
    /// Anything else: opaque bytes.
    Binary,
}

/// One entry in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What a reader sees.
    pub display: String,
    /// Where it lives, as a path on this host, or an absolute URL for
    /// something elsewhere.
    pub target: String,
    pub kind: EntryKind,
}

impl Entry {
    pub fn new(display: impl Into<String>, target: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            display: display.into(),
            target: target.into(),
            kind,
        }
    }

    /// Whether the target is already absolute, and so must not be joined to
    /// this host when projected.
    pub fn is_absolute(&self) -> bool {
        self.target.contains("://")
    }
}

/// A listing: entries, plus optional prose to introduce them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// A heading for the listing, if it has one.
    pub title: Option<String>,
    /// Lines of prose shown before the entries. Gopher renders these as `i`
    /// info lines; gemini as ordinary text.
    pub preamble: Vec<String>,
    pub entries: Vec<Entry>,
}

/// What a source can answer with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A document, served as-is with the MIME type the source declares.
    Document { mime: String, body: Vec<u8> },
    /// A listing, projected natively per protocol.
    Listing(Listing),
    /// The source wants input before it can answer. Gemini asks with `1x`;
    /// gopher's type-7 search is the same idea arriving differently.
    NeedsInput { prompt: String },
    /// Deliberately elsewhere.
    Redirect { target: String },
}

impl Item {
    /// A `text/gemini` document.
    pub fn gemtext(body: impl Into<Vec<u8>>) -> Self {
        Self::Document {
            mime: "text/gemini".into(),
            body: body.into(),
        }
    }

    /// A `text/plain` document.
    pub fn text(body: impl Into<String>) -> Self {
        Self::Document {
            mime: "text/plain".into(),
            body: body.into().into_bytes(),
        }
    }
}

/// Where served content comes from.
///
/// One implementation is a directory of files. The one this exists for is a
/// projection of a moot's authority-filtered contents, which is why the trait
/// says nothing about filesystems.
pub trait Source: Send + Sync + 'static {
    /// Resolve a request. `None` is "no such thing here", which each protocol
    /// then expresses in its own way: gemini has a status for it, gopher has
    /// only an error item inside a menu.
    fn get(&self, request: &SourceRequest) -> impl Future<Output = Option<Item>> + Send;
}

impl<F, Fut> Source for F
where
    F: Fn(&SourceRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<Item>> + Send,
{
    fn get(&self, request: &SourceRequest) -> impl Future<Output = Option<Item>> + Send {
        self(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_target_is_recognised() {
        assert!(Entry::new("Elsewhere", "gemini://other.test/", EntryKind::Text).is_absolute());
        assert!(!Entry::new("Here", "/page.gmi", EntryKind::Text).is_absolute());
    }

    #[tokio::test]
    async fn a_closure_is_a_source() {
        let source = |request: &SourceRequest| {
            let path = request.path.clone();
            async move { (path == "/hi").then(|| Item::text("hello")) }
        };
        let found = source
            .get(&SourceRequest {
                scheme: Scheme::Gemini,
                path: "/hi".into(),
                query: None,
            })
            .await;
        assert_eq!(found, Some(Item::text("hello")));

        let missing = source
            .get(&SourceRequest {
                scheme: Scheme::Gemini,
                path: "/nope".into(),
                query: None,
            })
            .await;
        assert_eq!(missing, None);
    }

    /// The whole chain, on a real socket: a directory of files becomes a
    /// `Source`, the adapter binds it to gopher's server, and gopher's own
    /// client fetches it back. Nothing here is mocked.
    #[tokio::test]
    async fn a_directory_is_served_and_fetched_over_gopher() {
        use std::sync::Arc;

        // A tiny capsule on disk.
        let root = std::env::temp_dir().join(format!("errand-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(
            root.join("hello.txt"),
            b"well met
",
        )
        .unwrap();
        std::fs::write(
            root.join("notes").join("a.gmi"),
            b"# A
",
        )
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handler = adapt::gopher(Arc::new(Directory::new(&root)), "127.0.0.1", port);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let _ = gopher_protocol::serve(
                listener,
                handler,
                gopher_protocol::ServerConfig::default(),
                async {
                    let _ = stopped.await;
                },
            )
            .await;
        });

        // A file comes back verbatim.
        let page = gopher_protocol::fetch(&format!("gopher://127.0.0.1:{port}/0hello.txt"))
            .await
            .unwrap();
        assert_eq!(
            page.body,
            b"well met
"
        );

        // The root is a directory, so it comes back as a real gophermap that
        // gopher's own parser accepts.
        let menu = gopher_protocol::fetch(&format!("gopher://127.0.0.1:{port}/1"))
            .await
            .unwrap();
        let items = gopher_protocol::parse_menu(&String::from_utf8_lossy(&menu.body));
        let displays: Vec<_> = items.iter().map(|i| i.display.as_str()).collect();
        assert!(displays.contains(&"notes/"), "got {displays:?}");
        assert!(displays.contains(&"hello.txt"), "got {displays:?}");

        // Directories sort before files.
        let first_resource = items.iter().find(|i| i.url.is_some()).unwrap();
        assert_eq!(first_resource.display, "notes/");

        // A missing path is gopher's error item, not a truncated reply.
        let missing = gopher_protocol::fetch(&format!("gopher://127.0.0.1:{port}/0nope.txt"))
            .await
            .unwrap();
        let items = gopher_protocol::parse_menu(&String::from_utf8_lossy(&missing.body));
        assert_eq!(items[0].kind, gopher_protocol::GopherKind::Error);

        // And traversal is refused over the wire, not just in the unit test.
        let escaped =
            gopher_protocol::fetch(&format!("gopher://127.0.0.1:{port}/0../../etc/hosts"))
                .await
                .unwrap();
        let items = gopher_protocol::parse_menu(&String::from_utf8_lossy(&escaped.body));
        assert_eq!(items[0].kind, gopher_protocol::GopherKind::Error);

        let _ = stop.send(());
        let _ = server.await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
