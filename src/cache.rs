/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HTTP cache (RFC 9111 core).
//!
//! The split: [`HttpCache`] is a dumb **storage** seam (`get`/`put` of a
//! [`StoredResponse`]) so Mere can back durable storage; the RFC 9111 **policy**
//! (cacheability, freshness, conditional revalidation) lives here in netfetcher
//! and is driven from `fetch`. `enabled()` lets the default [`NoHttpCache`] tell
//! `fetch` to skip the work entirely (caching forces body buffering, which is
//! pointless if the store discards it).
//!
//! **Increment-2 scope:** caches `GET` `200` responses with an explicit freshness
//! signal (`Cache-Control: max-age` / `Expires`); serves fresh hits without a
//! network round-trip; revalidates stale-or-`no-cache` entries via
//! `ETag`/`Last-Modified` → `If-None-Match`/`If-Modified-Since` → `304`. Honors
//! `no-store`. **Deferred:** heuristic freshness, `Vary` (responses carrying it
//! are conservatively *not* cached), the wider cacheable-status set,
//! stale-while-revalidate.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use url::Url;

use crate::Response;
use crate::response::{ResponseBody, ResponseType};

/// A cached response: enough to reconstruct a [`Response`] and to revalidate.
#[derive(Clone)]
pub struct StoredResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    /// When the entry was stored (or last revalidated) — the freshness baseline.
    pub stored_at: SystemTime,
}

/// Storage seam for the HTTP cache. Policy lives in this module; an implementor
/// only stores and retrieves by key.
pub trait HttpCache: Send + Sync {
    /// Whether caching is active. Default `true`; [`NoHttpCache`] returns `false`
    /// so `fetch` skips buffering responses purely to discard them.
    fn enabled(&self) -> bool {
        true
    }
    fn get(&self, key: &str) -> Option<StoredResponse>;
    fn put(&self, key: &str, entry: StoredResponse);
}

/// The default cache: stores nothing.
pub struct NoHttpCache;

impl HttpCache for NoHttpCache {
    fn enabled(&self) -> bool {
        false
    }
    fn get(&self, _key: &str) -> Option<StoredResponse> {
        None
    }
    fn put(&self, _key: &str, _entry: StoredResponse) {}
}

/// A simple process-local in-memory cache.
#[derive(Default)]
pub struct InMemoryHttpCache {
    entries: Mutex<HashMap<String, StoredResponse>>,
}

impl InMemoryHttpCache {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HttpCache for InMemoryHttpCache {
    fn get(&self, key: &str) -> Option<StoredResponse> {
        self.entries.lock().ok()?.get(key).cloned()
    }
    fn put(&self, key: &str, entry: StoredResponse) {
        if let Ok(mut map) = self.entries.lock() {
            map.insert(key.to_owned(), entry);
        }
    }
}

// ---------------------------------------------------------------------------
// Policy (RFC 9111 core) — used by `fetch`, kept here so storage stays dumb.
// ---------------------------------------------------------------------------

pub(crate) fn cache_key(method: &str, url: &Url) -> String {
    format!("{method} {url}")
}

/// May this response be *stored*? (Caller already restricts to `GET`.)
pub(crate) fn is_cacheable(status: u16, headers: &[(String, String)]) -> bool {
    status == 200
        && !cache_control_has(headers, "no-store")
        // Conservative v1: never cache a varying response (avoids wrong-variant hits).
        && header(headers, "vary").is_none()
}

/// Is a stored entry usable without revalidation right now?
pub(crate) fn is_fresh(entry: &StoredResponse, now: SystemTime) -> bool {
    match freshness_lifetime(&entry.headers, entry.stored_at) {
        Some(lifetime) => now.duration_since(entry.stored_at).unwrap_or_default() < lifetime,
        None => false, // no explicit signal → must revalidate (no heuristic freshness in v1)
    }
}

/// `Cache-Control: no-cache` → may be stored, but must revalidate before use.
pub(crate) fn must_revalidate(entry: &StoredResponse) -> bool {
    cache_control_has(&entry.headers, "no-cache")
}

pub(crate) fn has_validators(entry: &StoredResponse) -> bool {
    header(&entry.headers, "etag").is_some() || header(&entry.headers, "last-modified").is_some()
}

pub(crate) fn conditional_headers(entry: &StoredResponse) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(etag) = header(&entry.headers, "etag") {
        out.push(("if-none-match".to_owned(), etag.to_owned()));
    }
    if let Some(last_modified) = header(&entry.headers, "last-modified") {
        out.push(("if-modified-since".to_owned(), last_modified.to_owned()));
    }
    out
}

/// Update a stored entry from a `304` response: overlay its headers and reset the
/// freshness baseline.
pub(crate) fn refresh(
    mut entry: StoredResponse,
    new_headers: &[(String, String)],
    now: SystemTime,
) -> StoredResponse {
    for (k, v) in new_headers {
        if let Some(slot) = entry.headers.iter_mut().find(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            slot.1 = v.clone();
        } else {
            entry.headers.push((k.clone(), v.clone()));
        }
    }
    entry.stored_at = now;
    entry
}

/// Build a [`Response`] to hand back from a cached entry.
pub(crate) fn to_response(entry: StoredResponse, url_list: Vec<Url>) -> Response {
    Response {
        status: entry.status,
        headers: entry.headers,
        body: ResponseBody::from_bytes(entry.body),
        url_list,
        response_type: ResponseType::Basic,
    }
}

fn freshness_lifetime(headers: &[(String, String)], stored_at: SystemTime) -> Option<Duration> {
    if let Some(max_age) = parse_max_age(headers) {
        return Some(Duration::from_secs(max_age));
    }
    // Expires − Date (fall back to stored_at when there's no Date header).
    let expires = httpdate::parse_http_date(header(headers, "expires")?).ok()?;
    let base = header(headers, "date")
        .and_then(|d| httpdate::parse_http_date(d).ok())
        .unwrap_or(stored_at);
    expires.duration_since(base).ok()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn cache_control_has(headers: &[(String, String)], directive: &str) -> bool {
    header(headers, "cache-control").is_some_and(|cc| {
        cc.split(',').any(|d| d.trim().eq_ignore_ascii_case(directive))
    })
}

fn parse_max_age(headers: &[(String, String)]) -> Option<u64> {
    let cc = header(headers, "cache-control")?.to_ascii_lowercase();
    cc.split(',')
        .find_map(|d| d.trim().strip_prefix("max-age=").and_then(|v| v.trim().parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(headers: Vec<(&str, &str)>, age_secs: u64) -> StoredResponse {
        StoredResponse {
            status: 200,
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            body: Bytes::from_static(b"x"),
            stored_at: SystemTime::now() - Duration::from_secs(age_secs),
        }
    }

    #[test]
    fn fresh_within_max_age() {
        let e = entry(vec![("cache-control", "max-age=60")], 10);
        assert!(is_fresh(&e, SystemTime::now()));
    }

    #[test]
    fn stale_past_max_age() {
        let e = entry(vec![("cache-control", "max-age=5")], 30);
        assert!(!is_fresh(&e, SystemTime::now()));
    }

    #[test]
    fn no_freshness_signal_is_not_fresh() {
        let e = entry(vec![("etag", "\"abc\"")], 0);
        assert!(!is_fresh(&e, SystemTime::now()));
    }

    #[test]
    fn no_store_is_uncacheable_and_vary_is_skipped() {
        assert!(is_cacheable(200, &[("cache-control".into(), "max-age=60".into())]));
        assert!(!is_cacheable(200, &[("cache-control".into(), "no-store".into())]));
        assert!(!is_cacheable(200, &[("vary".into(), "accept-encoding".into())]));
        assert!(!is_cacheable(404, &[]));
    }

    #[test]
    fn conditional_headers_from_validators() {
        let e = entry(vec![("etag", "\"v1\""), ("last-modified", "Wed, 21 Oct 2026 07:28:00 GMT")], 0);
        let cond = conditional_headers(&e);
        assert!(cond.contains(&("if-none-match".to_owned(), "\"v1\"".to_owned())));
        assert!(cond.iter().any(|(k, _)| k == "if-modified-since"));
    }
}
