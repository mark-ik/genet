/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! End-to-end probe: HTML source → rendered master texture.
//!
//! This is the integration check the layout + paint arc has been
//! building toward. Earlier probes covered isolated seams:
//!
//! - `c4_smoke_probe.rs` — synthetic `PaintEnvelope` → Scene → master.
//! - `paint_render_e2e.rs` — `SendPaintList` → render → master.
//! - serval-layout's `paint_emit::tests::*` — cascade + layout +
//!   emission produce the right `PaintCmd` shape.
//!
//! Nothing yet drove a real HTML string all the way through.
//! This file does:
//!
//! 1. Parse HTML via `serval_static_dom::StaticDocument::parse`.
//! 2. Run the Stylo cascade with a stylesheet that paints `<body>`.
//! 3. Refresh Taffy styles from cascaded `ComputedValues`.
//! 4. Run layout → `FragmentPlane` + cached parley `Layout`s.
//! 5. Emit `ServalPaintList` (with glyph runs).
//! 6. Wrap into `PaintEnvelope::from_list`.
//! 7. Ship via `PaintMessage::SendPaintList` through `Paint`.
//! 8. `Paint::render(webview_id)`.
//! 9. Read back the master and assert the cascaded background color
//!    landed where the layout said `<body>` was.
//!
//! The probe doesn't validate text rendering — `DrawText` is still
//! a `warn!`-and-skip in the translator (gap documented in the
//! polyglot-renderer doc). It validates the seam: HTML in, pixels
//! out, with cascaded color applied.

use dpi::PhysicalSize;
use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::wgpu_readback::read_texture_to_image;
use paint_list_api::{DeviceIntSize, PaintEnvelope};
use paint_types::PipelineId;
use paint_types::units::{DeviceIntRect, LayoutSize};
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};
use serval_layout::{
    StylePlane, emit_paint_list_with_layouts, layout, run_cascade,
};
use serval_static_dom::StaticDocument;

const VIEWPORT: u32 = 128;

/// `WebViewId::new` and `PainterId::next` reach into a thread-local
/// `PipelineNamespace`; each `#[test]` runs on its own thread, so a
/// single unconditional install per test is safe.
fn ensure_pipeline_namespace() {
    PipelineNamespace::install(PipelineNamespaceId(1));
}

fn paint_info_for(pid: PipelineId) -> PaintDisplayListInfo {
    PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(VIEWPORT as f32, VIEWPORT as f32),
            hidpi_scale_factor: Scale::new(1.0),
        },
        LayoutSize::new(VIEWPORT as f32, VIEWPORT as f32),
        pid,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    )
}

/// Drive HTML → cascade → layout → emit → PaintEnvelope. Encapsulates
/// the producer side of the pipeline so the actual test bodies stay
/// focused on render + readback.
fn html_to_envelope(html: &str, stylesheets: &[&str]) -> PaintEnvelope {
    // 1. Parse HTML.
    let document = StaticDocument::parse(html);

    // 2. Cascade.
    let mut styles: StylePlane<_> = StylePlane::new();
    run_cascade(
        &document,
        &mut styles,
        euclid::Size2D::new(VIEWPORT as f32, VIEWPORT as f32),
        stylesheets,
    );

    // 3. Refresh Taffy styles from cascaded ComputedValues — switches
    //    layout from hand-rolled stubs to real CSS-driven box model.
    styles.refresh_taffy_from_cascade();

    // 4. Layout.
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(VIEWPORT as f32),
        height: taffy::AvailableSpace::Definite(VIEWPORT as f32),
    };
    let (fragments, built, text_ctx) = layout(&document, &styles, viewport);

    // 5. Emit (with glyph runs from the cached parley Layouts).
    let plist = emit_paint_list_with_layouts(
        &document,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
    );

    // 6. Wrap into the wire envelope.
    PaintEnvelope::from_list(&plist)
}

/// End-to-end seam: HTML source produces a master texture of the
/// right shape via the production-shaped `SendPaintList` path.
#[test]
fn html_to_pixels_drives_full_pipeline() {
    let handles = boot().expect("wgpu boot");
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    paint.install_renderer(painter_id, renderer);
    let webview_id = WebViewId::new(painter_id);

    let envelope = html_to_envelope(
        "<html><body><p>Hello, serval!</p></body></html>",
        // Empty stylist — every element gets Stylo defaults (transparent
        // background, initial color). The probe doesn't need a
        // stylesheet to validate the seam.
        &[],
    );

    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info: paint_info_for(PipelineId::default()),
    }]);
    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after HTML-driven render");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT, "master texture width");
    assert_eq!(size.height, VIEWPORT, "master texture height");
}

/// Same probe but with a stylesheet that paints `<body>` opaque red.
/// Reads pixels back and asserts the cascaded color landed where
/// layout said `<body>` was. This is the receipt that the *whole*
/// arc (cascade applies CSS → layout produces rects → emit produces
/// DrawRect with the cascaded color → translator emits SceneOp::Rect
/// → renderer rasterizes) holds together end-to-end.
#[test]
fn html_to_pixels_cascaded_background_color_renders_to_master() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    paint.install_renderer(painter_id, renderer);
    let webview_id = WebViewId::new(painter_id);

    // `body { background-color: rgb(255, 0, 0); }` — pure red.
    //
    // serval-layout's UA-defaults stylesheet (always prepended by
    // `run_cascade`) sets `html, body { display: block; width: 100%;
    // height: 100% }`, and `construct` gives the synthetic Taffy
    // root explicit viewport dimensions for those `100%`s to resolve
    // against. So an empty body with a `background-color` rule
    // covers the master texture — no test-side dimension workaround
    // needed.
    let envelope = html_to_envelope(
        "<html><body></body></html>",
        &["body { background-color: rgb(255, 0, 0); }"],
    );

    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info: paint_info_for(PipelineId::default()),
    }]);
    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after render");

    let image = read_texture_to_image(
        &device,
        &queue,
        &master,
        master.format(),
        PhysicalSize::new(VIEWPORT, VIEWPORT),
        DeviceIntRect::new(
            paint_types::units::DeviceIntPoint::new(0, 0),
            paint_types::units::DeviceIntPoint::new(VIEWPORT as i32, VIEWPORT as i32),
        ),
    )
    .expect("master readback");

    // A pixel near the center of the viewport should be opaque red —
    // somewhere inside the body's rect after the cascade applied the
    // background-color rule.
    let center = image.get_pixel(VIEWPORT as u32 / 2, VIEWPORT as u32 / 2).0;
    assert_eq!(
        center,
        [255, 0, 0, 255],
        "center pixel should be opaque red from cascaded background-color"
    );
}
