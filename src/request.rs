/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Fetch-spec [`Request`] — mode, credentials, redirect policy, body.
//!
//! Distinct from the *wire-level* `http::Request`: this carries the
//! Fetch-algorithm concepts (request mode, credentials mode, response tainting
//! inputs). Whether to thin-wrap the `http` crate instead is **plan open
//! question #1** — deferred; for now netfetcher owns its types.

use bytes::Bytes;
use url::{Origin, Url};

/// A Fetch request.
#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    pub method: Method,
    /// Header list. Placeholder shape — becomes an ordered header map once the
    /// `http`-crate-vs-own-types question (plan OQ#1) is settled.
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
    pub mode: RequestMode,
    pub credentials: Credentials,
    pub redirect: RedirectMode,
    /// The initiator's origin (the page/script that issued the fetch). Drives CORS
    /// gating and response tainting. `None` = no initiator (a top-level fetch),
    /// treated as same-origin with the target — no cross-origin checks apply.
    pub origin: Option<Origin>,
    /// What the response will be used for (WHATWG Fetch request *destination*).
    /// Drives the mixed-content active/passive split: optionally-blockable
    /// destinations (image/audio/video) are auto-upgraded http→https, the rest
    /// are blocked in a secure context.
    pub destination: Destination,
    /// The HTTP cache mode (WHATWG Fetch request *cache mode*): which cached
    /// response, if any, this request may use, and which request headers it adds.
    pub cache: CacheMode,
    /// The request's referrer URL (the initiator document), or `None` for no
    /// referrer. The `Referer` header is derived from this per [`referrer_policy`].
    pub referrer: Option<Url>,
    /// Referrer policy governing the `Referer` header (a redirect's
    /// `Referrer-Policy` response header can override it mid-chain).
    pub referrer_policy: ReferrerPolicy,
}

impl Request {
    /// A plain `GET` with default modes — the increment-1 starting point.
    pub fn get(url: Url) -> Self {
        Self {
            url,
            method: Method::Get,
            headers: Vec::new(),
            body: None,
            mode: RequestMode::default(),
            credentials: Credentials::default(),
            redirect: RedirectMode::default(),
            origin: None,
            destination: Destination::default(),
            cache: CacheMode::default(),
            referrer: None,
            referrer_policy: ReferrerPolicy::default(),
        }
    }

    /// Set the request destination (drives the mixed-content active/passive split).
    pub fn with_destination(mut self, destination: Destination) -> Self {
        self.destination = destination;
        self
    }

    /// Set the initiator origin (cross-origin requests need this for CORS).
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = Some(origin);
        self
    }
}

/// HTTP cache mode (WHATWG Fetch §2.2.5 request cache mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CacheMode {
    /// RFC 9111: serve a fresh stored response, revalidate a stale one.
    #[default]
    Default,
    /// Never read or write the cache; force a network fetch.
    NoStore,
    /// Bypass the stored response (always network) but store the result.
    Reload,
    /// Always revalidate a stored response before using it.
    NoCache,
    /// Use a stored response even if stale; only network on a miss.
    ForceCache,
    /// Use a stored response even if stale; a miss is a network error.
    OnlyIfCached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
}

/// Fetch request mode (WHATWG Fetch §2.2.5) — gates CORS and response tainting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
    Navigate,
}

/// Credentials mode — whether cookies/auth travel with the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Credentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}

/// Referrer policy (W3C Referrer Policy) — governs the `Referer` header value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReferrerPolicy {
    /// No policy set: apply the default (`strict-origin-when-cross-origin`).
    #[default]
    Empty,
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

/// WHATWG Fetch request *destination* — what the fetched bytes become. Only the
/// distinctions the mixed-content split needs are modeled; every other
/// destination (script, style, font, worker, the empty fetch()/XHR destination,
/// …) is "blockable" and collapses to [`Destination::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Destination {
    /// The empty destination (a plain `fetch()` / XHR) — blockable.
    #[default]
    None,
    /// `image` — optionally-blockable (auto-upgraded).
    Image,
    /// `audio` — optionally-blockable.
    Audio,
    /// `video` — optionally-blockable.
    Video,
    /// `document` (a nested navigable) — blockable.
    Document,
    /// Any other destination (script, style, font, worker, …) — blockable.
    Other,
}

impl Destination {
    /// "optionally-blockable" per the Mixed Content spec: `image` / `audio` /
    /// `video`. These are auto-upgraded http→https; everything else is blockable.
    pub(crate) fn is_optionally_blockable(self) -> bool {
        matches!(self, Destination::Image | Destination::Audio | Destination::Video)
    }
}

/// Redirect handling (WHATWG Fetch §2.2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}
