/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Fetch-spec [`Response`] — status, headers, a **streaming** body, plus the
//! spec concepts (response **type**/tainting and the redirect URL list) that a
//! wire-level `http::Response` doesn't carry.
//!
//! The body is a [`ResponseBody`] — a stream of decoded chunks delivered as they
//! arrive (`Content-Encoding` is undone on the fly). Most callers just want the
//! whole thing: [`Response::bytes`] / [`ResponseBody::bytes`] collect it.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use url::Url;

/// A pinned, boxed stream of decoded body chunks. Errors are `io::Error` so the
/// transport stream, `tokio_util` readers, and `async-compression` decoders all
/// compose without bespoke error plumbing.
pub(crate) type BodyStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

/// A Fetch response.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Decoded body, streamed. Consume via [`Self::bytes`] or as a [`Stream`].
    pub body: ResponseBody,
    /// Redirect chain, oldest first; the last entry is the final URL.
    pub url_list: Vec<Url>,
    pub response_type: ResponseType,
}

impl Response {
    /// The Fetch-spec **network error** response (`type` = error, `status` = 0,
    /// empty body). Returned for connection failures, blocked requests, etc.
    pub fn network_error() -> Self {
        Self {
            status: 0,
            headers: Vec::new(),
            body: ResponseBody::empty(),
            url_list: Vec::new(),
            response_type: ResponseType::Error,
        }
    }

    pub fn is_network_error(&self) -> bool {
        self.response_type == ResponseType::Error
    }

    /// Collect the entire body into one buffer. Convenience over
    /// `self.body.bytes()` for the common "I just want the bytes" case.
    pub async fn bytes(self) -> io::Result<Bytes> {
        self.body.bytes().await
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("url_list", &self.url_list)
            .field("response_type", &self.response_type)
            .field("body", &"<stream>")
            .finish()
    }
}

/// A streaming response body: decoded chunks delivered as they arrive.
///
/// Implements [`Stream`] (`Item = io::Result<Bytes>`) for incremental consumption
/// (downloads, progress, backpressure); [`Self::bytes`] collects it all.
pub struct ResponseBody {
    inner: BodyStream,
}

impl ResponseBody {
    pub(crate) fn new(inner: BodyStream) -> Self {
        Self { inner }
    }

    pub(crate) fn empty() -> Self {
        Self {
            inner: Box::pin(futures_util::stream::empty()),
        }
    }

    /// A body that yields a single already-buffered chunk (cached / collected
    /// responses). Empty input yields an empty body.
    pub(crate) fn from_bytes(data: Bytes) -> Self {
        if data.is_empty() {
            return Self::empty();
        }
        Self {
            inner: Box::pin(futures_util::stream::once(async move {
                Ok::<_, io::Error>(data)
            })),
        }
    }

    /// A body that yields the decoded prefix `data` (if any), then fails with
    /// `err`. For a buffered body whose `Content-Encoding` decode failed partway:
    /// the response itself is fine, but consuming the body must reject.
    pub(crate) fn from_bytes_then_error(data: Bytes, err: io::Error) -> Self {
        let mut items: Vec<io::Result<Bytes>> = Vec::new();
        if !data.is_empty() {
            items.push(Ok(data));
        }
        items.push(Err(err));
        Self {
            inner: Box::pin(futures_util::stream::iter(items)),
        }
    }

    /// Drain the stream into a single buffer.
    pub async fn bytes(mut self) -> io::Result<Bytes> {
        let mut buf = BytesMut::new();
        while let Some(chunk) = self.inner.next().await {
            buf.extend_from_slice(&chunk?);
        }
        Ok(buf.freeze())
    }

    /// Drain the stream, returning the decoded prefix plus the terminal error if
    /// decoding failed partway (`None` on a clean read). Lets a caller that must
    /// buffer the whole body (caching) tolerate a mid-body decode failure: keep
    /// the prefix, surface the error, and skip storing a corrupt entry.
    pub(crate) async fn collect_lossy(mut self) -> (Bytes, Option<io::Error>) {
        let mut buf = BytesMut::new();
        let mut err = None;
        while let Some(chunk) = self.inner.next().await {
            match chunk {
                Ok(b) => buf.extend_from_slice(&b),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        (buf.freeze(), err)
    }

    /// The next decoded chunk, or `None` at end of stream. Lets an embedder consume
    /// the body incrementally (stream it to a sink) without pulling in `StreamExt`.
    pub async fn next_chunk(&mut self) -> Option<io::Result<Bytes>> {
        self.inner.next().await
    }
}

impl Stream for ResponseBody {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `ResponseBody` is `Unpin` (its only field is a `Pin<Box<_>>`), so a
        // plain projection is sound.
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

/// Response tainting (WHATWG Fetch §2.2.6). Drives what script may observe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
    /// A network error — `status` 0, no usable headers/body.
    Error,
}
