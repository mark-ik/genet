/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`FetchContext`] and the pluggable policy/storage **seams**.
//!
//! The context is caller-owned and shareable across requests; it holds the state
//! the Fetch algorithm threads through. Storage is behind traits so Mere can back
//! the cookie jar / HTTP cache with persona- or session-scoped partitions, and
//! CSP stays a *hook* the embedder supplies (policy lives in the host, not here).
//!
//! **Resolved (increment 1, plan OQ#4):** the seams take `&self` and use interior
//! mutability, so a single shared `&FetchContext` can both *read* (attach cookies)
//! and *record* (store `Set-Cookie`) during a fetch — and they are `Send + Sync`
//! so the fetch future is movable across a multi-thread runtime.

use std::sync::Arc;

use cookie::SameSite;
use url::Url;

use crate::altsvc::{AltSvcStore, InMemoryAltSvc};
use crate::cache::{HttpCache, NoHttpCache};
use crate::cookie_jar::InMemoryCookieJar;
use crate::cors::{InMemoryPreflightCache, PreflightCache};
use crate::hsts::{HstsStore, InMemoryHsts};

/// Caller-owned bundle of policy + storage the Fetch algorithm consults.
pub struct FetchContext {
    pub cookies: Box<dyn CookieStore>,
    /// The HTTP cache. An `Arc` so one cache can be shared across many fetches
    /// (the `Default`/etc. modes are only meaningful against a persistent store).
    pub cache: Arc<dyn HttpCache>,
    pub csp: Box<dyn CspChecker>,
    pub hsts: Box<dyn HstsStore>,
    pub preflight: Box<dyn PreflightCache>,
    pub alt_svc: Box<dyn AltSvcStore>,
    // request origin (for CORS) travels on the Request; redirect-cap override … later.
}

impl FetchContext {
    /// A dev/default context: in-memory cookie jar, no cache, permissive CSP,
    /// in-memory HSTS / preflight / Alt-Svc stores. Real deployments supply
    /// durable, host-backed impls (plan §4).
    pub fn permissive() -> Self {
        Self {
            cookies: Box::new(InMemoryCookieJar::default()),
            cache: Arc::new(NoHttpCache),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(InMemoryHsts::new()),
            preflight: Box::new(InMemoryPreflightCache::new()),
            alt_svc: Box::new(InMemoryAltSvc::new()),
        }
    }
}

/// Whether a request is same-site with its target — drives SameSite cookie gating.
/// Computed by the fetch layer (which knows the initiator origin) and passed to
/// [`CookieStore::cookies_for`].
#[derive(Clone, Copy, Debug)]
pub struct SameSiteContext {
    /// The request's initiator site equals the target's site.
    pub same_site: bool,
    /// The request is a top-level navigation (lets `Lax` cookies through cross-site).
    pub top_level_navigation: bool,
}

impl SameSiteContext {
    /// A same-site request — the common case, and what a top-level fetch with no
    /// initiator is treated as.
    pub fn same_site() -> Self {
        Self {
            same_site: true,
            top_level_navigation: false,
        }
    }
}

/// A stored cookie's full record: the structured form behind [`CookieStore::cookies_for`]'s
/// header serialization. For consumers that need the attributes, not just the
/// `name=value` request-header pair — e.g. carrying a live session into another
/// engine (the verso flip), or persisting the jar durably.
#[derive(Clone, Debug, PartialEq)]
pub struct CookieRecord {
    pub name: String,
    pub value: String,
    /// The host for a host-only cookie, else the `Domain` value.
    pub domain: String,
    /// True when scoped to exactly `domain` (the `Set-Cookie` had no `Domain`).
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
    /// Absolute expiry in Unix seconds, or `None` for a session cookie.
    pub expires: Option<f64>,
}

/// RFC 6265bis cookie jar seam. In-memory default here; durable impls live in the
/// host (eidetic / persona-scoped storage).
pub trait CookieStore: Send + Sync {
    /// `Cookie` header value(s) to attach for a request to `url`, applying SameSite
    /// gating per `ctx`.
    fn cookies_for(&self, url: &Url, ctx: SameSiteContext) -> Vec<String>;
    /// The same cookies as [`cookies_for`](Self::cookies_for), but as structured
    /// [`CookieRecord`]s (attributes preserved). The default derives lossy records
    /// from `cookies_for` (name + value only, attributes inferred from the URL); a
    /// jar that holds the full record should override this for a lossless read.
    fn records_for(&self, url: &Url, ctx: SameSiteContext) -> Vec<CookieRecord> {
        let host = url.host_str().unwrap_or_default().to_string();
        let secure = url.scheme() == "https";
        self.cookies_for(url, ctx)
            .into_iter()
            .filter_map(|pair| {
                let (name, value) = pair.split_once('=')?;
                Some(CookieRecord {
                    name: name.to_string(),
                    value: value.to_string(),
                    domain: host.clone(),
                    host_only: true,
                    path: "/".to_string(),
                    secure,
                    http_only: false,
                    same_site: None,
                    expires: None,
                })
            })
            .collect()
    }
    /// Record a `Set-Cookie` header received from `url`.
    fn set_cookie(&self, url: &Url, set_cookie_header: &str);
}

/// CSP `connect-src` consultation hook. The embedder owns policy; netfetcher only
/// asks. Composes with Mere's capability gates.
pub trait CspChecker: Send + Sync {
    fn allows_connect(&self, url: &Url) -> bool;
}

/// Permissive CSP hook — allows every connection. The real policy is supplied by
/// the host; this is the dev default so a [`FetchContext`] is buildable today.
pub struct AllowAllCsp;

impl CspChecker for AllowAllCsp {
    fn allows_connect(&self, _url: &Url) -> bool {
        true
    }
}
