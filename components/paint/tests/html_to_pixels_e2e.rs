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
    ImagePlane, StylePlane, emit_paint_list_with_layouts, layout, run_cascade,
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

    // 3b. Decode <img> sources + give each <img> its intrinsic size
    //     on any axis CSS left auto.
    let images = ImagePlane::decode_from_dom(&document);
    styles.apply_intrinsic_image_sizes(&images);

    // 4. Layout.
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(VIEWPORT as f32),
        height: taffy::AvailableSpace::Definite(VIEWPORT as f32),
    };
    let (fragments, built, text_ctx) = layout(&document, &styles, viewport);

    // 5. Emit (glyph runs from cached parley Layouts + DrawImage from
    //    the decoded image plane).
    let plist = emit_paint_list_with_layouts(
        &document,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        &images,
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

/// Text renders to actual glyph pixels. `<p>Hello serval</p>` on a
/// white body with the default black text color: scan the top-left
/// region where the text lays out and assert some pixels are
/// markedly darker than white — i.e., glyphs rasterized. This is the
/// receipt that the full text path holds together: parley shaping →
/// per-run TextRunItems + font side-table → translator registers the
/// font, resolves the key, emits SceneOp::GlyphRun → vello rasterizes
/// the outlines.
///
/// We don't assert exact glyph positions (font-dependent); "dark
/// pixels appear in the text band" is the robust, font-agnostic
/// check.
#[test]
fn html_to_pixels_text_rasterizes_glyphs() {
    let image = render_to_image(
        "<html><body><p>Hello serval</p></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            // Explicit black text; also the Stylo default, but pin it.
            "p { color: rgb(0, 0, 0); }",
        ],
    );

    // The text lays out near the top-left (p at body origin, glyphs
    // around the first line's baseline). Scan a generous band and
    // count pixels noticeably darker than the white background.
    let mut dark_pixels = 0u32;
    for y in 0..32u32 {
        for x in 0..120u32 {
            let [r, g, b, _a] = image.get_pixel(x, y).0;
            // A glyph pixel is substantially darker than white on at
            // least one channel (anti-aliased edges included).
            if r < 160 && g < 160 && b < 160 {
                dark_pixels += 1;
            }
        }
    }
    assert!(
        dark_pixels > 20,
        "expected glyph pixels (dark-on-white) in the text band, found {dark_pixels}"
    );
}

/// Read back the master texture rendered from the given HTML +
/// stylesheets. Shared helper for the multi-pixel test bodies below.
fn render_to_image(
    html: &str,
    stylesheets: &[&str],
) -> image::ImageBuffer<image::Rgba<u8>, Vec<u8>> {
    render_envelope_to_image(html_to_envelope(html, stylesheets))
}

/// Render a prebuilt `PaintEnvelope` through the full embedder path
/// and read the master back. Lets tests synthesize a `PaintEnvelope`
/// directly (e.g., with an image side-table) when the HTML producer
/// doesn't yet emit the command in question.
fn render_envelope_to_image(
    envelope: PaintEnvelope,
) -> image::ImageBuffer<image::Rgba<u8>, Vec<u8>> {
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

    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info: paint_info_for(PipelineId::default()),
    }]);
    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after render");

    read_texture_to_image(
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
    .expect("master readback")
}

/// A `DrawImage` referencing an `ImageResource` rasterizes its pixels.
/// Synthesizes a `PaintEnvelope` with a 2×2 solid-green RGBA image in
/// the side-table and a `DrawImage` stretching it over a 64×64 box,
/// then asserts the rendered master is green there. The HTML producer
/// doesn't emit `<img>` yet (needs decode + intrinsic sizing), so the
/// envelope is hand-built — this is the renderer-side receipt that
/// the image side-table → atlas registration → SceneOp::Image path
/// works, mirroring the DrawExternalTexture probe.
#[test]
fn draw_image_rasterizes_from_side_table() {
    use paint_list_api::{
        AlphaType, CommonPlacement, DeviceIntSize, EngineId, ImageItem, ImageRendering,
        ImageResource, LayoutPoint, LayoutRect, PaintCmd, PrimitiveFlags, RectItem,
    };
    use paint_types::{ColorF, IdNamespace, ImageKey};

    let key = ImageKey::new(IdNamespace(0), 1);
    // 2×2 opaque green, RGBA8.
    let green = [0u8, 255, 0, 255];
    let mut pixels = Vec::with_capacity(2 * 2 * 4);
    for _ in 0..4 {
        pixels.extend_from_slice(&green);
    }

    let bounds =
        |x: f32, y: f32, w: f32, h: f32| -> LayoutRect {
            LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
        };

    let envelope = PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![
            // White backdrop so "green" is unambiguous.
            PaintCmd::DrawRect(RectItem {
                placement: CommonPlacement {
                    bounds: bounds(0.0, 0.0, VIEWPORT as f32, VIEWPORT as f32),
                    flags: PrimitiveFlags::empty(),
                },
                color: ColorF::WHITE,
            }),
            PaintCmd::DrawImage(ImageItem {
                placement: CommonPlacement {
                    bounds: bounds(0.0, 0.0, 64.0, 64.0),
                    flags: PrimitiveFlags::empty(),
                },
                image_key: key,
                image_rendering: ImageRendering::Auto,
                alpha_type: AlphaType::PremultipliedAlpha,
                color: ColorF::WHITE, // identity tint
            }),
        ],
        fonts: Vec::new(),
        images: vec![ImageResource {
            key,
            width: 2,
            height: 2,
            data: pixels,
        }],
    };

    let image = render_envelope_to_image(envelope);

    // Inside the 64×64 image box: green.
    assert_eq!(
        image.get_pixel(32, 32).0,
        [0, 255, 0, 255],
        "(32, 32) is inside the DrawImage box, should be the image's green"
    );
    // Outside the image box, inside viewport: white backdrop.
    assert_eq!(
        image.get_pixel(100, 100).0,
        [255, 255, 255, 255],
        "(100, 100) is outside the image box, should be white"
    );
}

/// Full producer path: an `<img>` with a `data:` URI src renders from
/// HTML. The producer decodes the data URI (data-url + image crates),
/// sizes the `<img>` box to the image's intrinsic dimensions, and
/// emits DrawImage + an ImageResource — no hand-built envelope. This
/// is the receipt that `<img src="data:image/png;base64,…">` works
/// end-to-end: decode → intrinsic sizing → emit → translator → pixels.
#[test]
fn html_to_pixels_img_data_uri_renders() {
    use base64::Engine as _;

    // Encode a 16×16 solid-blue PNG, base64 it, build the data URI.
    let blue = image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 255, 255]));
    let mut png_bytes = Vec::new();
    blue.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .expect("encode test PNG");
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let data_uri = format!("data:image/png;base64,{b64}");

    // The <img> has no CSS width/height, so it takes the decoded
    // intrinsic size (16×16) and lays out at body's origin.
    let html = format!("<html><body><img src=\"{data_uri}\"></body></html>");
    let image = render_to_image(
        &html,
        &["body { background-color: rgb(255, 255, 255); }"],
    );

    // Inside the 16×16 image box (top-left): blue.
    assert_eq!(
        image.get_pixel(8, 8).0,
        [0, 0, 255, 255],
        "(8, 8) is inside the intrinsically-sized <img>, should be blue"
    );
    // Outside the image box: white body.
    assert_eq!(
        image.get_pixel(40, 40).0,
        [255, 255, 255, 255],
        "(40, 40) is outside the 16×16 <img>, should be white body"
    );
}

/// Nested elements with distinct colors paint into the right pixels.
/// `<div>` is 50×50 anchored at body's origin (top-left); a pixel
/// inside the div should carry its background color, and a pixel
/// outside the div (but inside body) should carry body's.
///
/// This is the receipt that compositor transforms compose correctly:
/// PushTransform pairs nest, child PaintCmds emit in the parent's
/// local space, and the translator's transform palette + Scene
/// layer stack produce the right absolute pixel positions.
#[test]
fn html_to_pixels_nested_elements_render_with_distinct_colors() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body { background-color: rgb(255, 0, 0); }",
            "div { width: 50px; height: 50px; background-color: rgb(0, 0, 255); }",
        ],
    );

    // Inside the div (50×50 at body's top-left): blue.
    assert_eq!(
        image.get_pixel(25, 25).0,
        [0, 0, 255, 255],
        "(25, 25) is inside the div, should be blue"
    );
    // Outside the div, inside body: red.
    assert_eq!(
        image.get_pixel(75, 75).0,
        [255, 0, 0, 255],
        "(75, 75) is outside the div but inside body, should be red"
    );
}

/// An offset element renders at its offset position, not at the
/// origin. This is the receipt that the translator's composed-
/// transform stack works for *non-zero* offsets — the earlier
/// nested-element test has every element at (0,0), so it can't
/// distinguish "transforms compose" from "everything draws at origin
/// regardless." Here `body` has 40px padding, pushing the div's
/// content box to (40, 40); the blue must appear there, while the
/// padding area at the top-left still shows body's red. (Padding
/// rather than margin avoids CSS margin-collapsing, which would
/// shift the body box itself.)
#[test]
fn html_to_pixels_offset_element_renders_at_offset() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body {
                background-color: rgb(255, 0, 0);
                padding-left: 40px;
                padding-top: 40px;
            }",
            "div {
                width: 30px;
                height: 30px;
                background-color: rgb(0, 0, 255);
            }",
        ],
    );

    // Inside the div's offset box (40,40)..(70,70): blue.
    assert_eq!(
        image.get_pixel(50, 50).0,
        [0, 0, 255, 255],
        "(50, 50) is inside the padding-offset div, should be blue"
    );
    // Top-left corner — body's padding area, div is NOT here; body red.
    assert_eq!(
        image.get_pixel(10, 10).0,
        [255, 0, 0, 255],
        "(10, 10) is in body's padding area above-left of the div, should be red"
    );
}

/// Border emission lands at the element's edges. A `<div>` with
/// `border: 10px solid green; width: 40px; height: 40px;` lays
/// out at 60×60 (border-box semantics) anchored at body's origin.
/// Pixels inside the border ring are green; pixels in the inner
/// content area are body's white.
#[test]
fn html_to_pixels_border_renders_at_element_edges() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "div {
                width: 40px;
                height: 40px;
                border: 10px solid rgb(0, 128, 0);
            }",
        ],
    );

    // Top border: y in [0, 10), x in [0, 60). (5, 5) is firmly inside.
    assert_eq!(
        image.get_pixel(5, 5).0,
        [0, 128, 0, 255],
        "(5, 5) should be inside the top border (green)"
    );
    // Right border: x in [50, 60), y in [0, 60). (55, 30) is inside.
    assert_eq!(
        image.get_pixel(55, 30).0,
        [0, 128, 0, 255],
        "(55, 30) should be inside the right border (green)"
    );
    // Inside the div's content area (no background-color declared on
    // div → transparent content area, body's white shows through).
    assert_eq!(
        image.get_pixel(30, 30).0,
        [255, 255, 255, 255],
        "(30, 30) is the div's content area; body's white shows through"
    );
    // Far outside the div, still inside body: white.
    assert_eq!(
        image.get_pixel(100, 100).0,
        [255, 255, 255, 255],
        "(100, 100) is body interior, should be white"
    );
}
