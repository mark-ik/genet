/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The remote half of [`LocalFetcher`](crate::document::LocalFetcher): http(s)
//! document loading over the netfetcher engine (the `netfetch` feature), and smolweb
//! (gemini/gopher/nex/finger/spartan/guppy) over the errand transport (the `smolweb`
//! feature).
//!
//! pelt is serval's reference *host*, so -- like meerkat in the product -- it owns
//! networking and drives the sibling engines ([`netfetcher`] for the web, errand for
//! smolweb); serval's engine components stay byte-consuming and never link them.
//! `ResourceFetcher::fetch` is synchronous, so the engines' async `fetch` is bridged
//! onto it through a small tokio runtime, block-on per request -- the document load is
//! a one-shot at open time, not a per-frame cost. The same wiring serval-wpt's
//! `fetch()` uses.

use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// The shared tokio runtime the blocking bridge drives. Built once on first use: a
/// current-thread runtime is enough (a one-shot GET; `block_on` drives the hyper
/// connection task spawned inside `fetch`). `enable_all` lights the IO + time drivers
/// netfetcher's transport needs (its own tokio feature set enables them in the
/// unified build).
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("pelt netfetch tokio runtime")
    })
}

/// Blocking http(s) GET of `url`, returning the response body bytes, or `None` on a
/// parse / network error or a non-2xx status. The remote branch of
/// [`LocalFetcher::fetch`](crate::document::LocalFetcher); mirrors serval-wpt's
/// `do_get`, returning raw bytes (the document parser owns charset handling) rather
/// than a lossy string.
#[cfg(feature = "netfetch")]
pub(crate) fn http_get_bytes(url: &str) -> Option<Vec<u8>> {
    runtime().block_on(async move {
        let parsed = url::Url::parse(url).ok()?;
        let request = netfetcher::Request::get(parsed);
        let cx = netfetcher::FetchContext::permissive();
        let response = netfetcher::fetch(request, &cx).await;
        if response.is_network_error() || response.status < 200 || response.status >= 300 {
            return None;
        }
        response.bytes().await.ok().map(|b| b.to_vec())
    })
}

/// Blocking smolweb GET of `url` over the errand transport, returning the response
/// body on a success status, or `None` on a non-success status (input / redirect /
/// failure / cert-required) or a transport error. The smolweb branch of
/// [`LocalFetcher::fetch`](crate::document::LocalFetcher); mirrors `http_get_bytes`,
/// bridging errand's async `fetch` onto the sync `ResourceFetcher` through the shared
/// runtime. The caller surfaces the `None` as a clean load error rather than painting
/// a protocol error line as a document, matching the http path's non-2xx handling.
#[cfg(feature = "smolweb")]
pub(crate) fn smolweb_get_bytes(url: &str) -> Option<Vec<u8>> {
    install_tofu();
    runtime().block_on(async move {
        match errand::fetch(url).await {
            Ok(resp) if resp.status == errand::Status::Success => Some(resp.body),
            _ => None,
        }
    })
}

/// Install an [`errand::InMemoryTofu`] once for the process, so gemini certificate
/// pins persist across requests in a session: a first contact is trusted-on-first-use
/// and a later mismatch (a possible MITM or a key rotation) surfaces as a failed load
/// rather than a silent re-pin. Without this errand defaults to accept-any
/// (`PermissiveTofu`); the reference shell opts into real pinning. A durable on-disk
/// store is a later rung.
#[cfg(feature = "smolweb")]
fn install_tofu() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        errand::set_trust_store(std::sync::Arc::new(errand::InMemoryTofu::new()));
    });
}

#[cfg(all(test, feature = "netfetch"))]
mod tests {
    use pelt_core::ResourceFetcher;

    use crate::document::LocalFetcher;

    /// http(s) loading flows through the netfetcher engine end to end: an offline
    /// mock server serves a body, and `LocalFetcher` (with the `netfetch` branch)
    /// fetches its bytes -- the same path `pelt --engine static https://…` takes,
    /// proven without a live network.
    #[test]
    fn local_fetcher_gets_http_bytes_via_netfetcher() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/page.html")
            .with_status(200)
            .with_header("content-type", "text/html; charset=utf-8")
            .with_body("<h1>From the network</h1>")
            .create();

        let url = format!("{}/page.html", server.url());
        let bytes = LocalFetcher.fetch(&url).expect("the http(s) document fetches over netfetcher");
        assert_eq!(bytes, b"<h1>From the network</h1>", "the fetched bytes are the served body");
        mock.assert();
    }

    /// A non-2xx response is `None` (the caller surfaces a load error), not the error
    /// body painted as a document.
    #[test]
    fn http_not_found_is_none() {
        let mut server = mockito::Server::new();
        let mock = server.mock("GET", "/missing").with_status(404).with_body("nope").create();

        let url = format!("{}/missing", server.url());
        assert!(LocalFetcher.fetch(&url).is_none(), "a 404 is a failed load, not a document");
        mock.assert();
    }
}

#[cfg(all(test, feature = "smolweb"))]
mod smolweb_tests {
    use pelt_core::ResourceFetcher;

    use crate::document::LocalFetcher;

    /// A smolweb scheme is recognized and routed to the errand transport, and a host
    /// that cannot resolve fails to a clean `None` (a failed load, not a panic or an
    /// error document) -- the same contract the http path holds for a non-2xx. Uses a
    /// `.invalid` host (RFC 6761 guarantees NXDOMAIN, answered locally) so the test
    /// needs no live capsule, and exercises the one-time TOFU install on the way.
    #[test]
    fn smolweb_scheme_routes_and_unresolvable_host_is_none() {
        assert!(
            LocalFetcher.fetch("gemini://capsule.invalid/").is_none(),
            "an unresolvable gemini host is a failed load, not a document"
        );
    }

    /// A non-smolweb, non-http unknown scheme is not routed to errand; it falls
    /// through to the filesystem attempt and fails to `None`.
    #[test]
    fn unknown_scheme_is_not_routed_to_errand() {
        assert!(
            LocalFetcher.fetch("wat://nope/").is_none(),
            "a non-smolweb scheme is not an errand fetch nor a readable path"
        );
    }
}
