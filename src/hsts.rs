/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HTTP Strict Transport Security (RFC 6797).
//!
//! A storage seam (mirroring the cookie/cache pattern) plus the small policy:
//! record `Strict-Transport-Security` headers seen over https, and upgrade
//! subsequent `http` requests to known hosts to `https` *before* they hit the
//! network. Durable backing is the host's job (Mere); the in-memory default here
//! is process-local.
//!
//! **Scope:** dynamic HSTS only. The bundled **preload list** (a large static set
//! of always-HSTS hosts) is deferred — it's a sizable data dependency better added
//! deliberately (and it's a pure addition: another `is_secure` source).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use url::Url;

/// Storage seam for HSTS host policy.
pub trait HstsStore: Send + Sync {
    /// Should an `http` request to `host` be upgraded to `https`?
    fn is_secure(&self, host: &str) -> bool;
    /// Record a `Strict-Transport-Security` policy for `host` (seen over https).
    /// `max_age_secs` of 0 clears any existing policy.
    fn record(&self, host: &str, max_age_secs: u64, include_subdomains: bool);
}

struct Entry {
    expiry: SystemTime,
    include_subdomains: bool,
}

/// Process-local in-memory HSTS store. Empty until a `Strict-Transport-Security`
/// header is seen, so it's a no-op upgrade source by default.
#[derive(Default)]
pub struct InMemoryHsts {
    hosts: Mutex<HashMap<String, Entry>>,
}

impl InMemoryHsts {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HstsStore for InMemoryHsts {
    fn is_secure(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let now = SystemTime::now();
        let Ok(map) = self.hosts.lock() else {
            return false;
        };
        map.iter().any(|(domain, e)| {
            e.expiry > now
                && (host == *domain || (e.include_subdomains && is_subdomain(&host, domain)))
        })
    }

    fn record(&self, host: &str, max_age_secs: u64, include_subdomains: bool) {
        let host = host.to_ascii_lowercase();
        let Ok(mut map) = self.hosts.lock() else {
            return;
        };
        if max_age_secs == 0 {
            map.remove(&host);
            return;
        }
        map.insert(
            host,
            Entry {
                expiry: SystemTime::now() + Duration::from_secs(max_age_secs),
                include_subdomains,
            },
        );
    }
}

/// Whether `url` should be HSTS-upgraded under `store` (an `http` URL whose host
/// is a known secure host).
pub(crate) fn should_upgrade(url: &Url, store: &dyn HstsStore) -> bool {
    url.scheme() == "http" && url.host_str().is_some_and(|h| store.is_secure(h))
}

/// Parse a `Strict-Transport-Security` header value into `(max_age, includeSubDomains)`.
/// `None` when `max-age` is absent (the header is then ignored).
pub(crate) fn parse_sts(value: &str) -> Option<(u64, bool)> {
    let mut max_age = None;
    let mut include_subdomains = false;
    for directive in value.split(';') {
        let lower = directive.trim().to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("max-age=") {
            max_age = v.trim().trim_matches('"').parse::<u64>().ok();
        } else if lower == "includesubdomains" {
            include_subdomains = true;
        }
    }
    max_age.map(|ma| (ma, include_subdomains))
}

fn is_subdomain(host: &str, domain: &str) -> bool {
    host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sts_header() {
        assert_eq!(parse_sts("max-age=31536000"), Some((31536000, false)));
        assert_eq!(
            parse_sts("max-age=600; includeSubDomains"),
            Some((600, true))
        );
        assert_eq!(parse_sts("includeSubDomains"), None); // no max-age → ignored
    }

    #[test]
    fn records_and_upgrades_known_host() {
        let hsts = InMemoryHsts::new();
        hsts.record("example.org", 3600, false);
        assert!(should_upgrade(&"http://example.org/x".parse().unwrap(), &hsts));
        // Different host, and an already-https URL, are untouched.
        assert!(!should_upgrade(&"http://other.example/".parse().unwrap(), &hsts));
        assert!(!should_upgrade(&"https://example.org/".parse().unwrap(), &hsts));
    }

    #[test]
    fn include_subdomains_reaches_children_only_when_set() {
        let hsts = InMemoryHsts::new();
        hsts.record("example.org", 3600, true);
        assert!(should_upgrade(&"http://api.example.org/".parse().unwrap(), &hsts));

        let narrow = InMemoryHsts::new();
        narrow.record("example.org", 3600, false);
        assert!(!should_upgrade(&"http://api.example.org/".parse().unwrap(), &narrow));
    }

    #[test]
    fn max_age_zero_clears() {
        let hsts = InMemoryHsts::new();
        hsts.record("example.org", 3600, false);
        hsts.record("example.org", 0, false);
        assert!(!should_upgrade(&"http://example.org/".parse().unwrap(), &hsts));
    }
}
