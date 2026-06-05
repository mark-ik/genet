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

use crate::altsvc;
use crate::cache::{self, StoredResponse};
use crate::client::shared_client;
use crate::cors;
use crate::decode::decode_stream;
use crate::hsts;
use crate::referrer;
use crate::request::{CacheMode, Credentials, Method, RedirectMode, RequestMode};
use crate::response::{BodyStream, ResponseBody, ResponseType};
use crate::{FetchContext, Request, Response, SameSiteContext};

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
/// `Strict-Transport-Security` recorded over https); SameSite cookie gating
/// (Strict/Lax, same-site approximated by registrable domain); CORS preflight
/// (OPTIONS, Max-Age-cached) + `Cors` response-header filtering; the CSP
/// `connect-src` hook; and **HTTP/3** via Alt-Svc (a transport-abstracted h3 lane
/// over quinn, with h1/h2 fallback). Deferred: the active/passive mixed-content
/// split, public-suffix-accurate same-site, and h3 for requests with bodies.
pub async fn fetch(request: Request, cx: &FetchContext) -> Response {
    let mut current_url = request.url.clone();
    // A secure (https-origin) context drives mixed-content auto-upgrade; together
    // with HSTS this rewrites a http target to https before anything keys on it.
    let secure_context = request
        .origin
        .as_ref()
        .is_some_and(|o| o.ascii_serialization().starts_with("https://"));
    if resolve_mixed_content(&mut current_url, request.destination, secure_context, cx) {
        return Response::network_error();
    }
    let mut method = request.method.clone();
    let mut body = request.body.clone();
    let mut base_headers = request.headers.clone();
    let mut url_list = vec![current_url.clone()];
    let mut redirects_remaining = MAX_REDIRECTS;
    // Once a cross-origin redirect leaves an already-foreign URL, the request's
    // origin becomes opaque, so the `Origin` header flips to "null" (WHATWG
    // HTTP-redirect fetch).
    let mut origin_tainted = false;
    // The active referrer policy; a redirect's `Referrer-Policy` response header
    // can override it for subsequent hops.
    let mut referrer_policy = request.referrer_policy;
    // The request's referrer, reduced to whatever a hop's policy permits. A
    // restrictive policy (no-referrer / origin) "sticks": once reduced, a later
    // redirect that loosens the policy cannot recover the original URL.
    let mut referrer = request.referrer.clone();

    // HTTP cache (RFC 9111 + request cache mode): only for GET, only when a real
    // cache is wired, and never for `no-store`.
    let now = SystemTime::now();
    let mode = request.cache;
    let cache_key =
        (cx.cache.enabled() && matches!(method, Method::Get) && mode != CacheMode::NoStore)
            .then(|| cache::cache_key("GET", &current_url));
    let mut revalidate: Option<StoredResponse> = None;
    if let Some(key) = &cache_key {
        // `reload` always goes to the network (but still stores); the others may
        // consult the stored entry.
        if mode != CacheMode::Reload {
            match cx.cache.get(key) {
                Some(entry) => match mode {
                    // Use the stored response as-is, even if stale.
                    CacheMode::ForceCache | CacheMode::OnlyIfCached => {
                        return cache::to_response(entry, url_list);
                    }
                    // Always revalidate.
                    CacheMode::NoCache => {
                        if cache::has_validators(&entry) {
                            revalidate = Some(entry);
                        }
                    }
                    // Default: serve fresh, revalidate stale / no-cache.
                    _ => {
                        if cache::is_fresh(&entry, now) && !cache::must_revalidate(&entry) {
                            return cache::to_response(entry, url_list);
                        }
                        if cache::has_validators(&entry) {
                            revalidate = Some(entry);
                        }
                    }
                },
                // `only-if-cached` with no stored response is a network error.
                None if mode == CacheMode::OnlyIfCached => return Response::network_error(),
                None => {}
            }
        }
    }

    // CORS preflight: a cross-origin, cors-mode, non-simple request gets an OPTIONS
    // round-trip first (cached per Access-Control-Max-Age).
    let cross_origin = request
        .origin
        .as_ref()
        .is_some_and(|o| *o != current_url.origin());
    if cross_origin
        && matches!(request.mode, RequestMode::Cors)
        && cors::needs_preflight(&method, &base_headers)
    {
        let requested = cors::preflight_request_headers(&base_headers);
        let key = cors::preflight_key(request.origin.as_ref(), &current_url, &method, &requested);
        if !cx.preflight.check(&key) {
            match run_preflight(
                &current_url,
                request.origin.as_ref(),
                &method,
                &requested,
                request.credentials,
            )
            .await
            {
                Some(max_age) => cx.preflight.store(&key, max_age),
                None => return Response::network_error(),
            }
        }
    }

    loop {
        // CSP connect-src consultation (host-supplied policy).
        if !cx.csp.allows_connect(&current_url) {
            return Response::network_error();
        }

        // Whether this hop uses credentials (cookies/auth): always for `include`,
        // never for `omit`, and only same-origin for `same-origin` (WHATWG
        // HTTP-network-or-cache fetch). Same-origin is recomputed per hop.
        let same_origin = request
            .origin
            .as_ref()
            .is_none_or(|o| *o == current_url.origin());
        let use_credentials = match request.credentials {
            Credentials::Include => true,
            Credentials::Omit => false,
            Credentials::SameOrigin => same_origin,
        };

        // Assemble request headers: base + cookies + (initial-only) conditional.
        let mut req_headers = base_headers.clone();
        let cookies = if use_credentials {
            cx.cookies.cookies_for(
                &current_url,
                SameSiteContext {
                    same_site: is_same_site(request.origin.as_ref(), &current_url),
                    top_level_navigation: matches!(request.mode, RequestMode::Navigate),
                },
            )
        } else {
            Vec::new()
        };
        if !cookies.is_empty() {
            req_headers.push(("cookie".to_owned(), cookies.join("; ")));
        }
        if url_list.len() == 1 {
            if let Some(entry) = &revalidate {
                req_headers.extend(cache::conditional_headers(entry));
            }
            // Cache-mode request headers (WHATWG HTTP-network-or-cache fetch): the
            // server logs these, and the cache tests assert on them.
            match mode {
                CacheMode::NoCache => {
                    if header_val(&req_headers, "cache-control").is_none() {
                        req_headers.push(("cache-control".to_owned(), "max-age=0".to_owned()));
                    }
                }
                CacheMode::NoStore | CacheMode::Reload => {
                    if header_val(&req_headers, "pragma").is_none() {
                        req_headers.push(("pragma".to_owned(), "no-cache".to_owned()));
                    }
                    if header_val(&req_headers, "cache-control").is_none() {
                        req_headers.push(("cache-control".to_owned(), "no-cache".to_owned()));
                    }
                }
                _ => {}
            }
        }

        // Append the `Origin` header for a cross-origin request (or one whose
        // origin has been tainted to opaque by a cross-origin redirect, giving
        // "null"). A same-origin request carries no `Origin` — matching observed
        // browser behavior (the WPT redirect-origin oracle), which is narrower than
        // the spec's literal "append for any non-GET/HEAD method".
        if let Some(origin) = request.origin.as_ref() {
            let cur_cross = origin_tainted || *origin != current_url.origin();
            if cur_cross && header_val(&req_headers, "origin").is_none() {
                let value = if origin_tainted {
                    "null".to_owned()
                } else {
                    origin.ascii_serialization()
                };
                req_headers.push(("origin".to_owned(), value));
            }
        }

        // Append the `Referer` header from the request's referrer under the active
        // policy (recomputed per hop; the target's origin can change the answer).
        // The computed value also replaces the referrer, so a policy's reduction
        // carries forward to later hops.
        if let Some(r) = referrer.as_ref() {
            let value = referrer::referrer_header(r, &current_url, referrer_policy);
            referrer = value.as_deref().and_then(|v| Url::parse(v).ok());
            if let Some(value) = value {
                if header_val(&req_headers, "referer").is_none() {
                    req_headers.push(("referer".to_owned(), value));
                }
            }
        }

        // Transport: prefer h3 when this https origin advertised it (Alt-Svc).
        let try_h3 = current_url.scheme() == "https"
            && current_url.host_str().and_then(|h| cx.alt_svc.h3_port(h)).is_some();
        let raw = match send_request(&current_url, &method, &req_headers, body.as_ref(), try_h3).await
        {
            Some(raw) => raw,
            None => return Response::network_error(),
        };
        let status = raw.status;

        // Record any Set-Cookie headers against the URL that produced them — only
        // when this hop uses credentials (an `omit` fetch stores nothing).
        if use_credentials {
            for (name, value) in &raw.headers {
                if name.eq_ignore_ascii_case("set-cookie") {
                    cx.cookies.set_cookie(&current_url, value);
                }
            }
        }

        // Record HSTS policy — only honored when delivered over https.
        if current_url.scheme() == "https" {
            if let Some(sts) = header_val(&raw.headers, "strict-transport-security") {
                if let Some((max_age, include_subdomains)) = hsts::parse_sts(sts) {
                    if let Some(host) = current_url.host_str() {
                        cx.hsts.record(host, max_age, include_subdomains);
                    }
                }
            }
        }

        // Record Alt-Svc advertisements so future requests to this origin use h3.
        if let Some(host) = current_url.host_str() {
            if let Some(value) = header_val(&raw.headers, "alt-svc") {
                match altsvc::parse_alt_svc(value) {
                    altsvc::AltSvc::H3 { port, max_age } => cx.alt_svc.record_h3(host, port, max_age),
                    altsvc::AltSvc::Clear => cx.alt_svc.clear(host),
                    altsvc::AltSvc::None => {}
                }
            }
        }

        // Redirect handling.
        if (300..400).contains(&status) {
            // Own the location so `raw.headers` is free to move below.
            let location = header_val(&raw.headers, "location").map(str::to_owned);
            // Error / manual redirect modes apply to any redirect status, even one
            // without a `Location` header (only Follow needs a target).
            match (request.redirect, &location) {
                (RedirectMode::Error, None) => return Response::network_error(),
                (RedirectMode::Manual, None) => {
                    return Response {
                        status: 0,
                        headers: Vec::new(),
                        body: ResponseBody::empty(),
                        url_list,
                        response_type: ResponseType::OpaqueRedirect,
                    };
                }
                _ => {}
            }
            if let Some(location) = location {
                // Gate the redirect response *before* the redirect-mode switch
                // (WHATWG HTTP fetch runs the CORS check on the actual response,
                // a 3xx included, ahead of redirect processing):
                let req_cross = origin_tainted
                    || request
                        .origin
                        .as_ref()
                        .is_some_and(|o| *o != current_url.origin());
                // A cors-tainted request whose redirect fails the CORS check is a
                // network error even under manual/error redirect modes.
                if req_cross
                    && matches!(request.mode, RequestMode::Cors)
                    && matches!(
                        cors::evaluate(
                            request.origin.as_ref(),
                            &current_url,
                            request.mode,
                            request.credentials,
                            &raw.headers,
                        ),
                        cors::Taint::Blocked
                    )
                {
                    return Response::network_error();
                }
                // A no-cors cross-origin request may not observe a redirect with a
                // non-follow redirect mode (it would leak the cross-origin hop).
                if req_cross
                    && matches!(request.mode, RequestMode::NoCors)
                    && !matches!(request.redirect, RedirectMode::Follow)
                {
                    return Response::network_error();
                }
                match request.redirect {
                    RedirectMode::Error => return Response::network_error(),
                    RedirectMode::Manual => {
                        // A manual-redirect filtered response: status 0, no headers,
                        // no body (WHATWG opaque-redirect).
                        return Response {
                            status: 0,
                            headers: Vec::new(),
                            body: ResponseBody::empty(),
                            url_list,
                            response_type: ResponseType::OpaqueRedirect,
                        };
                    }
                    RedirectMode::Follow => {
                        if redirects_remaining == 0 {
                            return Response::network_error();
                        }
                        let Ok(next) = current_url.join(&location) else {
                            return Response::network_error();
                        };
                        // A redirect to a URL embedding credentials
                        // (user:password@host) is a network error for any
                        // non-navigate request (WHATWG HTTP-redirect fetch).
                        if (!next.username().is_empty() || next.password().is_some())
                            && !matches!(request.mode, RequestMode::Navigate)
                        {
                            return Response::network_error();
                        }
                        redirects_remaining -= 1;
                        // A `Referrer-Policy` on the redirect response governs the
                        // `Referer` header for subsequent hops.
                        if let Some(rp) = header_val(&raw.headers, "referrer-policy") {
                            referrer_policy = referrer::policy_from_header(referrer_policy, rp);
                        }
                        // Taint the origin to opaque when this redirect is
                        // cross-origin *and* the current URL was already foreign to
                        // the initiator (the second cross-origin hop): the `Origin`
                        // header becomes "null" from here on.
                        let crosses = current_url.origin() != next.origin();
                        let already_foreign = origin_tainted
                            || request
                                .origin
                                .as_ref()
                                .is_some_and(|o| *o != current_url.origin());
                        if crosses && already_foreign {
                            origin_tainted = true;
                        }
                        let prev_method = method.clone();
                        method = redirect_method(status, method, &mut body);
                        // A method-changing redirect (301/302 POST->GET, 303 ->GET)
                        // drops the body, so the request-body headers go too — per
                        // WHATWG HTTP-redirect fetch (regardless of whether a body
                        // was present).
                        if method != prev_method {
                            base_headers.retain(|(k, _)| {
                                !matches!(
                                    k.to_ascii_lowercase().as_str(),
                                    "content-type" | "content-length" | "content-encoding"
                                        | "content-language" | "content-location"
                                )
                            });
                        }
                        current_url = next;
                        if resolve_mixed_content(
                            &mut current_url,
                            request.destination,
                            secure_context,
                            cx,
                        ) {
                            return Response::network_error();
                        }
                        url_list.push(current_url.clone());
                        continue;
                    }
                }
            }
            // A 3xx without a Location is delivered as an ordinary response.
        }

        // 304 Not Modified → serve (and refresh) the stored entry.
        if status == 304 {
            if let (Some(key), Some(entry)) = (&cache_key, revalidate.take()) {
                let refreshed = cache::refresh(entry, &raw.headers, now);
                cx.cache.put(key, refreshed.clone());
                return cache::to_response(refreshed, url_list);
            }
        }

        // Terminal: tainting/filtering, then decode the transport-agnostic body.
        let content_encoding = header_val(&raw.headers, "content-encoding").map(str::to_owned);
        let response_type = match cors::evaluate(
            request.origin.as_ref(),
            &current_url,
            request.mode,
            request.credentials,
            &raw.headers,
        ) {
            cors::Taint::Basic => ResponseType::Basic,
            cors::Taint::Cors => ResponseType::Cors,
            cors::Taint::Opaque => ResponseType::Opaque,
            cors::Taint::Blocked => return Response::network_error(),
        };
        // An opaque (cross-origin no-cors) response is filtered: status 0, no
        // headers, no exposed body — and never cached.
        if matches!(response_type, ResponseType::Opaque) {
            return Response {
                status: 0,
                headers: Vec::new(),
                body: ResponseBody::empty(),
                url_list,
                response_type: ResponseType::Opaque,
            };
        }
        let headers = if matches!(response_type, ResponseType::Cors) {
            cors::filter_cors_response_headers(raw.headers)
        } else {
            raw.headers
        };
        let body = decode_stream(content_encoding.as_deref(), raw.body);

        // Cacheable GET 200 → buffer the decoded body to store it, then hand the
        // caller that same buffer (a live stream can't be tee'd into the cache).
        if let Some(key) = &cache_key {
            if cache::is_cacheable(status, &headers) {
                let bytes = match ResponseBody::new(body).bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => return Response::network_error(),
                };
                let mut stored_headers = headers;
                strip_body_encoding_headers(&mut stored_headers);
                cx.cache.put(
                    key,
                    StoredResponse {
                        status,
                        headers: stored_headers.clone(),
                        body: bytes.clone(),
                        stored_at: now,
                    },
                );
                return Response {
                    status,
                    headers: stored_headers,
                    body: ResponseBody::from_bytes(bytes),
                    url_list,
                    response_type,
                };
            }
        }

        // Non-cacheable: stream straight through.
        return Response {
            status,
            headers,
            body: ResponseBody::new(body),
            url_list,
            response_type,
        };
    }
}

/// Normalized transport response: status + headers + a raw (undecoded) body
/// stream, produced by either the h1/h2 or the h3 path so the loop's back half is
/// transport-agnostic.
struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: BodyStream,
}

/// Send one request over h3 (if `try_h3` and available) or h1/h2, normalizing the
/// result to a [`RawResponse`]. An h3 failure falls back to h1/h2.
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
async fn send_request(
    url: &Url,
    method: &Method,
    headers: &[(String, String)],
    body: Option<&Bytes>,
    try_h3: bool,
) -> Option<RawResponse> {
    #[cfg(not(target_arch = "wasm32"))]
    if try_h3 {
        if let Some(h3) =
            crate::h3_client::fetch_h3_default(url, http_method(method), headers, body.cloned())
                .await
        {
            return Some(RawResponse {
                status: h3.status,
                headers: h3.headers,
                body: once_body(h3.body),
            });
        }
        // h3 attempt failed → fall back to h1/h2.
    }

    let uri = http::Uri::try_from(url.as_str()).ok()?;
    let mut builder = http::Request::builder().method(http_method(method)).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let req = builder.body(Full::new(body.cloned().unwrap_or_default())).ok()?;
    let resp = shared_client().request(req).await.ok()?;
    let status = resp.status().as_u16();
    let headers = collect_headers(resp.headers());
    let data = resp.into_body().into_data_stream().map_err(|e| io::Error::other(e));
    Some(RawResponse {
        status,
        headers,
        body: Box::pin(data),
    })
}

/// Wrap already-collected bytes as a single-chunk body stream (the h3 path; the
/// shared decode step then handles its `Content-Encoding` uniformly).
fn once_body(bytes: Bytes) -> BodyStream {
    Box::pin(futures_util::stream::once(async move { Ok::<_, io::Error>(bytes) }))
}

fn header_val<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The stored/served body is decoded (identity), so its `Content-Encoding` and the
/// now-wrong `Content-Length` must not travel with it.
fn strip_body_encoding_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("content-encoding") && !k.eq_ignore_ascii_case("content-length")
    });
}

/// Resolve HSTS + mixed content for `url`, returning `true` if the request must
/// be **blocked** (a network error).
///
/// HSTS is host-keyed and independent of content type: a known host upgrades and
/// proceeds. Otherwise, in a secure (https-origin) context the mixed-content
/// active/passive split applies — optionally-blockable destinations (image /
/// audio / video) are auto-upgraded http→https, and blockable ones (script /
/// style / font / document / the empty fetch() destination) are blocked. Outside
/// a secure context with no HSTS entry, plain http is left as-is.
fn resolve_mixed_content(
    url: &mut Url,
    destination: crate::Destination,
    secure_context: bool,
    cx: &FetchContext,
) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    if hsts::should_upgrade(url, cx.hsts.as_ref()) {
        let _ = url.set_scheme("https");
        return false;
    }
    if !secure_context {
        return false;
    }
    if destination.is_optionally_blockable() {
        let _ = url.set_scheme("https");
        false
    } else {
        true
    }
}

/// Same-site test for SameSite cookie gating: equal registrable domains, via the
/// Public Suffix List. No initiator origin = a top-level request → same-site.
fn is_same_site(origin: Option<&url::Origin>, target: &Url) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    match (origin_host(origin), target.host_str()) {
        (Some(oh), Some(th)) => same_registrable_domain(&oh, th),
        _ => false,
    }
}

fn origin_host(origin: &url::Origin) -> Option<String> {
    match origin {
        url::Origin::Tuple(_, host, _) => Some(host.to_string()),
        url::Origin::Opaque(_) => None,
    }
}

/// Whether two hosts share a registrable domain (eTLD+1) per the PSL. Hosts the
/// PSL can't resolve to a registrable domain — IP literals, single labels,
/// unlisted TLDs — fall back to an exact host match.
fn same_registrable_domain(a: &str, b: &str) -> bool {
    match (psl::domain_str(a), psl::domain_str(b)) {
        (Some(da), Some(db)) => da.eq_ignore_ascii_case(db),
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// Send a CORS preflight `OPTIONS` and verify it. `Some(max_age)` if the actual
/// request is permitted, `None` if denied (or the preflight itself failed).
async fn run_preflight(
    target: &Url,
    origin: Option<&url::Origin>,
    method: &Method,
    requested_headers: &[String],
    credentials: Credentials,
) -> Option<u64> {
    let uri = http::Uri::try_from(target.as_str()).ok()?;
    let mut builder = http::Request::builder()
        .method(http::Method::OPTIONS)
        .uri(uri)
        .header("accept", "*/*")
        .header("access-control-request-method", http_method(method).as_str());
    if let Some(o) = origin {
        builder = builder.header(http::header::ORIGIN, o.ascii_serialization());
    }
    if !requested_headers.is_empty() {
        builder = builder.header("access-control-request-headers", requested_headers.join(","));
    }
    let req = builder.body(Full::new(Bytes::new())).ok()?;
    let resp = shared_client().request(req).await.ok()?;
    let headers = collect_headers(resp.headers());
    cors::preflight_verdict(origin, credentials, method, requested_headers, &headers)
}

fn http_method(method: &Method) -> http::Method {
    match method {
        Method::Get => http::Method::GET,
        Method::Head => http::Method::HEAD,
        Method::Post => http::Method::POST,
        Method::Put => http::Method::PUT,
        Method::Delete => http::Method::DELETE,
        Method::Patch => http::Method::PATCH,
        Method::Options => http::Method::OPTIONS,
        // A custom token: build an http::Method, falling back to GET if (somehow)
        // it isn't a valid method token.
        Method::Other(m) => {
            http::Method::from_bytes(m.as_bytes()).unwrap_or(http::Method::GET)
        }
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
            cache: std::sync::Arc::new(InMemoryHttpCache::new()),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(crate::InMemoryHsts::new()),
            preflight: Box::new(crate::InMemoryPreflightCache::new()),
            alt_svc: Box::new(crate::InMemoryAltSvc::new()),
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
            fn cookies_for(&self, _: &Url, _: SameSiteContext) -> Vec<String> {
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
            cache: std::sync::Arc::new(NoHttpCache),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(crate::InMemoryHsts::new()),
            preflight: Box::new(crate::InMemoryPreflightCache::new()),
            alt_svc: Box::new(crate::InMemoryAltSvc::new()),
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
    fn mixed_content_active_passive_split() {
        use crate::Destination;
        let cx = FetchContext::permissive();

        // Optionally-blockable (image) in a secure context → auto-upgraded, allowed.
        let mut img: Url = "http://example.org/x.png".parse().unwrap();
        assert!(!resolve_mixed_content(&mut img, Destination::Image, true, &cx));
        assert_eq!(img.scheme(), "https", "passive mixed content is upgraded");

        // Blockable (script, and the empty fetch() destination) in a secure
        // context → blocked.
        let mut script: Url = "http://example.org/a.js".parse().unwrap();
        assert!(resolve_mixed_content(&mut script, Destination::Other, true, &cx));
        let mut xhr: Url = "http://example.org/api".parse().unwrap();
        assert!(resolve_mixed_content(&mut xhr, Destination::None, true, &cx));

        // No secure context, no HSTS → plain http left as-is, not blocked.
        let mut insecure: Url = "http://example.org/a.js".parse().unwrap();
        assert!(!resolve_mixed_content(&mut insecure, Destination::Other, false, &cx));
        assert_eq!(insecure.scheme(), "http");
    }

    #[test]
    fn same_site_by_public_suffix_list() {
        let target: Url = "https://api.example.org/x".parse().unwrap();
        assert!(is_same_site(Some(&origin_of("https://www.example.org/")), &target));
        assert!(!is_same_site(Some(&origin_of("https://other.example/")), &target));
        assert!(is_same_site(None, &target), "no initiator is same-site");

        // PSL edge the last-two-labels approximation got wrong: github.io is a
        // public suffix, so two users' subdomains are *different* sites.
        let pages: Url = "https://alice.github.io/x".parse().unwrap();
        assert!(
            !is_same_site(Some(&origin_of("https://bob.github.io/")), &pages),
            "distinct github.io subdomains are cross-site"
        );
        assert!(is_same_site(Some(&origin_of("https://alice.github.io/y")), &pages));
    }

    #[tokio::test]
    async fn preflight_allows_then_sends_actual_request() {
        let mut server = mockito::Server::new_async().await;
        let options = server
            .mock("OPTIONS", "/x")
            .match_header("access-control-request-method", "PUT")
            .with_status(204)
            .with_header("access-control-allow-origin", "http://app.example")
            .with_header("access-control-allow-methods", "PUT")
            .with_header("access-control-max-age", "600")
            .expect(1)
            .create_async()
            .await;
        let actual = server
            .mock("PUT", "/x")
            .with_status(200)
            .with_header("access-control-allow-origin", "http://app.example")
            .with_body("done")
            .expect(1)
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let mut req = Request::get(format!("{}/x", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        req.method = Method::Put;
        let res = fetch(req, &cx).await;

        options.assert_async().await;
        actual.assert_async().await;
        assert_eq!(res.response_type, ResponseType::Cors);
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"done");
    }

    #[tokio::test]
    async fn preflight_denial_blocks_without_sending_actual() {
        let mut server = mockito::Server::new_async().await;
        // OPTIONS allows the origin but not the method → denied.
        let options = server
            .mock("OPTIONS", "/x")
            .with_status(204)
            .with_header("access-control-allow-origin", "http://app.example")
            .with_header("access-control-allow-methods", "GET")
            .expect(1)
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let mut req = Request::get(format!("{}/x", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        req.method = Method::Put;
        let res = fetch(req, &cx).await;

        options.assert_async().await;
        assert!(res.is_network_error(), "preflight denial blocks the actual request");
    }

    #[tokio::test]
    async fn records_alt_svc_h3_advertisement() {
        #[derive(Clone, Default)]
        struct SpyAltSvc(Arc<Mutex<Option<u16>>>);
        impl crate::AltSvcStore for SpyAltSvc {
            fn h3_port(&self, _: &str) -> Option<u16> {
                *self.0.lock().unwrap()
            }
            fn record_h3(&self, _: &str, port: u16, _: u64) {
                *self.0.lock().unwrap() = Some(port);
            }
            fn clear(&self, _: &str) {
                *self.0.lock().unwrap() = None;
            }
        }

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_header("alt-svc", "h3=\":443\"; ma=3600")
            .with_body("ok")
            .create_async()
            .await;

        let spy = SpyAltSvc::default();
        let cx = FetchContext {
            cookies: Box::new(InMemoryCookieJar::new()),
            cache: std::sync::Arc::new(NoHttpCache),
            csp: Box::new(AllowAllCsp),
            hsts: Box::new(crate::InMemoryHsts::new()),
            preflight: Box::new(crate::InMemoryPreflightCache::new()),
            alt_svc: Box::new(spy.clone()),
        };
        let _ = fetch(Request::get(server.url().parse().unwrap()), &cx)
            .await
            .bytes()
            .await;

        assert_eq!(*spy.0.lock().unwrap(), Some(443), "h3 advertisement recorded");
    }

    #[tokio::test]
    async fn cors_response_headers_are_filtered() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/data")
            .with_status(200)
            .with_header("access-control-allow-origin", "http://app.example")
            .with_header("content-type", "application/json")
            .with_header("x-secret", "leak")
            .with_body("{}")
            .create_async()
            .await;

        let cx = FetchContext::permissive();
        let req = Request::get(format!("{}/data", server.url()).parse().unwrap())
            .with_origin(origin_of("http://app.example/"));
        let res = fetch(req, &cx).await;

        assert_eq!(res.response_type, ResponseType::Cors);
        let names: Vec<&str> = res.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"content-type"), "safelisted header kept");
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("x-secret")),
            "non-exposed header filtered out"
        );
    }
}
