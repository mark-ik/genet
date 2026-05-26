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

use url::{Origin, Url};

use crate::request::{Credentials, RequestMode};

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
}
