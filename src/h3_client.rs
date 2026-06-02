/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HTTP/3 transport over QUIC (quinn + h3).
//!
//! A *separate* transport from the hyper h1/h2 client: it establishes a QUIC
//! connection, drives the h3 connection concurrently with the request, and
//! collects the response. Native-only (UDP sockets) — the module is excluded from
//! wasm builds.
//!
//! **Increment 4a:** the transport + an offline round-trip test (in-process quinn
//! h3 server). It is *not yet wired into the fetch loop* — Alt-Svc discovery and
//! routing land in 4b, which is when [`fetch_h3`] gets a caller.

use bytes::{Buf, Bytes, BytesMut};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use url::Url;

/// An HTTP/3 response: status, headers, and the fully-collected body.
#[allow(dead_code)] // wired into the fetch loop in 4b (Alt-Svc routing)
pub(crate) struct H3Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

/// Perform an HTTP/3 request over QUIC. `client_config` carries the QUIC/TLS
/// config (ALPN `h3`, trust roots). Returns `None` on any transport or protocol
/// failure — the caller treats that as "fall back to h1/h2" (4b).
#[allow(dead_code)] // wired into the fetch loop in 4b (Alt-Svc routing)
pub(crate) async fn fetch_h3(
    client_config: quinn::ClientConfig,
    url: &Url,
    method: http::Method,
    headers: &[(String, String)],
    body: Option<Bytes>,
) -> Option<H3Response> {
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    let addr = (host, port).to_socket_addrs().ok()?.next()?;

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().ok()?).ok()?;
    endpoint.set_default_client_config(client_config);

    let conn = endpoint.connect(addr, host).ok()?.await.ok()?;
    let (mut driver, mut send_request) =
        h3::client::new(h3_quinn::Connection::new(conn)).await.ok()?;

    let uri: http::Uri = url.as_str().parse().ok()?;
    let mut builder = http::Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let req = builder.body(()).ok()?;

    let request = async {
        let mut stream = send_request.send_request(req).await.ok()?;
        if let Some(body) = body {
            if !body.is_empty() {
                stream.send_data(body).await.ok()?;
            }
        }
        stream.finish().await.ok()?;
        let resp = stream.recv_response().await.ok()?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_owned(), s.to_owned())))
            .collect();
        let mut body = BytesMut::new();
        loop {
            match stream.recv_data().await {
                Ok(Some(mut chunk)) => body.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining())),
                Ok(None) => break,
                Err(_) => return None,
            }
        }
        Some(H3Response {
            status,
            headers,
            body: body.freeze(),
        })
    };

    // The connection driver must be polled for the request to make progress; run
    // both and take whichever finishes — a request result, or a driver close
    // (connection died first → treated as a network failure).
    tokio::select! {
        result = request => result,
        _ = std::future::poll_fn(|cx| driver.poll_close(cx)) => None,
    }
}

/// Production QUIC client config: webpki trust roots + ALPN `h3`.
#[allow(dead_code)] // used by fetch_h3_default; wired into the fetch loop in 4b routing
fn webpki_quic_config() -> Option<quinn::ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    Some(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).ok()?,
    )))
}

/// [`fetch_h3`] using the production (webpki) QUIC config.
#[allow(dead_code)] // wired into the fetch loop in 4b routing
pub(crate) async fn fetch_h3_default(
    url: &Url,
    method: http::Method,
    headers: &[(String, String)],
    body: Option<Bytes>,
) -> Option<H3Response> {
    fetch_h3(webpki_quic_config()?, url, method, headers, body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
    use std::sync::Arc;

    fn install_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    /// Test-only: accept any server certificate.
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

    fn no_verify_client_config() -> quinn::ClientConfig {
        let mut tls = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).unwrap()))
    }

    /// Start an in-process h3 server (127.0.0.1, ephemeral port) that answers every
    /// request with `200` + `body`. Returns the bound port.
    async fn start_server(body: &'static [u8]) -> u16 {
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
        let cert_der = CertificateDer::from(cert.cert.der().to_vec());
        let key_der = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());

        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der.into())
            .unwrap();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        let server_config =
            quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls).unwrap()));

        let endpoint =
            quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let port = endpoint.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Ok(conn) = incoming.await else { continue };
                let Ok(mut h3_conn) =
                    h3::server::Connection::new(h3_quinn::Connection::new(conn)).await
                else {
                    continue;
                };
                while let Ok(Some(resolver)) = h3_conn.accept().await {
                    let Ok((_req, mut stream)) = resolver.resolve_request().await else {
                        break;
                    };
                    // Drain any request body; echo it back when present, else the
                    // fixed `body`. This lets a POST test verify the h3 body lane.
                    let mut req_body = BytesMut::new();
                    while let Ok(Some(mut chunk)) = stream.recv_data().await {
                        req_body.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining()));
                    }
                    let out = if req_body.is_empty() {
                        Bytes::from_static(body)
                    } else {
                        req_body.freeze()
                    };
                    let resp = http::Response::builder().status(200).body(()).unwrap();
                    let _ = stream.send_response(resp).await;
                    let _ = stream.send_data(out).await;
                    let _ = stream.finish().await;
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn h3_round_trip() {
        install_provider();
        let port = start_server(b"hello over h3").await;
        let url: Url = format!("https://127.0.0.1:{port}/").parse().unwrap();

        let resp = fetch_h3(no_verify_client_config(), &url, http::Method::GET, &[], None)
            .await
            .expect("h3 round-trip succeeds");

        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_ref(), b"hello over h3");
    }

    #[tokio::test]
    async fn h3_round_trip_with_request_body() {
        install_provider();
        let port = start_server(b"unused").await;
        let url: Url = format!("https://127.0.0.1:{port}/").parse().unwrap();

        let resp = fetch_h3(
            no_verify_client_config(),
            &url,
            http::Method::POST,
            &[],
            Some(Bytes::from_static(b"posted over h3")),
        )
        .await
        .expect("h3 POST round-trip succeeds");

        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_ref(), b"posted over h3", "server echoed the request body");
    }
}
