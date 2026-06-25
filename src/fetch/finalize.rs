/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The back half of a hop: record response-derived state (cookies, HSTS,
//! Alt-Svc), then produce the terminal [`Response`] (tainting/filtering, body
//! decode, SRI, and the cache tee).

use std::time::SystemTime;

use bytes::BytesMut;
use url::Url;

use crate::cache;
use crate::cors;
use crate::decode::decode_stream;
use crate::response::{ResponseBody, ResponseType};
use crate::sri;
use crate::{altsvc, hsts};
use crate::{FetchContext, Request, Response};

use super::cache_phase::{over_cache_size_cap, strip_body_encoding_headers, CachingBody};
use super::transport::RawResponse;
use super::util::header_val;

/// Record the response-derived state a hop leaves behind: Set-Cookie (only when
/// the hop uses credentials), HSTS (https only), and Alt-Svc advertisements.
pub(super) fn record_response_metadata(
    cx: &FetchContext,
    current_url: &Url,
    use_credentials: bool,
    raw_headers: &[(String, String)],
) {
    // Record any Set-Cookie headers against the URL that produced them — only
    // when this hop uses credentials (an `omit` fetch stores nothing).
    if use_credentials {
        for (name, value) in raw_headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                cx.cookies.set_cookie(current_url, value);
            }
        }
    }

    // Record HSTS policy — only honored when delivered over https.
    if current_url.scheme() == "https" {
        if let Some(sts) = header_val(raw_headers, "strict-transport-security") {
            if let Some((max_age, include_subdomains)) = hsts::parse_sts(sts) {
                if let Some(host) = current_url.host_str() {
                    cx.hsts.record(host, max_age, include_subdomains);
                }
            }
        }
    }

    // Record Alt-Svc advertisements so future requests to this origin use h3.
    if let Some(host) = current_url.host_str() {
        if let Some(value) = header_val(raw_headers, "alt-svc") {
            match altsvc::parse_alt_svc(value) {
                altsvc::AltSvc::H3 { port, max_age } => cx.alt_svc.record_h3(host, port, max_age),
                altsvc::AltSvc::Clear => cx.alt_svc.clear(host),
                altsvc::AltSvc::None => {}
            }
        }
    }
}

/// Terminal hop handling: tainting/filtering, then decode the transport-agnostic
/// body. A cacheable GET 200 streams straight through while teeing into the cache;
/// an enforced integrity check buffers and verifies the whole body first.
pub(super) async fn finalize_response(
    request: &Request,
    cx: &FetchContext,
    raw: RawResponse,
    current_url: &Url,
    cache_key: Option<&str>,
    now: SystemTime,
    status: u16,
    url_list: Vec<Url>,
) -> Response {
    let content_encoding = header_val(&raw.headers, "content-encoding").map(str::to_owned);
    let response_type = match cors::evaluate(
        request.origin.as_ref(),
        current_url,
        request.mode,
        request.credentials,
        &raw.headers,
    ) {
        cors::Taint::Basic => ResponseType::Basic,
        cors::Taint::Cors => ResponseType::Cors,
        cors::Taint::Opaque => ResponseType::Opaque,
        cors::Taint::Blocked => return Response::network_error(),
    };
    // An opaque (cross-origin no-cors) response is filtered: status 0, no headers,
    // no exposed body — and never cached.
    if matches!(response_type, ResponseType::Opaque) {
        // An opaque body can't be read, so an enforced integrity check fails.
        if sri::is_enforced(&request.integrity) {
            return Response::network_error();
        }
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

    // Subresource Integrity: buffer the body and verify it against the metadata; a
    // mismatch is a network error. (Bypasses the streaming/cache paths — an
    // integrity request needs the whole body anyway.)
    if sri::is_enforced(&request.integrity) {
        let bytes = match ResponseBody::new(body).bytes().await {
            Ok(b) => b,
            Err(_) => return Response::network_error(),
        };
        if !sri::verify(&request.integrity, &bytes) {
            return Response::network_error();
        }
        return Response {
            status,
            headers,
            body: ResponseBody::from_bytes(bytes),
            url_list,
            response_type,
        };
    }

    // Cacheable GET 200 → stream the body straight through, teeing it into the
    // cache as it is read. The response resolves at its headers (no buffering up
    // front, so a slow or never-read body cannot stall it); the entry is stored
    // only once the body is read to completion. A body whose declared length
    // exceeds the cache cap is streamed without teeing (not worth a cache slot).
    if let Some(key) = cache_key {
        if cache::is_cacheable(status, &headers) && !over_cache_size_cap(&headers) {
            let mut stored_headers = headers;
            strip_body_encoding_headers(&mut stored_headers);
            let caching = CachingBody {
                inner: body,
                acc: BytesMut::new(),
                cache: cx.cache.clone(),
                key: key.to_owned(),
                status,
                headers: stored_headers.clone(),
                stored_at: now,
                poisoned: false,
                done: false,
            };
            return Response {
                status,
                headers: stored_headers,
                body: ResponseBody::new(Box::pin(caching)),
                url_list,
                response_type,
            };
        }
    }

    // Non-cacheable: stream straight through.
    Response {
        status,
        headers,
        body: ResponseBody::new(body),
        url_list,
        response_type,
    }
}
