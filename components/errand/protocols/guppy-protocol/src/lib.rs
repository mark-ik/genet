//! An implementation of the
//! [Guppy protocol](https://github.com/dimkr/guppy-protocol) v0.4.4
//! (`guppy://`, UDP port 6775): an async client and server with chunking,
//! per-packet acknowledgement, and retransmission, plus a small CLI.
//!
//! Guppy is dimkr's smolweb-over-UDP protocol, inspired by TFTP, DNS, and
//! Spartan. A request is a single datagram carrying a URL (user input rides
//! the query component, percent-encoded). The server answers with either a
//! special single-digit packet — `1 <prompt>` / `3 <url>` redirect /
//! `4 <error>` — or a **success** packet `<seq> <mimetype>\r\n<data>` whose
//! sequence number starts at a random value in `[6, 2^31-1]`, followed by
//! continuation packets `<seq>\r\n<data>` (seq incrementing by one), ended by
//! an empty continuation (the end-of-file packet). The client acknowledges
//! every success/continuation/EOF packet by echoing its sequence number.
//! Lost packets are handled by retransmission on both sides.
//!
//! This crate is independent and unaffiliated with the protocol's author.
//! The crates.io name is qualified (`guppy-protocol`) because the bare
//! `guppy` name is used by an unrelated project.
//!
//! ```no_run
//! # async fn run() -> Result<(), guppy_protocol::ClientError> {
//! use guppy_protocol::{GuppyResponse, fetch};
//!
//! match fetch("guppy://guppy.mozz.us/", &Default::default()).await? {
//!     GuppyResponse::Success { mime, body } => {
//!         println!("{} bytes of {mime}", body.len());
//!     }
//!     other => println!("{other:?}"),
//! }
//! # Ok(()) }
//! ```

/// Guppy's default port ('gu').
pub const GUPPY_PORT: u16 = 6775;

/// The spec's request-size ceiling: the URL plus the trailing CRLF must fit
/// in 2048 bytes.
pub const MAX_REQUEST_BYTES: usize = 2048;

/// The largest sequence number (maximum value of a signed 32-bit integer).
pub const MAX_SEQ: u32 = 2_147_483_647;

/// The smallest first-packet sequence number.
pub const MIN_SEQ: u32 = 6;

mod client;
mod packet;
mod server;

pub use client::{ClientError, FetchOptions, fetch};
pub use packet::{Packet, parse_packet};
pub use server::{FileHandler, Handler, Request, ServerConfig, serve};

/// A complete guppy response, as returned by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuppyResponse {
    /// A reassembled successful response.
    Success { mime: String, body: Vec<u8> },
    /// `1 <prompt>` — repeat the request with user input in the URL query.
    Prompt { text: String },
    /// `3 <url>` — re-request at this (possibly relative) URL.
    Redirect { target: String },
    /// `4 <error>` — a human-readable error for the user.
    Error { message: String },
}
