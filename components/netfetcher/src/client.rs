/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The shared hyper client (connection pool + rustls TLS).
//!
//! One process-wide pool, lazily built — increment 1 doesn't yet support
//! per-[`crate::FetchContext`] TLS config (a later refinement); a global pool is
//! the right default for a v1 fetcher.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use bytes::Bytes;
use http_body_util::Full;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

/// Request body type for the client: a fully-buffered `Bytes` (empty for GET).
/// Streaming request/response bodies are a later increment.
pub(crate) type ReqBody = Full<Bytes>;

/// The connection-pooled, TLS-capable client type.
pub(crate) type HttpClient = Client<HttpsConnector<HttpConnector>, ReqBody>;

static CLIENT: OnceLock<HttpClient> = OnceLock::new();

/// Whether the shared client should trust any server certificate. Set before the
/// first fetch (the client is built once, lazily).
static ACCEPT_INVALID_CERTS: AtomicBool = AtomicBool::new(false);

/// Make the shared client trust any server certificate — for a local test
/// harness driving a self-signed server (e.g. WPT). Must be called before the
/// first request; production never calls it.
pub fn accept_invalid_certs() {
    ACCEPT_INVALID_CERTS.store(true, Ordering::Relaxed);
}

/// The process-wide client, built on first use.
pub(crate) fn shared_client() -> &'static HttpClient {
    CLIENT.get_or_init(build_client)
}

fn build_client() -> HttpClient {
    // rustls 0.23 needs a process-default CryptoProvider for the high-level
    // config builders hyper-rustls uses. Installing is idempotent-ish — it errors
    // if one is already installed, which we ignore.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let builder = hyper_rustls::HttpsConnectorBuilder::new();
    // Escape hatch for local test harnesses (e.g. WPT's self-signed server):
    // when [`accept_invalid_certs`] was called, trust any server certificate.
    // Off by default; production uses the webpki trust anchors.
    let connector = if ACCEPT_INVALID_CERTS.load(Ordering::Relaxed) {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        // ALPN is set by enable_http1()/enable_http2() below — don't pre-define it.
        builder.with_tls_config(config)
    } else {
        builder.with_webpki_roots()
    };

    let https = connector
        .https_or_http() // allow plaintext http:// too (local/dev, smolweb)
        .enable_http1()
        .enable_http2()
        .build();

    Client::builder(TokioExecutor::new()).build(https)
}

/// Accept any server certificate. Gated behind `NETFETCHER_ACCEPT_INVALID_CERTS`,
/// for test harnesses driving a self-signed local server only.
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer,
        _: &[CertificateDer],
        _: &ServerName,
        _: &[u8],
        _: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
