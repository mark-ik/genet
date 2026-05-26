/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Fetch entry point.

use std::io;
use std::time::SystemTime;

use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, Full};
use url::Url;

use crate::cache::{self, StoredResponse};
use crate::client::shared_client;
use crate::cors;
use crate::decode::decode_stream;
use crate::hsts;
use crate::request::{Method, RedirectMode};
use crate::response::{ResponseBody, ResponseType};
use crate::{FetchContext, Request, Response};

/// WHATWG Fetch's redirect cap.
const MAX_REDIRECTS: u32 = 20;

/// Run the Fetch algorithm for `request` against `cx`.
///
/// Returns a [`Response`] in all cases — a network error is a `Response` with
/// `type` = error (there is no `Result`).
///
/// Real h1/h2 GET/POST over hyper + rustls; redirect handling (follow / error /
/// manual); streaming bodies with on-the-fly `Content-Encoding` decode; an RFC
/// 6265bis cookie jar (attach + record); an RFC 9111 cache (fresh hits served
/// without a round-trip, stale/`no-cache` entries revalidated via
/// `ETag`/`Last-Modified`); cross-origin response tainting + simple-request CORS
/// gating (`Basic`/`Cors`/`Opaque`, cross-origin CORS failures → network error);
/// HSTS + mixed-content auto-upgrade (a http target is rewritten to https when the
/// host is HSTS-known or the request runs in an https-origin context;
/// `Strict-Transport-Security` recorded over https); and the CSP `connect-src`
/// hook. Deferred: CORS preflight + response-header filtering, `SameSite`
/// enforcement, the active/passive mixed-content split, and HTTP/3.
pub async fn fetch(request: Request, cx: &FetchContext) -> Response {
    let client = shared_client();

    let mut current_url = request.url.clone();
    // A secure (https-origin) context drives mixed-content auto-upgrade; together
    // with HSTS this rewrites a http target to https before anything keys on it.
    let secure_context = request
        .origin
        .as_ref()
        .is_some_and(|o| o.ascii_serialization().starts_with("https://"));
    upgrade_to_https(&mut current_url, secure_context, cx);
    let mut method = request.method;
    let mut body = request.body.clone();
    let base_headers = request.headers.clone();
    let mut url_list = vec![current_url.clone()];
    let mut redirects_remaining = MAX_REDIRECTS;

    // HTTP cache (RFC 9111): only for GET, only when a real cache is wired.
    let now = SystemTime::now();
    let cache_key = (cx.cache.enabled() && matches!(method, Method::Get))
        .then(|| cache::cache_key("GET", &current_url));
    let mut revalidate: Option<StoredResponse> = None;
    if let Some(key) = &cache_key {
        if let Some(entry) = cx.cache.get(key) {
            if cache::is_fresh(&entry, now) && !cache::must_revalidate(&entry) {
                return cache::to_response(entry, url_list); // fresh hit — no network
            }
            if cache::has_validators(&entry) {
                revalidate = Some(entry); // stale / no-cache → conditional GET
            }
        }
    }

    loop {
        // CSP connect-src consultation (host-supplied policy).
        if !cx.csp.allows_connect(&current_url) {
            return Response::network_error();
        }

        let Ok(uri) = http::Uri::try_from(current_url.as_str()) else {
            return Response::network_error();
        };
        let mut builder = http::Request::builder()
            .method(http_method(method))
            .uri(uri);
        for (name, value) in &base_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let cookies = cx.cookies.cookies_for(&current_url);
        if !cookies.is_empty() {
            builder = builder.header(http::header::COOKIE, cookies.join("; "));
        }
        // Conditional revalidation headers, on the initial request only.
        if url_list.len() == 1 {
            if let Some(entry) = &revalidate {
                for (name, value) in cache::conditional_headers(entry) {
                    builder = builder.header(name.as_str(), value.as_str());
                }
            }
        }
        let wire_body = Full::new(body.clone().unwrap_or_default());
        let Ok(req) = builder.body(wire_body) else {
            return Response::network_error();
        };

        let resp = match client.request(req).await {
            Ok(resp) => resp,
            Err(_) => return Response::network_error(),
        };
        let status = resp.status();

        // Record any Set-Cookie headers against the URL that produced them.
        for value in resp.headers().get_all(http::header::SET_COOKIE).iter() {
            if let Ok(s) = value.to_str() {
                cx.cookies.set_cookie(&current_url, s);
            }
        }

        // Record HSTS policy — only honored when delivered over https.
        if current_url.scheme() == "https" {
            if let Some(sts) = resp
                .headers()
                .get(http::header::STRICT_TRANSPORT_SECURITY)
                .and_then(|v| v.to_str().ok())
            {
                if let Some((max_age, include_subdomains)) = hsts::parse_sts(sts) {
                    if let Some(host) = current_url.host_str() {
                        cx.hsts.record(host, max_age, include_subdomains);
                    }
                }
            }
        }

        // Redirect handling.
        if status.is_redirection() {
            if let Some(location) = resp
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                match request.redirect {
                    RedirectMode::Error => return Response::network_error(),
                    RedirectMode::Manual => {
                        return Response {
                            status: status.as_u16(),
                            headers: collect_headers(resp.headers()),
                            body: ResponseBody::empty(),
                            url_list,
                            response_type: ResponseType::OpaqueRedirect,
                        };
                    }
                    RedirectMode::Follow => {
                        if redirects_remaining == 0 {
                            return Response::network_error();
                        }
                        let Ok(next) = current_url.join(location) else {
                            return Response::network_error();
                        };
                        redirects_remaining -= 1;
                        method = redirect_method(status.as_u16(), method, &mut body);
                        current_url = next;
                        upgrade_to_https(&mut current_url, secure_context, cx);
                        url_list.push(current_url.clone());
                        continue;
                    }
                }
            }
            // A 3xx without a Location is delivered as an ordinary response.
        }

        // 304 Not Modified → serve (and refresh) the stored entry.
        if status.as_u16() == 304 {
            if let (Some(key), Some(entry)) = (&cache_key, revalidate.take()) {
                let refreshed = cache::refresh(entry, &collect_headers(resp.headers()), now);
                cx.cache.put(key, refreshed.clone());
                return cache::to_response(refreshed, url_list);
            }
        }

        // Terminal response: snapshot headers + encoding before consuming the body.
        let headers = collect_headers(resp.headers());
        let content_encoding = resp
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // Cross-origin policy: response tainting + CORS gating. A blocked CORS
        // check is a network error (and we skip even reading the body).
        let response_type = match cors::evaluate(
            request.origin.as_ref(),
            &current_url,
            request.mode,
            request.credentials,
            &headers,
        ) {
            cors::Taint::Basic => ResponseType::Basic,
            cors::Taint::Cors => ResponseType::Cors,
            cors::Taint::Opaque => ResponseType::Opaque,
            cors::Taint::Blocked => return Response::network_error(),
        };

        let data = resp
            .into_body()
            .into_data_stream()
            .map_err(|e| io::Error::other(e));
        let body = decode_stream(content_encoding.as_deref(), Box::pin(data));

        // Cacheable GET 200 → buffer the decoded body so we can store it, then hand
        // the caller that same buffer (a live stream can't be tee'd into the cache).
        if let Some(key) = &cache_key {
            if cache::is_cacheable(status.as_u16(), &headers) {
                let bytes = match ResponseBody::new(body).bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => return Response::network_error(),
                };
                let mut stored_headers = headers;
                strip_body_encoding_headers(&mut stored_headers);
                cx.cache.put(
                    key,
                    StoredResponse {
                        status: status.as_u16(),
                        headers: stored_headers.clone(),
                        body: bytes.clone(),
                        stored_at: now,
                    },
                );
                return Response {
                    status: status.as_u16(),
                    headers: stored_headers,
                    body: ResponseBody::from_bytes(bytes),
                    url_list,
                    response_type,
                };
            }
        }

        // Non-cacheable: stream straight through.
        return Response {
            status: status.as_u16(),
            headers,
            body: ResponseBody::new(body),
            url_list,
            response_type,
        };
    }
}

/// The stored/served body is decoded (identity), so its `Content-Encoding` and the
/// now-wrong `Content-Length` must not travel with it.
fn strip_body_encoding_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("content-encoding") && !k.eq_ignore_ascii_case("content-length")
    });
}

/// Rewrite a `http` URL to `https` when the request runs in a secure (https-origin)
/// context — mixed-content auto-upgrade — or the host is HSTS-known. The
/// active/passive split (block scripts, only upgrade media) is a later refinement;
/// it needs a request `destination` concept netfetcher doesn't model yet.
fn upgrade_to_https(url: &mut Url, secure_context: bool, cx: &FetchContext) {
    if url.scheme() == "http" && (secure_context || hsts::should_upgrade(url, cx.hsts.as_ref())) {
        let _ = url.set_scheme("https");
    }
}

fn http_method(method: Method) -> http::Method {
    match method {
        Method::Get => http::Method::GET,
        Method::Head => http::Method::HEAD,
        Method::Post => http::Method::POST,
        Method::Put => http::Method::PUT,
        Method::Delete => http::Method::DELETE,
        Method::Patch => http::Method::PATCH,
        Method::Options => http::Method::OPTIONS,
    }
}

fn collect_headers(map: &http::HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_owned(), s.to_owned())))
        .collect()
}

/// Method rewrite on redirect (WHATWG Fetch, HTTP-redirect fetch): 301/302 turn a
/// POST into a GET; 303 turns any non-GET/HEAD into a GET; 307/308 preserve.
/// A method change drops the body.
fn redirect_method(status: u16, method: Method, body: &mut Option<Bytes>) -> Method {
    match status {
        301 | 302 if method == Method::Post => {
            *body = None;
            Method::Get
        }
        303 if !matches!(method, Method::Get | Method::Head) => {
            *body = None;
            Method::Get
        }
        _ => method,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{InMemoryHttpCache, NoHttpCache};
    use crate::context::{AllowAllCsp, CookieStore};
    use crate::cookie_jar::InMemoryCookieJar;
    use std::sync::{Arc, Mutex};
    use url::Url;

    /// A context with a real in-memory cache (and jar) wired.
    fn caching_cx() -> FetchContext {
        FetchContext {
            cookies: Box::new(InMemoryCookieJar::new()),
            cache: Box::new(InMemoryHttpCache::new()),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(crate::InMemoryHsts::new()),
        }
    }

    #[tokio::test]
    async fn basic_get_returns_status_and_body() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/hello")
            .with_status(200)
            .with_body("hi there")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let url = format!("{}/hello", server.url());
        let res = fetch(Request::get(url.parse().unwrap()), &cx).await;

        m.assert_async().await;
        assert!(!res.is_network_error());
        assert_eq!(res.status, 200);
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"hi there");
    }

    #[tokio::test]
    async fn follows_a_redirect_and_records_the_chain() {
        let mut server = mockito::Server::new_async().await;
        let _r1 = server
            .mock("GET", "/a")
            .with_status(302)
            .with_header("location", "/b")
            .create_async()
            .await;
        let _r2 = server
            .mock("GET", "/b")
            .with_status(200)
            .with_body("arrived")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let res = fetch(
            Request::get(format!("{}/a", server.url()).parse().unwrap()),
            &cx,
        )
        .await;

        assert_eq!(res.status, 200);
        assert_eq!(res.url_list.len(), 2, "original + redirect target");
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"arrived");
    }

    #[tokio::test]
    async fn redirect_error_mode_yields_network_error() {
        let mut server = mockito::Server::new_async().await;
        let _r = server
            .mock("GET", "/a")
            .with_status(302)
            .with_header("location", "/b")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let mut req = Request::get(format!("{}/a", server.url()).parse().unwrap());
        req.redirect = RedirectMode::Error;
        let res = fetch(req, &cx).await;

        assert!(res.is_network_error());
    }

    #[tokio::test]
    async fn decodes_gzip_content_encoding() {
        use std::io::Write;
        let mut enc =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"compressed hello").unwrap();
        let gz = enc.finish().unwrap();

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/gz")
            .with_status(200)
            .with_header("content-encoding", "gzip")
            .with_body(gz)
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let res = fetch(
            Request::get(format!("{}/gz", server.url()).parse().unwrap()),
            &cx,
        )
        .await;

        assert_eq!(res.bytes().await.unwrap().as_ref(), b"compressed hello");
    }

    #[tokio::test]
    async fn records_set_cookie_through_the_jar() {
        #[derive(Clone, Default)]
        struct SpyJar(Arc<Mutex<Vec<String>>>);
        impl CookieStore for SpyJar {
            fn cookies_for(&self, _: &Url) -> Vec<String> {
                Vec::new()
            }
            fn set_cookie(&self, _: &Url, header: &str) {
                self.0.lock().unwrap().push(header.to_owned());
            }
        }

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/c")
            .with_status(200)
            .with_header("set-cookie", "id=abc")
            .with_body("ok")
            .create_async()
            .await;

        let spy = SpyJar::default();
        let cx = FetchContext {
            cookies: Box::new(spy.clone()),
            cache: Box::new(NoHttpCache),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(crate::InMemoryHsts::new()),
        };
        let res = fetch(
            Request::get(format!("{}/c", server.url()).parse().unwrap()),
            &cx,
        )
        .await;

        assert_eq!(res.status, 200);
        assert_eq!(spy.0.lock().unwrap().as_slice(), &["id=abc".to_string()]);
    }

    #[tokio::test]
    async fn fresh_response_served_from_cache_without_a_second_request() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/cached")
            .with_status(200)
            .with_header("cache-control", "max-age=300")
            .with_body("v1")
            .expect(1) // one network hit despite two fetches
            .create_async()
            .await;

        let cx = caching_cx();
        let url = format!("{}/cached", server.url());
        let r1 = fetch(Request::get(url.parse().unwrap()), &cx).await;
        assert_eq!(r1.bytes().await.unwrap().as_ref(), b"v1");
        let r2 = fetch(Request::get(url.parse().unwrap()), &cx).await;
        assert_eq!(r2.bytes().await.unwrap().as_ref(), b"v1");

        m.assert_async().await; // exactly one upstream request
    }

    #[tokio::test]
    async fn stale_entry_revalidates_via_304_and_serves_stored_body() {
        let mut server = mockito::Server::new_async().await;
        // Initial load: immediately stale (max-age=0), carries an ETag.
        let initial = server
            .mock("GET", "/r")
            .match_header("if-none-match", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("cache-control", "max-age=0")
            .with_header("etag", "\"v1\"")
            .with_body("hello")
            .expect(1)
            .create_async()
            .await;
        // Conditional revalidation returns 304.
        let revalidated = server
            .mock("GET", "/r")
            .match_header("if-none-match", "\"v1\"")
            .with_status(304)
            .expect(1)
            .create_async()
            .await;

        let cx = caching_cx();
        let url = format!("{}/r", server.url());
        let r1 = fetch(Request::get(url.parse().unwrap()), &cx).await;
        assert_eq!(r1.bytes().await.unwrap().as_ref(), b"hello");
        let r2 = fetch(Request::get(url.parse().unwrap()), &cx).await;
        assert_eq!(r2.status, 200, "304 is served as the stored 200, not a 304");
        assert_eq!(r2.bytes().await.unwrap().as_ref(), b"hello");

        initial.assert_async().await;
        revalidated.assert_async().await;
    }

    #[tokio::test]
    async fn no_store_response_is_not_cached() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/ns")
            .with_status(200)
            .with_header("cache-control", "no-store")
            .with_body("v1")
            .expect(2) // two fetches → two upstream hits
            .create_async()
            .await;

        let cx = caching_cx();
        let url = format!("{}/ns", server.url());
        let _ = fetch(Request::get(url.parse().unwrap()), &cx).await.bytes().await;
        let _ = fetch(Request::get(url.parse().unwrap()), &cx).await.bytes().await;

        m.assert_async().await;
    }

    fn origin_of(s: &str) -> url::Origin {
        Url::parse(s).unwrap().origin()
    }

    #[tokio::test]
    async fn cross_origin_cors_pass_taints_cors() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api")
            .with_status(200)
            .with_header("access-control-allow-origin", "http://app.example")
            .with_body("data")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let req = Request::get(format!("{}/api", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        let res = fetch(req, &cx).await;

        assert_eq!(res.response_type, ResponseType::Cors);
        assert!(!res.is_network_error());
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"data");
    }

    #[tokio::test]
    async fn cross_origin_cors_without_header_is_blocked() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api")
            .with_status(200)
            .with_body("data")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let req = Request::get(format!("{}/api", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        let res = fetch(req, &cx).await;

        assert!(res.is_network_error(), "cross-origin CORS with no ACAO is blocked");
    }

    #[tokio::test]
    async fn cross_origin_no_cors_is_opaque() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/img")
            .with_status(200)
            .with_body("data")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let mut req = Request::get(format!("{}/img", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        req.mode = crate::RequestMode::NoCors;
        let res = fetch(req, &cx).await;

        assert_eq!(res.response_type, ResponseType::Opaque);
    }

    #[test]
    fn mixed_content_upgrades_in_secure_context() {
        let cx = FetchContext::permissive();
        let mut url: Url = "http://example.org/x".parse().unwrap();
        upgrade_to_https(&mut url, true, &cx);
        assert_eq!(url.scheme(), "https", "https-origin context upgrades http target");

        // No secure context and no HSTS entry → left as http.
        let mut insecure: Url = "http://example.org/x".parse().unwrap();
        upgrade_to_https(&mut insecure, false, &cx);
        assert_eq!(insecure.scheme(), "http");
    }
}
