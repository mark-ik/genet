/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Referrer computation (W3C **Referrer Policy**).
//!
//! Given the request's referrer URL (the initiator document), the per-hop
//! target URL, and the active policy, produce the `Referer` header value — or
//! `None` for no referrer. Recomputed per redirect hop, because the target's
//! origin (and a `Referrer-Policy` response header on the redirect) can change
//! the answer.

use url::Url;

use crate::request::ReferrerPolicy;

/// The `Referer` header value for a request to `target`, derived from the
/// request's `referrer` URL under `policy`. `None` = send no `Referer`.
pub(crate) fn referrer_header(
    referrer: &Url,
    target: &Url,
    policy: ReferrerPolicy,
) -> Option<String> {
    use ReferrerPolicy::*;
    // An unset policy applies the default (strict-origin-when-cross-origin).
    let policy = match policy {
        Empty => StrictOriginWhenCrossOrigin,
        other => other,
    };
    if matches!(policy, NoReferrer) {
        return None;
    }
    let same_origin = referrer.origin() == target.origin();
    // A downgrade: a secure (potentially-trustworthy) referrer to a non-secure
    // target. (Scheme-based; the localhost special case is not modeled.)
    let downgrade = is_secure(referrer) && !is_secure(target);
    let full = || strip_referrer(referrer);
    let origin = || origin_referrer(referrer);
    match policy {
        UnsafeUrl => Some(full()),
        NoReferrerWhenDowngrade => (!downgrade).then(full),
        Origin => Some(origin()),
        SameOrigin => same_origin.then(full),
        StrictOrigin => (!downgrade).then(origin),
        OriginWhenCrossOrigin => Some(if same_origin { full() } else { origin() }),
        StrictOriginWhenCrossOrigin => {
            if same_origin {
                Some(full())
            } else if downgrade {
                None
            } else {
                Some(origin())
            }
        }
        // Handled above.
        Empty | NoReferrer => None,
    }
}

fn is_secure(u: &Url) -> bool {
    matches!(u.scheme(), "https" | "wss")
}

/// The "full" referrer: the URL with its fragment and any credentials removed
/// (W3C "strip url for use as a referrer", keep-path form).
fn strip_referrer(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

/// The origin-only referrer: the referrer's origin serialized with a trailing
/// `/` (e.g. `http://example.test:8000/`).
fn origin_referrer(u: &Url) -> String {
    format!("{}/", u.origin().ascii_serialization())
}

/// Parse one `Referrer-Policy` token (or init value) into a policy. An empty or
/// unknown token yields `None` (the caller keeps the current policy).
pub(crate) fn parse_policy(token: &str) -> Option<ReferrerPolicy> {
    use ReferrerPolicy::*;
    Some(match token.trim().to_ascii_lowercase().as_str() {
        "no-referrer" => NoReferrer,
        "no-referrer-when-downgrade" => NoReferrerWhenDowngrade,
        "same-origin" => SameOrigin,
        "origin" => Origin,
        "strict-origin" => StrictOrigin,
        "origin-when-cross-origin" => OriginWhenCrossOrigin,
        "strict-origin-when-cross-origin" => StrictOriginWhenCrossOrigin,
        "unsafe-url" => UnsafeUrl,
        _ => return None,
    })
}

/// Apply a `Referrer-Policy` response header (a comma-separated list): the last
/// valid, non-empty token wins; an absent/empty/unknown header leaves `current`
/// unchanged.
pub(crate) fn policy_from_header(current: ReferrerPolicy, header_value: &str) -> ReferrerPolicy {
    header_value
        .split(',')
        .rev()
        .find_map(parse_policy)
        .unwrap_or(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn default_policy_is_strict_origin_when_cross_origin() {
        let r = url("http://a.test/page?x=1");
        // same-origin → full URL
        assert_eq!(
            referrer_header(&r, &url("http://a.test/other"), ReferrerPolicy::Empty),
            Some("http://a.test/page?x=1".to_owned())
        );
        // cross-origin (no downgrade) → origin only
        assert_eq!(
            referrer_header(&r, &url("http://b.test/x"), ReferrerPolicy::Empty),
            Some("http://a.test/".to_owned())
        );
    }

    #[test]
    fn policy_variants() {
        let r = url("http://a.test/p");
        let cross = url("http://b.test/");
        assert_eq!(referrer_header(&r, &cross, ReferrerPolicy::NoReferrer), None);
        assert_eq!(
            referrer_header(&r, &cross, ReferrerPolicy::UnsafeUrl),
            Some("http://a.test/p".to_owned())
        );
        assert_eq!(
            referrer_header(&r, &cross, ReferrerPolicy::Origin),
            Some("http://a.test/".to_owned())
        );
        assert_eq!(referrer_header(&r, &cross, ReferrerPolicy::SameOrigin), None);
        assert_eq!(
            referrer_header(&r, &cross, ReferrerPolicy::OriginWhenCrossOrigin),
            Some("http://a.test/".to_owned())
        );
    }

    #[test]
    fn https_to_http_downgrade_drops_referrer() {
        let r = url("https://a.test/p");
        let http = url("http://a.test/x"); // same host, scheme downgrade
        assert_eq!(
            referrer_header(&r, &http, ReferrerPolicy::NoReferrerWhenDowngrade),
            None
        );
        assert_eq!(referrer_header(&r, &http, ReferrerPolicy::StrictOrigin), None);
    }

    #[test]
    fn response_header_last_valid_token_wins() {
        let p = policy_from_header(ReferrerPolicy::SameOrigin, "no-referrer, bogus, origin");
        assert_eq!(p, ReferrerPolicy::Origin);
        // empty / unknown keeps the current policy
        assert_eq!(
            policy_from_header(ReferrerPolicy::SameOrigin, ""),
            ReferrerPolicy::SameOrigin
        );
    }
}
