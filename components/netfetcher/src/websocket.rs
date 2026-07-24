/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebSocket (RFC 6455) over `ws://` / `wss://`.
//!
//! A distinct protocol from `fetch`: the handshake upgrades an HTTP/1.1 connection,
//! after which the connection is a bidirectional message stream. Native-only
//! (tokio + tungstenite); a wasm build would bind the browser's `WebSocket` instead.
//!
//! **Scope:** the connect + message send/recv surface, wrapping `tokio-tungstenite`
//! so the public API doesn't leak tungstenite types. The strongest consumer is
//! serval's eventual JS `WebSocket` binding (open-web rendering); Mere's p2p paths
//! use iroh streams, not this.

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

/// A WebSocket message, decoupled from the underlying tungstenite types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

impl WsMessage {
    fn from_tungstenite(message: Message) -> Option<Self> {
        match message {
            Message::Text(t) => Some(WsMessage::Text(t.as_str().to_owned())),
            Message::Binary(b) => Some(WsMessage::Binary(b.to_vec())),
            Message::Ping(b) => Some(WsMessage::Ping(b.to_vec())),
            Message::Pong(b) => Some(WsMessage::Pong(b.to_vec())),
            Message::Close(_) => Some(WsMessage::Close),
            Message::Frame(_) => None, // raw frames aren't surfaced
        }
    }

    fn into_tungstenite(self) -> Message {
        match self {
            WsMessage::Text(t) => Message::Text(t.into()),
            WsMessage::Binary(b) => Message::Binary(b.into()),
            WsMessage::Ping(b) => Message::Ping(b.into()),
            WsMessage::Pong(b) => Message::Pong(b.into()),
            WsMessage::Close => Message::Close(None),
        }
    }
}

/// An open WebSocket connection: a bidirectional [`WsMessage`] stream.
pub struct WebSocket {
    inner: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl WebSocket {
    /// Send a message. Returns `false` if the connection is broken.
    pub async fn send(&mut self, message: WsMessage) -> bool {
        self.inner.send(message.into_tungstenite()).await.is_ok()
    }

    /// Receive the next message, or `None` when the stream ends / errors. Raw
    /// frames are skipped.
    pub async fn recv(&mut self) -> Option<WsMessage> {
        loop {
            match self.inner.next().await {
                Some(Ok(message)) => {
                    if let Some(msg) = WsMessage::from_tungstenite(message) {
                        return Some(msg);
                    }
                    // Frame(_) → keep reading.
                }
                _ => return None,
            }
        }
    }

    /// Send a close frame.
    pub async fn close(&mut self) {
        let _ = self.inner.close(None).await;
    }
}

/// Open a WebSocket connection (`ws://` or `wss://`). `None` on handshake failure.
pub async fn connect(url: &Url) -> Option<WebSocket> {
    let (stream, _response) = tokio_tungstenite::connect_async(url.as_str()).await.ok()?;
    Some(WebSocket { inner: stream })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Start an in-process `ws://` echo server (127.0.0.1, ephemeral port).
    async fn start_echo() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    while let Some(Ok(message)) = ws.next().await {
                        if message.is_text() || message.is_binary() {
                            let _ = ws.send(message).await;
                        } else if message.is_close() {
                            break;
                        }
                    }
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn echo_round_trip() {
        let port = start_echo().await;
        let url: Url = format!("ws://127.0.0.1:{port}/").parse().unwrap();

        let mut ws = connect(&url).await.expect("ws handshake");
        assert!(ws.send(WsMessage::Text("hi".to_owned())).await);
        assert_eq!(ws.recv().await, Some(WsMessage::Text("hi".to_owned())));

        assert!(ws.send(WsMessage::Binary(vec![1, 2, 3])).await);
        assert_eq!(ws.recv().await, Some(WsMessage::Binary(vec![1, 2, 3])));

        ws.close().await;
    }
}
