/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cross-origin policy: response tainting + CORS gating (WHATWG Fetch).
//!
//! Given the request's initiator origin, mode, and credentials, plus the
//! response headers, decide the [`crate::ResponseType`] (`Basic` / `Cors` /
//! `Opaque`) — or that the response must be blocked as a network error.
//!
//! **Slice 1 (increment 3):** same-origin and the *simple-request* CORS form.
//! Deferred: preflight (`OPTIONS` for non-simple requests), CORS response-header
//! *filtering* (the `Cors` tainting type is set, but readable-header restriction
//! per `Access-Control-Expose-Headers` is not yet applied), and `SameSite`
//! enforcement.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use url::{Origin, Url};

use crate::request::{Credentials, Method, RequestMode};

/// The cross-origin verdict for a response.
pub(crate) enum Taint {
    Basic,
    Cors,
    Opaque,
    /// CORS check failed → the fetch is a network error.
    Blocked,
}

/// Evaluate tainting for a response delivered from `target`.
pub(crate) fn evaluate(
    origin: Option<&Origin>,
    target: &Url,
    mode: RequestMode,
    credentials: Credentials,
    response_headers: &[(String, String)],
) -> Taint {
    // No initiator (top-level fetch) is treated as same-origin: no cross-checks.
    let same_origin = match origin {
        None => true,
        Some(o) => *o == target.origin(),
    };
    if same_origin {
        return Taint::Basic;
    }

    match mode {
        // A same-origin-mode request that reached a cross-origin URL is a violation.
        RequestMode::SameOrigin => Taint::Blocked,
        // Top-level navigations may be cross-origin.
        RequestMode::Navigate => Taint::Basic,
        RequestMode::NoCors => Taint::Opaque,
        RequestMode::Cors => {
            if cors_check(origin, credentials, response_headers) {
                Taint::Cors
            } else {
                Taint::Blocked
            }
        }
    }
}

/// The CORS resource-sharing check (simple-request form): consult
/// `Access-Control-Allow-Origin` (and, with credentials, `-Allow-Credentials`).
fn cors_check(
    origin: Option<&Origin>,
    credentials: Credentials,
    headers: &[(String, String)],
) -> bool {
    let Some(acao) = header(headers, "access-control-allow-origin") else {
        return false;
    };

    if matches!(credentials, Credentials::Include) {
        // With credentials, `*` is forbidden and `Allow-Credentials: true` required.
        if acao == "*" {
            return false;
        }
        let allow_creds = header(headers, "access-control-allow-credentials")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        allow_creds && origin_matches(origin, acao)
    } else {
        acao == "*" || origin_matches(origin, acao)
    }
}

fn origin_matches(origin: Option<&Origin>, acao: &str) -> bool {
    origin.is_some_and(|o| o.ascii_serialization() == acao)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------
// Preflight (CORS-preflight fetch) + response-header filtering.
// ---------------------------------------------------------------------------

const SAFELISTED_CONTENT_TYPES: [&str; 3] = [
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/plain",
];

fn is_safelisted_request_header(name_lc: &str, value: &str) -> bool {
    match name_lc {
        "accept" | "accept-language" | "content-language" => true,
        "content-type" => {
            let essence = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            SAFELISTED_CONTENT_TYPES.contains(&essence.as_str())
        }
        _ => false,
    }
}

/// Does this cross-origin CORS request need a preflight? (Caller restricts to
/// cross-origin + cors mode.) Non-simple = a non-`GET`/`HEAD`/`POST` method or any
/// non-safelisted request header.
pub(crate) fn needs_preflight(method: Method, headers: &[(String, String)]) -> bool {
    if !matches!(method, Method::Get | Method::Head | Method::Post) {
        return true;
    }
    headers
        .iter()
        .any(|(name, value)| !is_safelisted_request_header(&name.to_ascii_lowercase(), value))
}

/// Non-safelisted request-header names (lowercased, sorted, deduped) — the
/// `Access-Control-Request-Headers` list for the preflight.
pub(crate) fn preflight_request_headers(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers
        .iter()
        .filter(|(name, value)| !is_safelisted_request_header(&name.to_ascii_lowercase(), value))
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Verify a preflight response. `Some(max_age)` when the actual request is
/// allowed (origin + method + every requested header), `None` when denied.
pub(crate) fn preflight_verdict(
    origin: Option<&Origin>,
    credentials: Credentials,
    method: Method,
    requested_headers: &[String],
    response_headers: &[(String, String)],
) -> Option<u64> {
    if !cors_check(origin, credentials, response_headers) {
        return None;
    }
    let allow_methods = header(response_headers, "access-control-allow-methods");
    if !list_allows(allow_methods, method_name(method), credentials) {
        return None;
    }
    let allow_headers = header(response_headers, "access-control-allow-headers");
    if !requested_headers
        .iter()
        .all(|h| list_allows(allow_headers, h, credentials))
    {
        return None;
    }
    Some(
        header(response_headers, "access-control-max-age")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0),
    )
}

/// Whether a comma-separated CORS allow-list permits `item`. `*` matches anything
/// only when the request is not credentialed.
fn list_allows(list: Option<&str>, item: &str, credentials: Credentials) -> bool {
    let Some(list) = list else {
        return false;
    };
    let credentialed = matches!(credentials, Credentials::Include);
    list.split(',').any(|entry| {
        let e = entry.trim();
        (e == "*" && !credentialed) || e.eq_ignore_ascii_case(item)
    })
}

fn method_name(method: Method) -> &'static str {
    match method {
        Method::Get => "GET",
        Method::Head => "HEAD",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Patch => "PATCH",
        Method::Options => "OPTIONS",
    }
}

/// Filter a `Cors`-tainted response's headers to the CORS-safelisted response
/// headers plus any named in `Access-Control-Expose-Headers` (or all on `*`).
pub(crate) fn filter_cors_response_headers(
    headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    const SAFELISTED: [&str; 7] = [
        "cache-control",
        "content-language",
        "content-length",
        "content-type",
        "expires",
        "last-modified",
        "pragma",
    ];
    let expose: Vec<String> = header(&headers, "access-control-expose-headers")
        .map(|v| v.split(',').map(|s| s.trim().to_ascii_lowercase()).collect())
        .unwrap_or_default();
    let expose_all = expose.iter().any(|e| e == "*");
    headers
        .into_iter()
        .filter(|(name, _)| {
            let n = name.to_ascii_lowercase();
            expose_all || SAFELISTED.contains(&n.as_str()) || expose.contains(&n)
        })
        .collect()
}

/// Cache key for a preflight grant: initiator origin + target origin + method +
/// the requested non-safelisted headers.
pub(crate) fn preflight_key(
    origin: Option<&Origin>,
    target: &Url,
    method: Method,
    requested_headers: &[String],
) -> String {
    let o = origin.map(Origin::ascii_serialization).unwrap_or_default();
    format!(
        "{o}|{}|{}|{}",
        target.origin().ascii_serialization(),
        method_name(method),
        requested_headers.join(","),
    )
}

/// Storage seam for preflight results (RFC: `Access-Control-Max-Age`). Lets the
/// engine skip the OPTIONS round-trip while a grant is still valid.
pub trait PreflightCache: Send + Sync {
    /// Is there a still-valid cached grant for `key`?
    fn check(&self, key: &str) -> bool;
    /// Cache a grant for `max_age_secs` (0 = don't cache).
    fn store(&self, key: &str, max_age_secs: u64);
}

/// Process-local in-memory preflight cache.
#[derive(Default)]
pub struct InMemoryPreflightCache {
    entries: Mutex<HashMap<String, SystemTime>>,
}

impl InMemoryPreflightCache {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PreflightCache for InMemoryPreflightCache {
    fn check(&self, key: &str) -> bool {
        self.entries
            .lock()
            .ok()
            .and_then(|m| m.get(key).copied())
            .is_some_and(|expiry| expiry > SystemTime::now())
    }
    fn store(&self, key: &str, max_age_secs: u64) {
        if max_age_secs == 0 {
            return;
        }
        if let Ok(mut m) = self.entries.lock() {
            m.insert(key.to_owned(), SystemTime::now() + Duration::from_secs(max_age_secs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin_of(s: &str) -> Origin {
        s.parse::<Url>().unwrap().origin()
    }

    fn hdr(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn is(t: Taint, want: &str) -> bool {
        matches!(
            (t, want),
            (Taint::Basic, "basic")
                | (Taint::Cors, "cors")
                | (Taint::Opaque, "opaque")
                | (Taint::Blocked, "blocked")
        )
    }

    #[test]
    fn same_origin_is_basic() {
        let target: Url = "https://example.org/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::SameOrigin, &[]),
            "basic"
        ));
    }

    #[test]
    fn no_initiator_is_basic() {
        let target: Url = "https://example.org/x".parse().unwrap();
        assert!(is(
            evaluate(None, &target, RequestMode::Cors, Credentials::SameOrigin, &[]),
            "basic"
        ));
    }

    #[test]
    fn cross_origin_no_cors_is_opaque() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::NoCors, Credentials::SameOrigin, &[]),
            "opaque"
        ));
    }

    #[test]
    fn cross_origin_same_origin_mode_is_blocked() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::SameOrigin, Credentials::SameOrigin, &[]),
            "blocked"
        ));
    }

    #[test]
    fn cors_wildcard_allows() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        let h = hdr(&[("access-control-allow-origin", "*")]);
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::SameOrigin, &h),
            "cors"
        ));
    }

    #[test]
    fn cors_echoed_origin_allows() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        let h = hdr(&[("access-control-allow-origin", "https://example.org")]);
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::SameOrigin, &h),
            "cors"
        ));
    }

    #[test]
    fn cors_missing_header_blocks() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::SameOrigin, &[]),
            "blocked"
        ));
    }

    #[test]
    fn cors_credentialed_wildcard_blocks() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        let h = hdr(&[("access-control-allow-origin", "*")]);
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::Include, &h),
            "blocked"
        ));
    }

    #[test]
    fn cors_credentialed_echo_with_allow_creds_allows() {
        let target: Url = "https://other.example/x".parse().unwrap();
        let o = origin_of("https://example.org/");
        let h = hdr(&[
            ("access-control-allow-origin", "https://example.org"),
            ("access-control-allow-credentials", "true"),
        ]);
        assert!(is(
            evaluate(Some(&o), &target, RequestMode::Cors, Credentials::Include, &h),
            "cors"
        ));
    }

    #[test]
    fn preflight_triggers_on_nonsimple() {
        assert!(!needs_preflight(Method::Get, &[]));
        assert!(!needs_preflight(Method::Post, &hdr(&[("content-type", "text/plain")])));
        assert!(needs_preflight(Method::Put, &[]));
        assert!(needs_preflight(Method::Post, &hdr(&[("x-custom", "1")])));
        assert!(needs_preflight(
            Method::Post,
            &hdr(&[("content-type", "application/json")])
        ));
    }

    #[test]
    fn preflight_verdict_checks_method_and_headers() {
        let o = origin_of("https://app.example/");
        let ok = hdr(&[
            ("access-control-allow-origin", "https://app.example"),
            ("access-control-allow-methods", "PUT, DELETE"),
            ("access-control-allow-headers", "x-custom"),
            ("access-control-max-age", "600"),
        ]);
        assert_eq!(
            preflight_verdict(
                Some(&o),
                Credentials::SameOrigin,
                Method::Put,
                &["x-custom".to_string()],
                &ok
            ),
            Some(600)
        );
        // Method not in Allow-Methods.
        assert_eq!(
            preflight_verdict(Some(&o), Credentials::SameOrigin, Method::Patch, &[], &ok),
            None
        );
        // Requested header not in Allow-Headers.
        let no_hdr = hdr(&[
            ("access-control-allow-origin", "https://app.example"),
            ("access-control-allow-methods", "PUT"),
        ]);
        assert_eq!(
            preflight_verdict(
                Some(&o),
                Credentials::SameOrigin,
                Method::Put,
                &["x-custom".to_string()],
                &no_hdr
            ),
            None
        );
    }

    #[test]
    fn cors_response_header_filtering() {
        let headers = hdr(&[
            ("content-type", "application/json"),
            ("x-secret", "leak"),
            ("access-control-expose-headers", "x-public"),
            ("x-public", "ok"),
        ]);
        let names: Vec<String> = filter_cors_response_headers(headers)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert!(names.iter().any(|n| n == "content-type"), "safelisted kept");
        assert!(names.iter().any(|n| n == "x-public"), "exposed kept");
        assert!(!names.iter().any(|n| n == "x-secret"), "non-exposed hidden");
    }
}
