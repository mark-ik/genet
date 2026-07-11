//! An implementation of the [Nex protocol](nex://nightfall.city/nex/info/specification.txt)
//! (`nex://`, TCP port 1900): an async client, a directory-serving server, a
//! listing parser, and a small CLI.
//!
//! Nex is the minimal smolweb protocol, from nightfall.city, inspired by
//! gopher and gemini. The whole wire format: the client connects and sends a
//! path (which may be empty); the server responds with text or binary data
//! and closes the connection. No TLS, no status codes, no headers, no state.
//! Directory content is plain text where each line beginning `=> ` followed
//! by a URL is a link; an empty path or a path ending in `/` is a directory;
//! a document's display type follows its file extension, defaulting to plain
//! text.
//!
//! This crate is independent and unaffiliated with the protocol's author.
//! The crates.io name is qualified (`nex-protocol`) because the bare `nex`
//! name is used by an unrelated project.
//!
//! ```no_run
//! # async fn run() -> Result<(), nex_protocol::ClientError> {
//! let body = nex_protocol::fetch("nex://nightfall.city/", &Default::default()).await?;
//! println!("{}", String::from_utf8_lossy(&body));
//! # Ok(()) }
//! ```

/// Nex's port ("Afterall, night falls at 7pm!").
pub const NEX_PORT: u16 = 1900;

mod client;
mod listing;
mod server;

pub use client::{ClientError, FetchOptions, fetch, fetch_path};
pub use listing::{ListingLine, parse_listing};
pub use server::{FileHandler, Handler, Request, ServerConfig, serve};

/// Whether a request path names a directory, per the spec: an empty path or
/// one ending in `/`.
pub fn is_directory_path(path: &str) -> bool {
    path.is_empty() || path.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_paths_per_spec() {
        assert!(is_directory_path(""));
        assert!(is_directory_path("/"));
        assert!(is_directory_path("/nexlog/"));
        assert!(!is_directory_path("/about.txt"));
    }
}
