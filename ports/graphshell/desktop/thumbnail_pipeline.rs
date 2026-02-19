/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::mpsc::{Receiver, Sender};

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use log::warn;
use servo::{Image, PixelFormat, WebViewId};

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::window::ServoShellWindow;

const NODE_THUMBNAIL_WIDTH: u32 = 256;
const NODE_THUMBNAIL_HEIGHT: u32 = 192;

pub(crate) struct ThumbnailCaptureResult {
    pub(crate) webview_id: WebViewId,
    pub(crate) requested_url: String,
    pub(crate) png_bytes: Option<Vec<u8>>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) fn request_pending_thumbnail_captures(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    result_tx: &Sender<ThumbnailCaptureResult>,
    in_flight: &mut HashSet<WebViewId>,
) {
    in_flight.retain(|id| window.contains_webview(*id));

    for id in window.take_pending_thumbnail_capture_requests() {
        if in_flight.contains(&id) {
            continue;
        }

        let Some(webview) = window.webview_by_id(id) else {
            continue;
        };
        let Some(node_key) = graph_app.get_node_for_webview(id) else {
            continue;
        };
        let Some(node) = graph_app.graph.get_node(node_key) else {
            continue;
        };

        let requested_url = node.url.clone();
        if requested_url.starts_with("about:blank") {
            continue;
        }
        let sender = result_tx.clone();
        in_flight.insert(id);
        webview.take_screenshot(None, move |result| {
            let (png_bytes, width, height) = match result {
                Ok(image) => {
                    let resized = DynamicImage::ImageRgba8(image)
                        .resize_to_fill(
                            NODE_THUMBNAIL_WIDTH,
                            NODE_THUMBNAIL_HEIGHT,
                            FilterType::Triangle,
                        )
                        .to_rgba8();
                    let (width, height) = resized.dimensions();
                    let mut cursor = Cursor::new(Vec::new());
                    let png_bytes = match DynamicImage::ImageRgba8(resized)
                        .write_to(&mut cursor, ImageFormat::Png)
                    {
                        Ok(()) => Some(cursor.into_inner()),
                        Err(error) => {
                            warn!("Could not encode thumbnail PNG for {id:?}: {error}");
                            None
                        },
                    };
                    (png_bytes, width, height)
                },
                Err(error) => {
                    warn!("Could not capture thumbnail for {id:?}: {error:?}");
                    (None, 0, 0)
                },
            };
            let _ = sender.send(ThumbnailCaptureResult {
                webview_id: id,
                requested_url,
                png_bytes,
                width,
                height,
            });
        });
    }
}

pub(crate) fn load_pending_thumbnail_results(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    result_rx: &Receiver<ThumbnailCaptureResult>,
    in_flight: &mut HashSet<WebViewId>,
) -> Vec<GraphIntent> {
    in_flight.retain(|id| window.contains_webview(*id));
    let mut intents = Vec::new();

    while let Ok(result) = result_rx.try_recv() {
        in_flight.remove(&result.webview_id);
        if let Some(intent) = graph_intent_for_thumbnail_result(graph_app, &result) {
            intents.push(intent);
        }
    }
    intents
}

pub(crate) fn graph_intent_for_thumbnail_result(
    graph_app: &GraphBrowserApp,
    result: &ThumbnailCaptureResult,
) -> Option<GraphIntent> {
    let node_key = graph_app.get_node_for_webview(result.webview_id)?;
    let node = graph_app.graph.get_node(node_key)?;
    if node.url != result.requested_url {
        return None;
    }
    let png_bytes = result.png_bytes.clone()?;
    Some(GraphIntent::SetNodeThumbnail {
        key: node_key,
        png_bytes,
        width: result.width,
        height: result.height,
    })
}

fn embedder_image_to_rgba(image: &Image) -> (usize, usize, Vec<u8>) {
    let width = image.width as usize;
    let height = image.height as usize;

    let data = match image.format {
        PixelFormat::K8 => image.data().iter().flat_map(|&v| [v, v, v, 255]).collect(),
        PixelFormat::KA8 => image
            .data()
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        PixelFormat::RGB8 => image
            .data()
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        PixelFormat::RGBA8 => image.data().to_vec(),
        PixelFormat::BGRA8 => image
            .data()
            .chunks_exact(4)
            .flat_map(|chunk| [chunk[2], chunk[1], chunk[0], chunk[3]])
            .collect(),
    };

    (width, height, data)
}

pub(crate) fn load_pending_favicons(
    ctx: &egui::Context,
    window: &ServoShellWindow,
    graph_app: &GraphBrowserApp,
    texture_cache: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    for id in window.take_pending_favicon_loads() {
        let Some(webview) = window.webview_by_id(id) else {
            continue;
        };
        let Some(favicon) = webview.favicon() else {
            continue;
        };

        let (width, height, rgba) = embedder_image_to_rgba(&favicon);
        let egui_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        let handle = ctx.load_texture(format!("favicon-{id:?}"), egui_image, Default::default());
        let texture = egui::load::SizedTexture::new(
            handle.id(),
            egui::vec2(favicon.width as f32, favicon.height as f32),
        );
        texture_cache.insert(id, (handle, texture));

        if let Some(node_key) = graph_app.get_node_for_webview(id) {
            intents.push(GraphIntent::SetNodeFavicon {
                key: node_key,
                rgba,
                width: width as u32,
                height: height as u32,
            });
        }
    }
    intents
}
