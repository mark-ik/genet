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

use url::Url;

use crate::cache::{HttpCache, NoHttpCache};
use crate::cookie_jar::InMemoryCookieJar;
use crate::cors::{InMemoryPreflightCache, PreflightCache};
use crate::hsts::{HstsStore, InMemoryHsts};

/// Caller-owned bundle of policy + storage the Fetch algorithm consults.
pub struct FetchContext {
    pub cookies: Box<dyn CookieStore>,
    pub cache: Box<dyn HttpCache>,
    pub csp: Box<dyn CspChecker>,
    pub hsts: Box<dyn HstsStore>,
    pub preflight: Box<dyn PreflightCache>,
    // request origin (for CORS) travels on the Request; redirect-cap override … later.
}

impl FetchContext {
    /// A dev/default context: in-memory cookie jar, no cache, permissive CSP,
    /// in-memory HSTS + preflight cache. Real deployments supply durable,
    /// host-backed impls (plan §4).
    pub fn permissive() -> Self {
        Self {
            cookies: Box::new(InMemoryCookieJar::default()),
            cache: Box::new(NoHttpCache),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(InMemoryHsts::new()),
            preflight: Box::new(InMemoryPreflightCache::new()),
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

/// RFC 6265bis cookie jar seam. In-memory default here; durable impls live in the
/// host (eidetic / persona-scoped storage).
pub trait CookieStore: Send + Sync {
    /// `Cookie` header value(s) to attach for a request to `url`, applying SameSite
    /// gating per `ctx`.
    fn cookies_for(&self, url: &Url, ctx: SameSiteContext) -> Vec<String>;
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
