/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HTTP-cache interaction for the fetch loop: the up-front cache probe, and the
//! tee-into-cache response body.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::SystemTime;

use bytes::{Bytes, BytesMut};
use futures_util::Stream;
use url::Url;

use crate::cache::{self, HttpCache, StoredResponse};
use crate::request::CacheMode;
use crate::response::BodyStream;
use crate::{FetchContext, Response};

use super::util::header_val;

/// Largest body buffered into the HTTP cache. Beyond this the response is streamed
/// straight through (never cached): buffering a large media body to store it would
/// stall the response, and it is not worth a cache slot.
const CACHE_MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Outcome of consulting the HTTP cache before going to the network.
pub(super) enum CacheProbe {
    /// A usable stored response (a fresh hit, `force-cache`, or `only-if-cached`):
    /// serve it without a network round-trip. (Also carries the `only-if-cached`
    /// miss, which is a network error.)
    Serve(Response),
    /// Proceed to the network. `revalidate` carries a stored entry to conditionally
    /// revalidate (its validators become request headers; a 304 refreshes it).
    Proceed { revalidate: Option<StoredResponse> },
}

/// Consult the HTTP cache (RFC 9111 + request cache mode) for a GET before the
/// network. `cache_key` is `None` when caching does not apply (non-GET, no-store,
/// or no cache wired) — then this always proceeds.
pub(super) fn probe_cache(
    cx: &FetchContext,
    cache_key: Option<&str>,
    mode: CacheMode,
    now: SystemTime,
    url_list: &[Url],
) -> CacheProbe {
    let mut revalidate: Option<StoredResponse> = None;
    if let Some(key) = cache_key {
        // `reload` always goes to the network (but still stores); the others may
        // consult the stored entry.
        if mode != CacheMode::Reload {
            match cx.cache.get(key) {
                Some(entry) => match mode {
                    // Use the stored response as-is, even if stale.
                    CacheMode::ForceCache | CacheMode::OnlyIfCached => {
                        return CacheProbe::Serve(cache::to_response(entry, url_list.to_vec()));
                    }
                    // Always revalidate.
                    CacheMode::NoCache => {
                        if cache::has_validators(&entry) {
                            revalidate = Some(entry);
                        }
                    }
                    // Default: serve fresh, revalidate stale / no-cache.
                    _ => {
                        if cache::is_fresh(&entry, now) && !cache::must_revalidate(&entry) {
                            return CacheProbe::Serve(cache::to_response(entry, url_list.to_vec()));
                        }
                        if cache::has_validators(&entry) {
                            revalidate = Some(entry);
                        }
                    }
                },
                // `only-if-cached` with no stored response is a network error.
                None if mode == CacheMode::OnlyIfCached => {
                    return CacheProbe::Serve(Response::network_error());
                }
                None => {}
            }
        }
    }
    CacheProbe::Proceed { revalidate }
}

/// Does `Content-Length` declare a body larger than the cache cap?
pub(super) fn over_cache_size_cap(headers: &[(String, String)]) -> bool {
    header_val(headers, "content-length")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|len| len > CACHE_MAX_BODY_BYTES)
}

/// A body stream that tees the decoded chunks it yields into the HTTP cache.
/// Passes every chunk through unchanged while accumulating it; on clean
/// end-of-stream (the consumer read the whole body) it stores the entry. A
/// mid-stream error, or a body the consumer abandons before completion, is never
/// stored — so the cache never holds a partial or corrupt response.
pub(super) struct CachingBody {
    pub(super) inner: BodyStream,
    pub(super) acc: BytesMut,
    pub(super) cache: Arc<dyn HttpCache>,
    pub(super) key: String,
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) stored_at: SystemTime,
    pub(super) poisoned: bool,
    pub(super) done: bool,
}

impl Stream for CachingBody {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `CachingBody`'s fields are all `Unpin`, so a plain projection is sound.
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(bytes))) => {
                this.acc.extend_from_slice(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.poisoned = true; // a decode/transport error: do not cache
                this.done = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.done = true;
                if !this.poisoned {
                    this.cache.put(
                        &this.key,
                        StoredResponse {
                            status: this.status,
                            headers: std::mem::take(&mut this.headers),
                            body: std::mem::take(&mut this.acc).freeze(),
                            stored_at: this.stored_at,
                        },
                    );
                }
                Poll::Ready(None)
            }
        }
    }
}

/// The stored/served body is decoded (identity), so its `Content-Encoding` and the
/// now-wrong `Content-Length` must not travel with it.
pub(super) fn strip_body_encoding_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("content-encoding") && !k.eq_ignore_ascii_case("content-length")
    });
}
