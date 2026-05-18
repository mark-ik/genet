/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Request/response state for the active session — redirects, MIME,
//! TLS summary, cache origin, errors.
//!
//! Cf. Hekate doc §"Loading/Network Plane". Lanes (or their protocol
//! adapters) emit loading events; Hekate records the normalized
//! snapshot per session; the host displays status / error / progress.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

/// Common-minimum loading-state queries. Implemented by Hekate's
/// per-session snapshot — exposed to the host for chrome (URL bar,
/// security indicator, loading spinner) and to Apparatus for
/// debugging.
pub trait LoadingQuery {
    fn state(&self) -> LoadingState;
    fn progress(&self) -> Option<LoadProgress>;
    /// Final URL after redirects, if a load has completed enough to
    /// identify a final source.
    fn final_url(&self) -> Option<&str>;
    /// Redirect chain (in order). Empty if no redirects occurred.
    fn redirect_chain(&self) -> &[String];
    /// MIME / Content-Type of the response body.
    fn mime(&self) -> Option<&str>;
    /// TLS handshake summary, if the load used HTTPS / TLS-wrapped
    /// protocol.
    fn tls_summary(&self) -> Option<&TlsSummary>;
    /// Whether the response came from cache, missed, or isn't
    /// cacheable.
    fn cache_origin(&self) -> CacheOrigin;
    /// Protocol/network/certificate error, if the load failed.
    fn error(&self) -> Option<&LoadError>;
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum LoadingState {
    /// No load in progress yet; session created but request not sent.
    #[default]
    Pending,
    /// Request sent; response headers may or may not have arrived.
    InProgress,
    /// Response fully received (or fully rendered if the lane does
    /// stream-as-render).
    Done,
    /// Load terminated by error before completion.
    Failed,
}

/// Progress signal: bytes received vs (optionally) total bytes.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct LoadProgress {
    pub bytes_received: u64,
    /// Total bytes if Content-Length / equivalent is known, else
    /// `None` (e.g., chunked encoding without a total).
    pub bytes_total: Option<u64>,
}

/// Minimal TLS handshake summary. Lane-specific protocols (Gemini,
/// Tor onion) may extend this in their own observables.
#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct TlsSummary {
    pub protocol: String, // e.g., "TLS 1.3"
    pub cipher_suite: String,
    /// Whether the cert chain validated against the trust store.
    pub validated: bool,
    /// Hostname certificate is valid for (the leaf cert's CN/SAN).
    pub host: String,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum CacheOrigin {
    /// Response came from cache (HTTP cache, lane-specific cache).
    CacheHit,
    /// Cache had no usable entry; went to network.
    #[default]
    CacheMiss,
    /// Response carried `Cache-Control: no-store` or equivalent.
    NotCacheable,
}

/// Categorized load error. Concrete shape per error kind kept
/// intentionally narrow — consumers usually only need the kind +
/// summary for display, not deep structured access.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct LoadError {
    pub kind: LoadErrorKind,
    /// Human-readable summary. Lane-specific.
    pub message: String,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum LoadErrorKind {
    /// Couldn't reach the host (DNS, connection refused, timeout).
    Network,
    /// HTTPS / TLS handshake failed (bad cert, hostname mismatch,
    /// protocol downgrade).
    TlsHandshake,
    /// HTTP status >= 400 or protocol-specific equivalent.
    ServerError,
    /// Response body malformed for the declared MIME / protocol.
    Decoding,
    /// Other / unclassified.
    #[default]
    Other,
}
