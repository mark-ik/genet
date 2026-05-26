/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The shared hyper client (connection pool + rustls TLS).
//!
//! One process-wide pool, lazily built — increment 1 doesn't yet support
//! per-[`crate::FetchContext`] TLS config (a later refinement); a global pool is
//! the right default for a v1 fetcher.

use std::sync::OnceLock;

use bytes::Bytes;
use http_body_util::Full;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

/// Request body type for the client: a fully-buffered `Bytes` (empty for GET).
/// Streaming request/response bodies are a later increment.
pub(crate) type ReqBody = Full<Bytes>;

/// The connection-pooled, TLS-capable client type.
pub(crate) type HttpClient = Client<HttpsConnector<HttpConnector>, ReqBody>;

static CLIENT: OnceLock<HttpClient> = OnceLock::new();

/// The process-wide client, built on first use.
pub(crate) fn shared_client() -> &'static HttpClient {
    CLIENT.get_or_init(build_client)
}

fn build_client() -> HttpClient {
    // rustls 0.23 needs a process-default CryptoProvider for the high-level
    // config builders hyper-rustls uses. Installing is idempotent-ish — it errors
    // if one is already installed, which we ignore.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http() // allow plaintext http:// too (local/dev, smolweb)
        .enable_http1()
        .enable_http2()
        .build();

    Client::builder(TokioExecutor::new()).build(https)
}
