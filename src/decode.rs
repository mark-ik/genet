/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Streaming `Content-Encoding` decode: gzip / deflate / br / zstd.
//!
//! Wraps a body stream so chunks are decompressed on the fly (no full-buffer
//! detour), using the standard `StreamReader → async decoder → ReaderStream`
//! pipeline. Multiple encodings (a comma-separated `Content-Encoding`) are undone
//! in reverse application order by chaining decoders. Unknown/`identity` layers
//! pass through untouched.

use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder};
use tokio::io::BufReader;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::response::BodyStream;

/// Wrap `stream` so it yields decoded bytes per the `Content-Encoding` header.
pub(crate) fn decode_stream(encoding: Option<&str>, stream: BodyStream) -> BodyStream {
    let Some(header) = encoding else {
        return stream;
    };
    let layers: Vec<String> = header
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let mut stream = stream;
    // Encodings are listed in application order; undo last-applied first.
    for layer in layers.iter().rev() {
        stream = wrap(layer, stream);
    }
    stream
}

fn wrap(codec: &str, stream: BodyStream) -> BodyStream {
    match codec {
        "gzip" | "x-gzip" => {
            Box::pin(ReaderStream::new(GzipDecoder::new(BufReader::new(StreamReader::new(stream)))))
        }
        // HTTP "deflate" is nominally zlib-wrapped.
        "deflate" => {
            Box::pin(ReaderStream::new(ZlibDecoder::new(BufReader::new(StreamReader::new(stream)))))
        }
        "br" => {
            Box::pin(ReaderStream::new(BrotliDecoder::new(BufReader::new(StreamReader::new(stream)))))
        }
        "zstd" => {
            Box::pin(ReaderStream::new(ZstdDecoder::new(BufReader::new(StreamReader::new(stream)))))
        }
        // identity / unknown: leave the stream untouched.
        _ => stream,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{Bytes, BytesMut};
    use futures_util::StreamExt;
    use std::io;

    fn once(data: Vec<u8>) -> BodyStream {
        Box::pin(futures_util::stream::once(async move {
            Ok::<_, io::Error>(Bytes::from(data))
        }))
    }

    async fn collect(mut s: BodyStream) -> Bytes {
        let mut buf = BytesMut::new();
        while let Some(chunk) = s.next().await {
            buf.extend_from_slice(&chunk.unwrap());
        }
        buf.freeze()
    }

    #[tokio::test]
    async fn gzip_stream_round_trips() {
        use std::io::Write;
        let mut enc =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"hello stream").unwrap();
        let gz = enc.finish().unwrap();

        let decoded = collect(decode_stream(Some("gzip"), once(gz))).await;
        assert_eq!(decoded.as_ref(), b"hello stream");
    }

    #[tokio::test]
    async fn no_encoding_is_passthrough() {
        let out = collect(decode_stream(None, once(b"plain".to_vec()))).await;
        assert_eq!(out.as_ref(), b"plain");
    }

    #[tokio::test]
    async fn unknown_and_identity_pass_through() {
        let out = collect(decode_stream(Some("exotic"), once(b"raw".to_vec()))).await;
        assert_eq!(out.as_ref(), b"raw");
        let out = collect(decode_stream(Some("identity"), once(b"raw".to_vec()))).await;
        assert_eq!(out.as_ref(), b"raw");
    }
}
