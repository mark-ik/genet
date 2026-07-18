//! # errand
//!
//! Async small-web ("smolweb") transport in one scheme-routed call. `errand`
//! fetches a URL over a small-web protocol and hands back the raw bytes, a
//! normalized [`Status`], and a MIME hint. It speaks **gemini** (TLS, TOFU),
//! **gopher**, **finger**, **spartan**, **nex**, **guppy**, and **titan**,
//! routed by scheme.
//!
//! Alongside the read schemes sit two **write companions**: [`titan_upload`]
//! (gemini's upload sibling) and [`misfin_send`] (gemini-style mail delivery,
//! with a caller-supplied client certificate). These are not fetchable schemes,
//! so they are direct calls, not part of [`fetch`].
//!
//! It does not speak HTTP, on purpose. HTTP is already well served (reqwest), and
//! a browser-extension host gets HTTP from the browser. `errand` fills the gap
//! those leave: the protocols of the small web, with no large dependency cone.
//!
//! ```no_run
//! # async fn run() -> Result<(), errand::Error> {
//! let page = errand::fetch("gemini://geminiprotocol.net/").await?;
//! if page.status == errand::Status::Success {
//!     println!("{} bytes of {}", page.body.len(), page.mime().unwrap_or("?"));
//! }
//! # Ok(()) }
//! ```

mod finger;
mod gemini;
mod gopher;
mod guppy;
mod misfin;
mod nex;
mod plain;
mod spartan;
mod titan;
mod tls;
mod tofu;

/// Smolweb document parsers: model-free, host-agnostic per-format parsers that turn
/// a protocol's bytes into a small AST. Separate from the transport above; a consumer
/// composes them (fetch, then parse) or parses a local file with no fetch at all. The
/// dep-free parsers (gemtext, gopher, nex, …) are always available; only the feed
/// (RSS/Atom) parser, which needs an XML reader, sits behind the `parse-feed` feature.
/// See [`parse`] for the per-format submodules.
pub mod parse;

pub use gemini::exchange as gemini_exchange;
pub use misfin::{ClientIdentity, MISFIN_PORT, send as misfin_send};
pub use titan::upload as titan_upload;
pub use tofu::{InMemoryTofu, PermissiveTofu, TofuStore, set_trust_store};
pub use url::Url;

/// A small-web scheme `errand` can route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// `gemini://`, TLS on port 1965.
    Gemini,
    /// `gopher://`, plaintext on port 70.
    Gopher,
    /// `finger://`, plaintext on port 79.
    Finger,
    /// `spartan://`, plaintext on port 300.
    Spartan,
    /// `nex://`, plaintext on port 1900.
    Nex,
    /// `guppy://`, UDP on port 6775.
    Guppy,
    /// `titan://`, TLS on port 1965 (upload companion to gemini).
    Titan,
}

impl Scheme {
    /// The scheme's conventional default port.
    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Gemini => 1965,
            Scheme::Gopher => 70,
            Scheme::Finger => 79,
            Scheme::Spartan => 300,
            Scheme::Nex => 1900,
            Scheme::Guppy => 6775,
            Scheme::Titan => 1965,
        }
    }

    /// Map a URL scheme string to a [`Scheme`], or `None` if it is not smolweb.
    pub fn parse(scheme: &str) -> Option<Scheme> {
        match scheme {
            "gemini" => Some(Scheme::Gemini),
            "gopher" => Some(Scheme::Gopher),
            "finger" => Some(Scheme::Finger),
            "spartan" => Some(Scheme::Spartan),
            "nex" => Some(Scheme::Nex),
            "guppy" => Some(Scheme::Guppy),
            "titan" => Some(Scheme::Titan),
            _ => None,
        }
    }
}

/// A normalized cross-protocol response status. The protocol's own numeric code,
/// where it has one (gemini, spartan, guppy, titan), is preserved in
/// [`Response::raw_status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The body is content of the type named in [`Response::meta`] (gemini 2x,
    /// spartan 2, guppy 2x, and every gopher/finger/nex reply).
    Success,
    /// The server wants input; `meta` is the prompt (gemini 1x, guppy 1x).
    Input,
    /// `meta` is a URL to follow (gemini 3x, spartan 3, guppy 3x). `errand` does
    /// not follow redirects itself; the caller decides.
    Redirect,
    /// The request failed; `meta` is a human-readable reason (gemini 4x/5x,
    /// spartan 4/5, guppy 4x/5x).
    Failure,
    /// A client certificate is required (gemini 6x).
    CertRequired,
}

/// One smolweb response: a normalized [`Status`], the protocol `meta` line (a
/// MIME type, prompt, redirect target, or reason depending on the status), and
/// the raw body bytes.
#[derive(Clone, Debug)]
pub struct Response {
    /// The URL that was fetched.
    pub url: Url,
    /// The normalized status.
    pub status: Status,
    /// The protocol's own two-digit code, for gemini, spartan, guppy, and titan.
    /// `None` for gopher, finger, and nex, which carry no status.
    pub raw_status: Option<u8>,
    /// The header's meta field: a MIME type on success, otherwise a prompt,
    /// redirect target, or reason. May be empty.
    pub meta: String,
    /// The response body. Empty for non-success statuses, where the payload is
    /// the `meta` line instead.
    pub body: Vec<u8>,
}

impl Response {
    /// The MIME type for a [`Status::Success`] response: `meta` up to the first
    /// `;` parameter, trimmed. `None` for non-success statuses or empty meta.
    pub fn mime(&self) -> Option<&str> {
        if self.status != Status::Success {
            return None;
        }
        let mime = self.meta.split(';').next().unwrap_or("").trim();
        (!mime.is_empty()).then_some(mime)
    }
}

/// A transport error.
#[derive(Clone, Debug)]
pub enum Error {
    /// The URL's scheme is not a smolweb scheme `errand` routes.
    UnsupportedScheme(String),
    /// The URL could not be parsed, or it lacks a host.
    BadUrl(String),
    /// The TCP/TLS/UDP connection could not be established.
    Connect(String),
    /// A read or write failed mid-exchange.
    Io(String),
    /// The response violated the protocol grammar (e.g. a malformed header).
    Protocol(String),
    /// The request did not complete within the caller-supplied timeout.
    Timeout,
    /// A gemini host's certificate no longer matches its pinned fingerprint —
    /// a man-in-the-middle, a server key rotation, or a moved host. The
    /// request was *not* sent. Never silently re-pinned: the embedder decides
    /// (warn, then re-pin via the [`TofuStore`] if the change is legitimate).
    CertificateChanged {
        host: String,
        /// The previously pinned fingerprint, lowercase hex.
        pinned: String,
        /// The fingerprint just presented, lowercase hex.
        seen: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnsupportedScheme(s) => write!(f, "unsupported scheme: {s}"),
            Error::BadUrl(s) => write!(f, "bad URL: {s}"),
            Error::Connect(s) => write!(f, "connect: {s}"),
            Error::Io(s) => write!(f, "io: {s}"),
            Error::Protocol(s) => write!(f, "protocol: {s}"),
            Error::Timeout => write!(f, "fetch timed out"),
            Error::CertificateChanged { host, pinned, seen } => write!(
                f,
                "certificate for {host} changed (possible MITM); pinned {pinned}, saw {seen}"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Fetch `url` over its smolweb scheme. Convenience wrapper over [`fetch_url`]
/// that parses the string first.
pub async fn fetch(url: &str) -> Result<Response, Error> {
    let parsed = Url::parse(url).map_err(|e| Error::BadUrl(e.to_string()))?;
    fetch_url(&parsed).await
}

/// Fetch an already-parsed `url`, routing by scheme. The body is read in full
/// (smolweb servers close the connection when the response ends).
///
/// This is the single load-bearing transport entry: every public fetch helper
/// (`fetch`, `fetch_timeout`, `fetch_url_timeout`) routes through it. It emits one
/// structured `tracing` event per call on target `"errand"` — DEBUG on completion
/// (scheme, status, raw_status, byte_len, elapsed_ms) and WARN on failure (scheme,
/// error, elapsed_ms). The facade only emits; the consuming app installs the
/// subscriber and chooses the level.
pub async fn fetch_url(url: &Url) -> Result<Response, Error> {
    let started = std::time::Instant::now();
    let result = fetch_url_inner(url).await;
    let elapsed_ms = started.elapsed().as_millis();
    let scheme = url.scheme();
    match &result {
        Ok(response) => {
            tracing::debug!(
                target: "errand",
                url = %url,
                scheme,
                status = ?response.status,
                raw_status = ?response.raw_status,
                byte_len = response.body.len(),
                elapsed_ms,
                "smolweb fetch complete"
            );
        },
        Err(error) => {
            tracing::warn!(
                target: "errand",
                url = %url,
                scheme,
                error = %error,
                elapsed_ms,
                "smolweb fetch failed"
            );
        },
    }
    result
}

/// Scheme routing for [`fetch_url`], split out so the public entry can time and
/// trace the whole exchange in one place.
async fn fetch_url_inner(url: &Url) -> Result<Response, Error> {
    match Scheme::parse(url.scheme()) {
        Some(Scheme::Gemini) => gemini::fetch(url).await,
        Some(Scheme::Gopher) => gopher::fetch(url).await,
        Some(Scheme::Finger) => finger::fetch(url).await,
        Some(Scheme::Spartan) => spartan::fetch(url).await,
        Some(Scheme::Nex) => nex::fetch(url).await,
        Some(Scheme::Guppy) => guppy::fetch(url).await,
        Some(Scheme::Titan) => titan::fetch(url).await,
        None => Err(Error::UnsupportedScheme(url.scheme().to_string())),
    }
}

/// Fetch `url` with a per-request timeout. Returns [`Error::Timeout`] if the
/// fetch does not complete within `timeout`.
pub async fn fetch_timeout(url: &str, timeout: std::time::Duration) -> Result<Response, Error> {
    let parsed = Url::parse(url).map_err(|e| Error::BadUrl(e.to_string()))?;
    fetch_url_timeout(&parsed, timeout).await
}

/// Fetch an already-parsed `url` with a per-request timeout. Returns
/// [`Error::Timeout`] if the fetch does not complete within `timeout`.
pub async fn fetch_url_timeout(url: &Url, timeout: std::time::Duration) -> Result<Response, Error> {
    tokio::time::timeout(timeout, fetch_url(url))
        .await
        .map_err(|_| Error::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_round_trips_and_ports() {
        assert_eq!(Scheme::parse("gemini"), Some(Scheme::Gemini));
        assert_eq!(
            Scheme::parse("spartan").map(Scheme::default_port),
            Some(300)
        );
        assert_eq!(Scheme::parse("https"), None);
    }

    #[test]
    fn new_schemes_round_trip() {
        assert_eq!(Scheme::parse("nex").map(Scheme::default_port), Some(1900));
        assert_eq!(Scheme::parse("guppy").map(Scheme::default_port), Some(6775));
        assert_eq!(Scheme::parse("titan").map(Scheme::default_port), Some(1965));
    }

    #[test]
    fn mime_strips_params_and_gates_on_success() {
        let mut r = Response {
            url: Url::parse("gemini://x/").unwrap(),
            status: Status::Success,
            raw_status: Some(20),
            meta: "text/gemini; charset=utf-8".into(),
            body: Vec::new(),
        };
        assert_eq!(r.mime(), Some("text/gemini"));
        r.status = Status::Failure;
        assert_eq!(r.mime(), None, "non-success has no mime");
    }

    #[tokio::test]
    async fn http_is_not_routed() {
        let err = fetch("https://example.com/").await.unwrap_err();
        assert!(matches!(err, Error::UnsupportedScheme(s) if s == "https"));
    }

    #[test]
    fn every_smolweb_scheme_is_recognized() {
        for s in [
            "gemini", "gopher", "finger", "spartan", "nex", "guppy", "titan",
        ] {
            assert!(Scheme::parse(s).is_some(), "{s} should route");
        }
    }

    #[test]
    fn timeout_error_displays() {
        assert_eq!(Error::Timeout.to_string(), "fetch timed out");
    }
}
