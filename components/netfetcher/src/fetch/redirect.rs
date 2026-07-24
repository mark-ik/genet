/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Redirect handling for one hop (WHATWG HTTP-redirect fetch): redirect-mode
//! gating, the CORS check on a 3xx, opaque-redirect filtering, and advancing the
//! per-hop state when a redirect is followed.

use bytes::Bytes;
use url::Url;

use crate::cors;
use crate::referrer;
use crate::request::{Method, RedirectMode, ReferrerPolicy, RequestMode};
use crate::response::{ResponseBody, ResponseType};
use crate::{FetchContext, Request, Response};

use super::mixed_content::resolve_mixed_content;
use super::util::header_val;

/// What the fetch loop should do after a hop's response is inspected for redirects.
pub(super) enum RedirectStep {
    /// Not a redirect (or a 3xx without a `Location`): fall through to terminal
    /// response handling.
    Fallthrough,
    /// Follow to the next hop (the per-hop state has been advanced): loop again.
    Follow,
    /// Terminal: return this response now.
    Done(Response),
}

/// A manual-redirect / opaque-redirect filtered response: status 0, no headers,
/// no body (WHATWG opaque-redirect).
fn opaque_redirect(url_list: &[Url]) -> Response {
    Response {
        status: 0,
        headers: Vec::new(),
        body: ResponseBody::empty(),
        url_list: url_list.to_vec(),
        response_type: ResponseType::OpaqueRedirect,
    }
}

/// Process a hop's response for redirects, advancing the per-hop state in place
/// when a redirect is followed. Returns [`RedirectStep`] to drive the loop.
#[allow(clippy::too_many_arguments)]
pub(super) fn process_redirect(
    request: &Request,
    raw_headers: &[(String, String)],
    status: u16,
    secure_context: bool,
    cx: &FetchContext,
    method: &mut Method,
    body: &mut Option<Bytes>,
    base_headers: &mut Vec<(String, String)>,
    current_url: &mut Url,
    url_list: &mut Vec<Url>,
    redirects_remaining: &mut u32,
    origin_tainted: &mut bool,
    referrer_policy: &mut ReferrerPolicy,
) -> RedirectStep {
    if !(300..400).contains(&status) {
        return RedirectStep::Fallthrough;
    }
    // Own the location so `raw_headers` is free for later reads.
    let location = header_val(raw_headers, "location").map(str::to_owned);
    // Error / manual redirect modes apply to any redirect status, even one
    // without a `Location` header (only Follow needs a target).
    match (request.redirect, &location) {
        (RedirectMode::Error, None) => return RedirectStep::Done(Response::network_error()),
        (RedirectMode::Manual, None) => return RedirectStep::Done(opaque_redirect(url_list)),
        _ => {}
    }
    // A 3xx without a Location is delivered as an ordinary response.
    let Some(location) = location else {
        return RedirectStep::Fallthrough;
    };

    // Gate the redirect response *before* the redirect-mode switch (WHATWG HTTP
    // fetch runs the CORS check on the actual response, a 3xx included, ahead of
    // redirect processing):
    let req_cross = *origin_tainted
        || request.origin.as_ref().is_some_and(|o| *o != current_url.origin());
    // A cors-tainted request whose redirect fails the CORS check is a network
    // error even under manual/error redirect modes.
    if req_cross
        && matches!(request.mode, RequestMode::Cors)
        && matches!(
            cors::evaluate(
                request.origin.as_ref(),
                current_url,
                request.mode,
                request.credentials,
                raw_headers,
            ),
            cors::Taint::Blocked
        )
    {
        return RedirectStep::Done(Response::network_error());
    }
    // A no-cors cross-origin request may not observe a redirect with a non-follow
    // redirect mode (it would leak the cross-origin hop).
    if req_cross
        && matches!(request.mode, RequestMode::NoCors)
        && !matches!(request.redirect, RedirectMode::Follow)
    {
        return RedirectStep::Done(Response::network_error());
    }

    match request.redirect {
        RedirectMode::Error => RedirectStep::Done(Response::network_error()),
        RedirectMode::Manual => RedirectStep::Done(opaque_redirect(url_list)),
        RedirectMode::Follow => {
            if *redirects_remaining == 0 {
                return RedirectStep::Done(Response::network_error());
            }
            let Ok(next) = current_url.join(&location) else {
                return RedirectStep::Done(Response::network_error());
            };
            // A redirect to a URL embedding credentials (user:password@host) is a
            // network error for any non-navigate request (WHATWG HTTP-redirect fetch).
            if (!next.username().is_empty() || next.password().is_some())
                && !matches!(request.mode, RequestMode::Navigate)
            {
                return RedirectStep::Done(Response::network_error());
            }
            *redirects_remaining -= 1;
            // A `Referrer-Policy` on the redirect response governs the `Referer`
            // header for subsequent hops.
            if let Some(rp) = header_val(raw_headers, "referrer-policy") {
                *referrer_policy = referrer::policy_from_header(*referrer_policy, rp);
            }
            // Taint the origin to opaque when this redirect is cross-origin *and*
            // the current URL was already foreign to the initiator (the second
            // cross-origin hop): the `Origin` header becomes "null" from here on.
            let crosses = current_url.origin() != next.origin();
            let already_foreign = *origin_tainted
                || request.origin.as_ref().is_some_and(|o| *o != current_url.origin());
            if crosses && already_foreign {
                *origin_tainted = true;
            }
            let prev_method = method.clone();
            *method = redirect_method(status, method.clone(), body);
            // A method-changing redirect (301/302 POST->GET, 303 ->GET) drops the
            // body, so the request-body headers go too — per WHATWG HTTP-redirect
            // fetch (regardless of whether a body was present).
            if *method != prev_method {
                base_headers.retain(|(k, _)| {
                    !matches!(
                        k.to_ascii_lowercase().as_str(),
                        "content-type"
                            | "content-length"
                            | "content-encoding"
                            | "content-language"
                            | "content-location"
                    )
                });
            }
            *current_url = next;
            if resolve_mixed_content(current_url, request.destination, secure_context, cx) {
                return RedirectStep::Done(Response::network_error());
            }
            url_list.push(current_url.clone());
            RedirectStep::Follow
        }
    }
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
