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
//! `mere/design_docs/archive_docs/2026-06-09_completed_plans/2026-05-25_netfetcher_plan.md`.
//!
//! ## Status — increments 1–5 (2026-05-26)
//!
//! - **1** h1/h2 GET/POST over hyper + rustls, redirects, streaming bodies with
//!   on-the-fly `Content-Encoding` decode.
//! - **2** RFC 6265bis cookie jar; RFC 9111 cache (freshness + revalidation).
//! - **3** cross-origin model: response tainting, CORS (simple + preflight +
//!   header filtering), HSTS, mixed-content auto-upgrade, SameSite; CSP hook.
//! - **4** HTTP/3 via Alt-Svc — a transport-abstracted h3 lane (quinn) with
//!   h1/h2 fallback.
//! - **5** WebSocket (`ws://` / `wss://`).
//!
//! Native-focused; the h3 and WebSocket lanes are native-only (wasm-excluded).
//! Deferred: h3 for requests with bodies, the active/passive mixed-content split,
//! and public-suffix-accurate same-site.

mod altsvc;
mod cache;
mod client;
mod context;
mod cookie_jar;
mod cors;
mod data_url;
mod decode;
mod fetch;
// HTTP/3 transport — native-only (QUIC over UDP); excluded from wasm builds.
#[cfg(not(target_arch = "wasm32"))]
mod h3_client;
mod hsts;
mod referrer;
mod request;
mod response;
mod sri;
// WebSocket — native-only (tokio + tungstenite); a wasm build binds browser WS.
#[cfg(not(target_arch = "wasm32"))]
mod websocket;

pub use altsvc::{AltSvcStore, InMemoryAltSvc};
pub use cache::{HttpCache, InMemoryHttpCache, NoHttpCache, StoredResponse};
pub use client::accept_invalid_certs;
pub use context::{AllowAllCsp, CookieStore, CspChecker, FetchContext, SameSiteContext};
pub use cors::{InMemoryPreflightCache, PreflightCache};
pub use cookie_jar::InMemoryCookieJar;
pub use fetch::fetch;
pub use hsts::{HstsStore, InMemoryHsts};
pub use request::{
    CacheMode, Credentials, Destination, Method, RedirectMode, ReferrerPolicy, Request, RequestMode,
};
pub use response::{Response, ResponseBody, ResponseType};
#[cfg(not(target_arch = "wasm32"))]
pub use websocket::{WebSocket, WsMessage, connect as connect_websocket};
