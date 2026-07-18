//! A spec-faithful implementation of the
//! [Spartan protocol](https://portal.mozz.us/spartan/spartan.mozz.us/)
//! (`spartan://`): an async client, a server with a pluggable handler, static
//! file serving, and a small CLI.
//!
//! Spartan is Michael Lazar's plaintext smolweb protocol (specification last
//! updated 2021-03-24, hosted at
//! [michael-lazar/spartan](https://github.com/michael-lazar/spartan)): an
//! ASCII request line `host SP path SP content-length CRLF` with an optional
//! upload data block, answered by a single-digit status line
//! (`2` success / `3` redirect / `4` client error / `5` server error) and a
//! body. The preferred document format is gemtext plus the `=:` prompt line
//! for input. The default port is 300.
//!
//! This crate is independent and unaffiliated with the protocol's author.
//! The crates.io name is qualified (`spartan-protocol`) because the bare
//! `spartan` name is used by an unrelated project.
//!
//! ```no_run
//! # async fn run() -> Result<(), spartan_protocol::ClientError> {
//! use spartan_protocol::{Status, fetch};
//!
//! let response = fetch("spartan://spartan.mozz.us/", &Default::default()).await?;
//! if response.status == Status::Success {
//!     println!("{} bytes of {}", response.body.len(), response.meta);
//! }
//! # Ok(()) }
//! ```

/// Spartan's default port (see the spec's Battle of Thermopylae aside).
pub const SPARTAN_PORT: u16 = 300;

mod client;
mod files;
mod server;

pub use client::{ClientError, FetchOptions, Response, fetch, submit};
pub use files::FileHandler;
pub use server::{Handler, Request, ServerConfig, SpartanResponse, serve};

/// A spartan response status: the single digit that leads the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// `2` — success; META is the body's MIME type.
    Success,
    /// `3` — redirect; META is an absolute path on the same host.
    Redirect,
    /// `4` — client error; META is a human-readable message.
    ClientError,
    /// `5` — server error; META is a human-readable message.
    ServerError,
}

impl Status {
    pub fn code(&self) -> u8 {
        match self {
            Self::Success => 2,
            Self::Redirect => 3,
            Self::ClientError => 4,
            Self::ServerError => 5,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            2 => Some(Self::Success),
            3 => Some(Self::Redirect),
            4 => Some(Self::ClientError),
            5 => Some(Self::ServerError),
            _ => None,
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Success => "success",
            Self::Redirect => "redirect",
            Self::ClientError => "client error",
            Self::ServerError => "server error",
        };
        write!(formatter, "{} ({name})", self.code())
    }
}

/// Parse a gemtext **prompt line** (spec §4.1): `=:` followed by a URL and an
/// optional user-friendly label. A prompt line is a link that should gather
/// user input to send as the request's data block.
pub fn parse_prompt_line(line: &str) -> Option<(&str, Option<&str>)> {
    let rest = line.strip_prefix("=:")?.trim_start();
    if rest.is_empty() {
        return None;
    }
    match rest.split_once(char::is_whitespace) {
        Some((target, label)) => {
            let label = label.trim();
            Some((target, (!label.is_empty()).then_some(label)))
        },
        None => Some((rest, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip() {
        for code in [2u8, 3, 4, 5] {
            assert_eq!(Status::from_code(code).unwrap().code(), code);
        }
        assert!(Status::from_code(6).is_none());
    }

    #[test]
    fn prompt_lines_parse_with_and_without_labels() {
        assert_eq!(
            parse_prompt_line("=: /guestbook/submit Sign the guestbook"),
            Some(("/guestbook/submit", Some("Sign the guestbook")))
        );
        assert_eq!(parse_prompt_line("=:/upload"), Some(("/upload", None)));
        assert_eq!(parse_prompt_line("=> /link"), None);
        assert_eq!(parse_prompt_line("=:"), None);
    }
}
