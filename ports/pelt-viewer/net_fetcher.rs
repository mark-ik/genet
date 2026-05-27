/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Netfetcher-backed [`ResourceFetcher`] — the `netfetch` feature.
//!
//! Bridges the host's *sync* resource seam to the *async* netfetcher engine by
//! `block_on`-ing a GET on a small runtime. serval's `ImageLoader` (via
//! `HostImageLoader`) delegates `http(s)` URLs to this.
//!
//! Reference-host wiring: this holds a per-fetcher current-thread runtime.
//! Production (Mere's `FetcherPool` worker) should hold **one** fetcher (one
//! runtime) and fetch off the UI thread.

use pelt_core::ResourceFetcher;

/// Fetches `http(s)` resources via netfetcher, collecting the body synchronously.
pub struct NetResourceFetcher {
    runtime: tokio::runtime::Runtime,
}

impl NetResourceFetcher {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("netfetch runtime");
        Self { runtime }
    }
}

impl Default for NetResourceFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceFetcher for NetResourceFetcher {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let url = url::Url::parse(url).ok()?;
        self.runtime.block_on(async move {
            let cx = netfetcher::FetchContext::permissive();
            let resp = netfetcher::fetch(netfetcher::Request::get(url), &cx).await;
            if resp.is_network_error() {
                return None;
            }
            resp.bytes().await.ok().map(|bytes| bytes.to_vec())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::HostImageLoader;
    use image::ImageEncoder;
    use serval_layout::{ImagePlane, LocalFileImageLoader, ResourceResolver};
    use serval_static_dom::StaticDocument;

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 30, 30, 255]));
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(img.as_raw(), 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        buf
    }

    /// The whole integration: netfetcher fetches image bytes off a mock server,
    /// and those bytes flow through serval's `ImageLoader` seam into a decoded
    /// `ImagePlane` — all offline.
    #[test]
    fn netfetcher_bytes_flow_into_serval_image_decode() {
        let mut server = mockito::Server::new();
        let png = tiny_png();
        let _m = server
            .mock("GET", "/x.png")
            .with_status(200)
            .with_header("content-type", "image/png")
            .with_body(&png)
            .create();
        let url = format!("{}/x.png", server.url());

        let fetcher = NetResourceFetcher::new();

        // (1) netfetcher → ResourceFetcher returns the exact bytes.
        assert_eq!(fetcher.fetch(&url).as_deref(), Some(png.as_slice()));

        // (2) those bytes flow through serval's ImageLoader seam (via the host
        //     loader) and decode into the ImagePlane.
        let html = format!("<html><body><img src=\"{url}\"></body></html>");
        let doc = StaticDocument::parse(&html);
        let loader = HostImageLoader {
            local: LocalFileImageLoader::new(ResourceResolver {
                base_dir: None,
                tests_root: None,
            }),
            fetcher: Some(&fetcher),
        };
        let images = ImagePlane::decode_from_dom_with_loader(&doc, &loader);
        assert!(!images.is_empty(), "remote <img> decoded via netfetcher");

        // (3) a remote <link rel=stylesheet> flows through the same fetcher-backed
        //     loader: netfetcher fetches the CSS bytes and they become an author sheet.
        let css = "body { color: teal; }";
        let _c = server
            .mock("GET", "/s.css")
            .with_status(200)
            .with_header("content-type", "text/css")
            .with_body(css)
            .create();
        let css_url = format!("{}/s.css", server.url());
        let css_html =
            format!("<html><head><link rel=\"stylesheet\" href=\"{css_url}\"></head><body></body></html>");
        let css_doc = StaticDocument::parse(&css_html);
        let sheets = serval_layout::linked_stylesheets_with_loader(&css_doc, &loader);
        assert_eq!(sheets.len(), 1, "remote stylesheet fetched via netfetcher");
        assert!(sheets[0].contains("color: teal"));
    }
}
