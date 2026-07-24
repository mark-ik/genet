/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Fetch entry point.
//!
//! [`fetch`] is the WHATWG Fetch algorithm's outer loop. Each hop is composed
//! from phase modules:
//!
//! - [`cache_phase`] — the up-front HTTP-cache probe and the tee-into-cache body.
//! - [`request_headers`] — per-hop request-header assembly.
//! - [`transport`] — the h3 / h1-h2 send.
//! - [`redirect`] — redirect-mode gating and following.
//! - [`finalize`] — response-state recording and the terminal response.
//! - [`mixed_content`] / [`preflight`] — HSTS/mixed-content and CORS preflight.

mod cache_phase;
mod finalize;
mod mixed_content;
mod preflight;
mod redirect;
mod request_headers;
mod transport;
mod util;

#[cfg(test)]
mod tests;

use std::time::{Instant, SystemTime};

use crate::cache;
use crate::cors;
use crate::request::{CacheMode, Credentials, Method, RequestMode};
use crate::{FetchContext, Request, Response};

use cache_phase::{probe_cache, CacheProbe};
use finalize::{finalize_response, record_response_metadata};
use mixed_content::resolve_mixed_content;
use redirect::{process_redirect, RedirectStep};
use request_headers::assemble_request_headers;
use transport::send_request;

/// WHATWG Fetch's redirect cap.
const MAX_REDIRECTS: u32 = 20;

/// Default `User-Agent` sent when the request carries none.
pub(super) const USER_AGENT: &str = "Mozilla/5.0 (compatible; serval netfetcher)";

/// WHATWG [`fetch`]: runs [`fetch_inner`] and emits one per-fetch diagnostic on
/// the `netfetcher` target — `debug` on completion (status + elapsed) and `warn`
/// on a network error. This is the browsing pipeline's first leg, so every load's
/// fetches and their faults reach the host's diagnostics ring (the app installs
/// the subscriber; this crate only emits).
pub async fn fetch(request: Request, cx: &FetchContext) -> Response {
    let url = request.url.clone();
    let method = request.method.clone();
    let started = Instant::now();
    let response = fetch_inner(request, cx).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if response.is_network_error() {
        tracing::warn!(target: "netfetcher", url = %url, ?method, elapsed_ms, "fetch network error");
    } else {
        tracing::debug!(target: "netfetcher", url = %url, status = response.status, elapsed_ms, "fetch complete");
    }
    response
}

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
async fn fetch_inner(request: Request, cx: &FetchContext) -> Response {
    // A `data:` URL is decoded in place (no network): the body and media type come
    // straight from the URL. Always a basic response.
    if request.url.scheme() == "data" {
        return crate::data_url::process(&request.url, vec![request.url.clone()]);
    }
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
    // origin becomes opaque, so the `Origin` header flips to "null".
    let mut origin_tainted = false;
    // The active referrer policy; a redirect's `Referrer-Policy` can override it.
    let mut referrer_policy = request.referrer_policy;
    // The request's referrer, reduced to whatever a hop's policy permits (sticky).
    let mut referrer = request.referrer.clone();
    // Whether response tainting is "cors" — set once any hop is cross-origin in
    // cors mode, and sticky thereafter (so an unsafe request that redirects back
    // to the origin is still preflighted).
    let mut tainting_cors = false;

    // HTTP cache (RFC 9111 + request cache mode): only for GET, only when a real
    // cache is wired, and never for `no-store`.
    let now = SystemTime::now();
    let mode = request.cache;
    let cache_key =
        (cx.cache.enabled() && matches!(method, Method::Get) && mode != CacheMode::NoStore)
            .then(|| cache::cache_key("GET", &current_url));
    let mut revalidate = match probe_cache(cx, cache_key.as_deref(), mode, now, &url_list) {
        CacheProbe::Serve(response) => return response,
        CacheProbe::Proceed { revalidate } => revalidate,
    };

    loop {
        // CSP connect-src consultation (host-supplied policy).
        if !cx.csp.allows_connect(&current_url) {
            return Response::network_error();
        }

        // CORS preflight (per hop): a cors-tainted, non-simple request gets an
        // OPTIONS round-trip first (cached per Access-Control-Max-Age). Tainting is
        // sticky: once a hop is cross-origin in cors mode it stays "cors".
        let cross_origin = request
            .origin
            .as_ref()
            .is_some_and(|o| *o != current_url.origin());
        if cross_origin && matches!(request.mode, RequestMode::Cors) {
            tainting_cors = true;
        }
        if tainting_cors && cors::needs_preflight(&method, &base_headers) {
            let requested = cors::preflight_request_headers(&base_headers);
            let key =
                cors::preflight_key(request.origin.as_ref(), &current_url, &method, &requested);
            if !cx.preflight.check(&key) {
                match preflight::run_preflight(
                    &current_url,
                    request.origin.as_ref(),
                    &method,
                    &requested,
                    request.credentials,
                    request.referrer.as_ref(),
                    request.referrer_policy,
                )
                .await
                {
                    Some(max_age) => cx.preflight.store(&key, max_age),
                    None => return Response::network_error(),
                }
            }
        }

        // Whether this hop uses credentials (cookies/auth): always for `include`,
        // never for `omit`, and only same-origin for `same-origin`. Recomputed
        // per hop.
        let same_origin = request
            .origin
            .as_ref()
            .is_none_or(|o| *o == current_url.origin());
        let use_credentials = match request.credentials {
            Credentials::Include => true,
            Credentials::Omit => false,
            Credentials::SameOrigin => same_origin,
        };

        // Assemble the hop's request headers (also reduces `referrer` in place
        // under the active policy, so the reduction carries to later hops).
        let req_headers = assemble_request_headers(
            &request,
            cx,
            &current_url,
            &base_headers,
            &method,
            body.as_ref(),
            mode,
            use_credentials,
            url_list.len() == 1,
            &revalidate,
            origin_tainted,
            &mut referrer,
            referrer_policy,
        );

        // Transport: prefer h3 when this https origin advertised it (Alt-Svc).
        let try_h3 = current_url.scheme() == "https"
            && current_url.host_str().and_then(|h| cx.alt_svc.h3_port(h)).is_some();
        let raw =
            match send_request(&current_url, &method, &req_headers, body.as_ref(), try_h3).await {
                Some(raw) => raw,
                None => return Response::network_error(),
            };
        let status = raw.status;

        // Record the response-derived state this hop leaves behind.
        record_response_metadata(cx, &current_url, use_credentials, &raw.headers);

        // Redirect handling (advances the per-hop state in place when followed).
        match process_redirect(
            &request,
            &raw.headers,
            status,
            secure_context,
            cx,
            &mut method,
            &mut body,
            &mut base_headers,
            &mut current_url,
            &mut url_list,
            &mut redirects_remaining,
            &mut origin_tainted,
            &mut referrer_policy,
        ) {
            RedirectStep::Done(response) => return response,
            RedirectStep::Follow => continue,
            RedirectStep::Fallthrough => {}
        }

        // 304 Not Modified → serve (and refresh) the stored entry.
        if status == 304 {
            if let (Some(key), Some(entry)) = (cache_key.as_deref(), revalidate.take()) {
                let refreshed = cache::refresh(entry, &raw.headers, now);
                cx.cache.put(key, refreshed.clone());
                return cache::to_response(refreshed, url_list);
            }
        }

        // Terminal: tainting/filtering, then decode the transport-agnostic body.
        return finalize_response(
            &request,
            cx,
            raw,
            &current_url,
            cache_key.as_deref(),
            now,
            status,
            url_list,
        )
        .await;
    }
}
