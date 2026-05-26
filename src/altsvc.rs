/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Alt-Svc discovery (RFC 7838) — learning which origins speak HTTP/3.
//!
//! A server advertises h3 via an `Alt-Svc` response header on an h1/h2 response;
//! the engine records it so *subsequent* requests to that origin can use h3. This
//! module is the parser + the storage seam; the actual h3 routing wiring is the
//! next step. The seam is portable (host strings + ports + expiry — no quinn
//! types), so it lives on `FetchContext` regardless of target.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Storage seam for learned Alt-Svc (h3) advertisements.
pub trait AltSvcStore: Send + Sync {
    /// The advertised h3 port for `host`, if a non-expired advertisement exists.
    fn h3_port(&self, host: &str) -> Option<u16>;
    /// Record an h3 advertisement for `host` (`max_age_secs` 0 clears it).
    fn record_h3(&self, host: &str, port: u16, max_age_secs: u64);
    /// Clear advertisements for `host` (`Alt-Svc: clear`).
    fn clear(&self, host: &str);
}

/// Process-local in-memory Alt-Svc store.
#[derive(Default)]
pub struct InMemoryAltSvc {
    entries: Mutex<HashMap<String, (u16, SystemTime)>>,
}

impl InMemoryAltSvc {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AltSvcStore for InMemoryAltSvc {
    fn h3_port(&self, host: &str) -> Option<u16> {
        let entries = self.entries.lock().ok()?;
        let (port, expiry) = entries.get(&host.to_ascii_lowercase())?;
        (*expiry > SystemTime::now()).then_some(*port)
    }
    fn record_h3(&self, host: &str, port: u16, max_age_secs: u64) {
        if max_age_secs == 0 {
            self.clear(host);
            return;
        }
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                host.to_ascii_lowercase(),
                (port, SystemTime::now() + Duration::from_secs(max_age_secs)),
            );
        }
    }
    fn clear(&self, host: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&host.to_ascii_lowercase());
        }
    }
}

/// The parsed meaning of an `Alt-Svc` header value (for our purposes).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AltSvc {
    /// `Alt-Svc: clear` — drop all advertisements for the origin.
    Clear,
    /// The first `h3` alternative: its port and `ma` (max-age, default 86400).
    H3 { port: u16, max_age: u64 },
    /// No usable h3 advertisement.
    None,
}

/// Parse an `Alt-Svc` header value. v1 takes the first `h3` alternative and its
/// port; alt-host routing (`h3="other:443"`) is recorded as the port only — the
/// engine routes to the same host on that port.
pub(crate) fn parse_alt_svc(value: &str) -> AltSvc {
    let value = value.trim();
    if value.eq_ignore_ascii_case("clear") {
        return AltSvc::Clear;
    }
    for alternative in value.split(',') {
        let mut params = alternative.trim().split(';');
        let Some(first) = params.next() else { continue };
        let Some((proto, authority)) = first.trim().split_once('=') else {
            continue;
        };
        if proto.trim() != "h3" {
            continue; // v1: only "h3" (not h3-29 and other drafts)
        }
        let authority = authority.trim().trim_matches('"');
        let Some(port) = authority.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
            continue;
        };
        let mut max_age = 86400; // RFC 7838 default
        for param in params {
            if let Some(ma) = param.trim().strip_prefix("ma=") {
                if let Ok(n) = ma.trim().parse() {
                    max_age = n;
                }
            }
        }
        return AltSvc::H3 { port, max_age };
    }
    AltSvc::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_h3_advertisement() {
        assert_eq!(
            parse_alt_svc("h3=\":443\"; ma=3600"),
            AltSvc::H3 { port: 443, max_age: 3600 }
        );
        // No ma → default 86400; alt-host authority → port only.
        assert_eq!(
            parse_alt_svc("h3=\"alt.example:8443\""),
            AltSvc::H3 { port: 8443, max_age: 86400 }
        );
        // First alternative wins; h2 ignored.
        assert_eq!(
            parse_alt_svc("h2=\":443\", h3=\":443\"; ma=60"),
            AltSvc::H3 { port: 443, max_age: 60 }
        );
        assert_eq!(parse_alt_svc("clear"), AltSvc::Clear);
        assert_eq!(parse_alt_svc("h2=\":443\""), AltSvc::None);
    }

    #[test]
    fn store_records_and_expires() {
        let store = InMemoryAltSvc::new();
        store.record_h3("example.org", 443, 3600);
        assert_eq!(store.h3_port("example.org"), Some(443));
        assert_eq!(store.h3_port("EXAMPLE.ORG"), Some(443)); // case-insensitive
        assert_eq!(store.h3_port("other.example"), None);

        store.record_h3("example.org", 443, 0); // ma=0 clears
        assert_eq!(store.h3_port("example.org"), None);
    }
}
