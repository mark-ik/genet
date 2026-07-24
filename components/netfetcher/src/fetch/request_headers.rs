/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Per-hop request-header assembly (WHATWG HTTP-network-or-cache fetch):
//! base headers + cookies + conditional/cache-mode + Range + Origin + Referer +
//! User-Agent + a `Content-Length: 0` for bodyless POST/PUT.

use bytes::Bytes;
use url::Url;

use crate::cache::{self, StoredResponse};
use crate::referrer;
use crate::request::{CacheMode, Method, ReferrerPolicy, RequestMode};
use crate::{FetchContext, Request, SameSiteContext};

use super::mixed_content::is_same_site;
use super::util::header_val;
use super::USER_AGENT;

/// Build the header list for one hop. `base_headers` is the loop's current base
/// (a method-changing redirect drops the body headers from it); `first_hop` gates
/// the conditional/cache-mode headers to the initial request. The referrer is
/// reduced in place under the active policy so the reduction carries to later hops.
#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_request_headers(
    request: &Request,
    cx: &FetchContext,
    current_url: &Url,
    base_headers: &[(String, String)],
    method: &Method,
    body: Option<&Bytes>,
    mode: CacheMode,
    use_credentials: bool,
    first_hop: bool,
    revalidate: &Option<StoredResponse>,
    origin_tainted: bool,
    referrer: &mut Option<Url>,
    referrer_policy: ReferrerPolicy,
) -> Vec<(String, String)> {
    // Assemble request headers: base + cookies + (initial-only) conditional.
    let mut req_headers = base_headers.to_vec();
    let cookies = if use_credentials {
        cx.cookies.cookies_for(
            current_url,
            SameSiteContext {
                same_site: is_same_site(request.origin.as_ref(), current_url),
                top_level_navigation: matches!(request.mode, RequestMode::Navigate),
            },
        )
    } else {
        Vec::new()
    };
    if !cookies.is_empty() {
        req_headers.push(("cookie".to_owned(), cookies.join("; ")));
    }
    if first_hop {
        if let Some(entry) = revalidate {
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

    // WHATWG HTTP-network-or-cache fetch: if the request's header list contains
    // `Range`, set `Accept-Encoding: identity` so the server does not transfer-
    // compress the response (which would invalidate the requested byte offsets).
    if header_val(&req_headers, "range").is_some() {
        req_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("accept-encoding"));
        req_headers.push(("accept-encoding".to_owned(), "identity".to_owned()));
    }

    // Append the `Origin` header (WHATWG "append a request Origin header"):
    // for any non-GET/HEAD method, or a cross-origin request. After a
    // cross-origin redirect taints the origin, the value becomes "null".
    if let Some(origin) = request.origin.as_ref() {
        let cur_cross = origin_tainted || *origin != current_url.origin();
        let send_origin = !matches!(method, Method::Get | Method::Head) || cur_cross;
        if send_origin && header_val(&req_headers, "origin").is_none() {
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
        let value = referrer::referrer_header(r, current_url, referrer_policy);
        *referrer = value.as_deref().and_then(|v| Url::parse(v).ok());
        if let Some(value) = value {
            if header_val(&req_headers, "referer").is_none() {
                req_headers.push(("referer".to_owned(), value));
            }
        }
    }

    // Default User-Agent (a request may override it).
    if header_val(&req_headers, "user-agent").is_none() {
        req_headers.push(("user-agent".to_owned(), USER_AGENT.to_owned()));
    }
    // A bodyless POST/PUT still carries Content-Length: 0 (WHATWG
    // HTTP-network-or-cache fetch).
    if body.is_none()
        && matches!(method, Method::Post | Method::Put)
        && header_val(&req_headers, "content-length").is_none()
    {
        req_headers.push(("content-length".to_owned(), "0".to_owned()));
    }

    req_headers
}
