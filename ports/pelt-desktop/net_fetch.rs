/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The remote half of [`LocalFetcher`](crate::document::LocalFetcher) (the
//! `netfetch` feature): http(s) document loading over the netfetcher engine.
//!
//! pelt is serval's reference *host*, so -- like meerkat in the product -- it owns
//! networking and drives [`netfetcher`] (the sibling WHATWG-Fetch engine); serval's
//! engine components stay byte-consuming and never link it. `ResourceFetcher::fetch`
//! is synchronous, so netfetcher's async `fetch` is bridged onto it through a small
//! tokio runtime, block-on per request -- the document load is a one-shot at open
//! time, not a per-frame cost. The same wiring serval-wpt's `fetch()` uses.

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

#[cfg(test)]
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
