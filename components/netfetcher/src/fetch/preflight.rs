/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CORS preflight: send an `OPTIONS` and verify it grants the actual request.

use bytes::Bytes;
use http_body_util::Full;
use url::Url;

use crate::client::shared_client;
use crate::cors;
use crate::referrer;
use crate::request::{Credentials, Method, ReferrerPolicy};

use super::transport::{collect_headers, http_method};
use super::USER_AGENT;

/// Send a CORS preflight `OPTIONS` and verify it. `Some(max_age)` if the actual
/// request is permitted, `None` if denied (or the preflight itself failed).
pub(super) async fn run_preflight(
    target: &Url,
    origin: Option<&url::Origin>,
    method: &Method,
    requested_headers: &[String],
    credentials: Credentials,
    referrer: Option<&Url>,
    referrer_policy: ReferrerPolicy,
) -> Option<u64> {
    let uri = http::Uri::try_from(target.as_str()).ok()?;
    let mut builder = http::Request::builder()
        .method(http::Method::OPTIONS)
        .uri(uri)
        .header("accept", "*/*")
        .header("user-agent", USER_AGENT)
        .header("access-control-request-method", http_method(method).as_str());
    if let Some(o) = origin {
        builder = builder.header(http::header::ORIGIN, o.ascii_serialization());
    }
    // The preflight carries the request's referrer under its policy (same as the
    // actual request would for this target).
    if let Some(r) = referrer {
        if let Some(value) = referrer::referrer_header(r, target, referrer_policy) {
            builder = builder.header(http::header::REFERER, value);
        }
    }
    if !requested_headers.is_empty() {
        builder = builder.header("access-control-request-headers", requested_headers.join(","));
    }
    let req = builder.body(Full::new(Bytes::new())).ok()?;
    let resp = shared_client().request(req).await.ok()?;
    // The preflight response must have an ok (2xx) status; a redirect or error
    // status is a network error (WHATWG CORS-preflight fetch). The client does not
    // follow redirects here, so a 3xx is delivered as-is and rejected.
    if !resp.status().is_success() {
        return None;
    }
    let headers = collect_headers(resp.headers());
    cors::preflight_verdict(origin, credentials, method, requested_headers, &headers)
}
