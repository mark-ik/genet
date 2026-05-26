/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # netfetcher
//!
//! A portable **WHATWG-Fetch** network engine for the Mere ecosystem: Servo's
//! `net` made portable — the Fetch algorithm (CORS, cookie jar, HTTP cache,
//! redirects, HSTS, mixed-content, CSP hooks, content-encoding) lifted off
//! Servo's `ipc-channel` / resource-thread coupling and exposed as a
//! directly-callable async **library**, plus an HTTP/3 lane.
//!
//! **Layering:** Mere owns networking and drives netfetcher (off the UI thread,
//! in a `FetcherPool` worker); serval and other renderers stay byte-consuming
//! and never link this crate. The JS `fetch()` binding calls it *through the
//! host*, not by linking it. See the plan:
//! `mere/design_docs/mere_docs/implementation_strategy/2026-05-25_netfetcher_plan.md`.
//!
//! ## Status — increment 1 (2026-05-25)
//!
//! Real fetching works: h1/h2 GET/POST over hyper + rustls, redirect handling
//! (follow / error / manual), **streaming bodies** with on-the-fly
//! `Content-Encoding` decode (gzip/deflate/br/zstd), cookie attach/record, and
//! the CSP `connect-src` hook. Still ahead: HTTP cache + real RFC 6265bis cookie
//! matching (increment 2), CORS/tainting (increment 3), HSTS (increment 3), and
//! HTTP/3 (increment 4).

mod cache;
mod client;
mod context;
mod cookie_jar;
mod cors;
mod decode;
mod fetch;
mod hsts;
mod request;
mod response;

pub use cache::{HttpCache, InMemoryHttpCache, NoHttpCache, StoredResponse};
pub use context::{AllowAllCsp, CookieStore, CspChecker, FetchContext, SameSiteContext};
pub use cookie_jar::InMemoryCookieJar;
pub use fetch::fetch;
pub use hsts::{HstsStore, InMemoryHsts};
pub use request::{Credentials, Method, RedirectMode, Request, RequestMode};
pub use response::{Response, ResponseBody, ResponseType};
