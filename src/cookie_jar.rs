/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! An in-memory RFC 6265bis cookie jar.
//!
//! Parses `Set-Cookie` via the `cookie` crate, then applies the standard
//! client-side storage + retrieval rules: domain-match (host-only vs `Domain`),
//! path-match (with the request's default-path), `Secure` (https-only), and
//! expiry (`Max-Age` over `Expires`). Cookies replace any prior entry with the
//! same (name, domain, path); `cookies_for` returns matches sorted longest-path
//! first, then oldest-first (the spec's serialization order).
//!
//! **Increment-2 boundaries (deliberate):** `SameSite` is *stored* but not
//! *enforced* — cross-site enforcement needs the site-for-cookies / initiator
//! context that arrives with CORS in increment 3. The public-suffix guard
//! (rejecting `Domain=com`-style super-cookies) and the `__Secure-`/`__Host-`
//! prefix rules are also deferred.

use std::net::IpAddr;
use std::sync::Mutex;

use cookie::{Cookie, SameSite};
use time::OffsetDateTime;
use url::Url;

use crate::context::CookieStore;

#[derive(Clone)]
struct StoredCookie {
    name: String,
    value: String,
    /// Lowercased; the host for a host-only cookie, else the `Domain` value.
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    #[allow(dead_code)] // recorded for increment-3 SameSite enforcement
    same_site: Option<SameSite>,
    /// `None` = session cookie (kept for the jar's lifetime).
    expiry: Option<OffsetDateTime>,
    created: OffsetDateTime,
}

/// In-memory cookie jar. Cheap to share; all access is behind a `Mutex`.
#[derive(Default)]
pub struct InMemoryCookieJar {
    cookies: Mutex<Vec<StoredCookie>>,
}

impl InMemoryCookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live (unexpired) stored cookies — inspection aid for tests.
    pub fn len(&self) -> usize {
        let now = OffsetDateTime::now_utc();
        self.cookies
            .lock()
            .map(|c| c.iter().filter(|c| !expired(c, now)).count())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CookieStore for InMemoryCookieJar {
    fn cookies_for(&self, url: &Url) -> Vec<String> {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let req_path = url.path();
        let secure_ctx = url.scheme() == "https";
        let now = OffsetDateTime::now_utc();

        let Ok(mut jar) = self.cookies.lock() else {
            return Vec::new();
        };
        jar.retain(|c| !expired(c, now));

        let mut hits: Vec<&StoredCookie> = jar
            .iter()
            .filter(|c| {
                let domain_ok = if c.host_only {
                    host == c.domain
                } else {
                    domain_matches(&host, &c.domain)
                };
                domain_ok && path_matches(req_path, &c.path) && (!c.secure || secure_ctx)
            })
            .collect();

        // Longer paths first; ties broken by creation order (oldest first).
        hits.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.created.cmp(&b.created)));
        hits.iter().map(|c| format!("{}={}", c.name, c.value)).collect()
    }

    fn set_cookie(&self, url: &Url, set_cookie_header: &str) {
        let Ok(parsed) = Cookie::parse(set_cookie_header.to_owned()) else {
            return;
        };
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();

        let (domain, host_only) = match parsed.domain() {
            Some(d) => {
                let d = d.trim_start_matches('.').to_ascii_lowercase();
                // Reject a Domain the request host doesn't domain-match.
                if !domain_matches(&host, &d) {
                    return;
                }
                (d, false)
            }
            None => (host, true),
        };
        let path = parsed
            .path()
            .map(str::to_owned)
            .unwrap_or_else(|| default_path(url));
        let now = OffsetDateTime::now_utc();
        let expiry = compute_expiry(&parsed, now);

        let stored = StoredCookie {
            name: parsed.name().to_owned(),
            value: parsed.value().to_owned(),
            domain,
            host_only,
            path,
            secure: parsed.secure().unwrap_or(false),
            same_site: parsed.same_site(),
            expiry,
            created: now,
        };

        let Ok(mut jar) = self.cookies.lock() else {
            return;
        };
        // A new cookie replaces any prior one with the same identity tuple.
        jar.retain(|c| {
            !(c.name == stored.name && c.domain == stored.domain && c.path == stored.path)
        });
        // An already-expired Set-Cookie is a deletion (replace step above) — don't re-add.
        if expiry.is_some_and(|e| e <= now) {
            return;
        }
        jar.push(stored);
    }
}

fn expired(c: &StoredCookie, now: OffsetDateTime) -> bool {
    c.expiry.is_some_and(|e| e <= now)
}

/// Max-Age takes precedence over Expires (RFC 6265bis §5.3).
fn compute_expiry(c: &Cookie, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if let Some(max_age) = c.max_age() {
        return Some(now.saturating_add(max_age));
    }
    c.expires_datetime()
}

/// RFC 6265bis domain-match: equal, or `host` is a sub-domain of `domain` and
/// `host` is not an IP literal.
fn domain_matches(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    if host.len() <= domain.len() || !host.ends_with(domain) {
        return false;
    }
    let boundary = host.len() - domain.len() - 1;
    host.as_bytes()[boundary] == b'.' && host.parse::<IpAddr>().is_err()
}

/// RFC 6265bis path-match.
fn path_matches(req_path: &str, cookie_path: &str) -> bool {
    if req_path == cookie_path {
        return true;
    }
    if !req_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || req_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

/// RFC 6265bis default-path: the request path up to (not including) the last `/`,
/// or `/` if there's no non-leading slash.
fn default_path(url: &Url) -> String {
    let path = url.path();
    if !path.starts_with('/') {
        return "/".to_owned();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(i) => path[..i].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    #[test]
    fn stores_and_returns_a_host_only_cookie() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/"), "id=abc");
        assert_eq!(jar.cookies_for(&url("https://example.org/")), vec!["id=abc"]);
        // Host-only must not leak to a sub-domain.
        assert!(jar.cookies_for(&url("https://sub.example.org/")).is_empty());
    }

    #[test]
    fn domain_cookie_reaches_subdomains() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/"), "id=abc; Domain=example.org");
        assert_eq!(jar.cookies_for(&url("https://api.example.org/")), vec!["id=abc"]);
    }

    #[test]
    fn secure_cookie_not_sent_over_http() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/"), "id=abc; Secure");
        assert!(jar.cookies_for(&url("http://example.org/")).is_empty());
        assert_eq!(jar.cookies_for(&url("https://example.org/")), vec!["id=abc"]);
    }

    #[test]
    fn path_scopes_the_cookie() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/app/"), "id=abc; Path=/app");
        assert_eq!(jar.cookies_for(&url("https://example.org/app/x")), vec!["id=abc"]);
        assert!(jar.cookies_for(&url("https://example.org/other")).is_empty());
    }

    #[test]
    fn max_age_zero_deletes() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/"), "id=abc");
        assert_eq!(jar.len(), 1);
        jar.set_cookie(&url("https://example.org/"), "id=abc; Max-Age=0");
        assert!(jar.cookies_for(&url("https://example.org/")).is_empty());
        assert_eq!(jar.len(), 0);
    }

    #[test]
    fn longer_paths_sort_first() {
        let jar = InMemoryCookieJar::new();
        jar.set_cookie(&url("https://example.org/"), "a=1; Path=/");
        jar.set_cookie(&url("https://example.org/app/"), "b=2; Path=/app");
        assert_eq!(
            jar.cookies_for(&url("https://example.org/app/page")),
            vec!["b=2", "a=1"],
        );
    }
}
