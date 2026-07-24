/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The transport layer: send one request over h3 (when advertised) or h1/h2,
//! normalizing the result to a transport-agnostic [`RawResponse`].

use std::io;

use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, Full};
use url::Url;

use crate::client::shared_client;
use crate::request::Method;
use crate::response::BodyStream;

/// Normalized transport response: status + headers + a raw (undecoded) body
/// stream, produced by either the h1/h2 or the h3 path so the loop's back half is
/// transport-agnostic.
pub(super) struct RawResponse {
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: BodyStream,
}

/// Send one request over h3 (if `try_h3` and available) or h1/h2, normalizing the
/// result to a [`RawResponse`]. An h3 failure falls back to h1/h2.
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
pub(super) async fn send_request(
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

pub(super) fn http_method(method: &Method) -> http::Method {
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

pub(super) fn collect_headers(map: &http::HeaderMap) -> Vec<(String, String)> {
    map.iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_owned(), s.to_owned())))
        .collect()
}
