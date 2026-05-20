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
    BackgroundImagePlane, ImagePlane, StylePlane, emit_paint_list_with_layouts, layout, run_cascade,
};
use serval_static_dom::StaticDocument;

const VIEWPORT: u32 = 128;

/// Serializes GPU-touching tests. Booting several wgpu instances
/// concurrently faults on some Windows drivers
/// (`STATUS_ACCESS_VIOLATION`), so every test that boots a renderer
/// holds this guard for the duration of its GPU work — making the
/// suite safe under the default multi-threaded test harness without a
/// `--test-threads=1` invocation. Poison is recovered: a failing
/// assertion shouldn't cascade into lock-poison failures across the
/// rest of the suite.
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    html_to_envelope_with_loader(html, stylesheets, &serval_layout::NoImageLoader)
}

/// `html_to_envelope` variant that resolves remote `<img>` srcs
/// through `loader` (the host's resource cache, here a test fake).
fn html_to_envelope_with_loader<L: serval_layout::ImageLoader>(
    html: &str,
    stylesheets: &[&str],
    loader: &L,
) -> PaintEnvelope {
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

    // 3b. Decode <img> sources (data: inline, remote via loader) +
    //     give each <img> its intrinsic size on any auto axis.
    let images = ImagePlane::decode_from_dom_with_loader(&document, loader);
    styles.apply_intrinsic_image_sizes(&images);

    // 3c. Decode CSS background-image url() layers (data:/remote via
    //     loader). Kept in a separate plane — backgrounds don't size
    //     their box, so these must not feed apply_intrinsic_image_sizes.
    let bg_images = BackgroundImagePlane::decode_from_cascade(&document, &styles, loader);

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
        &bg_images,
        DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
    );

    // 6. Wrap into the wire envelope.
    PaintEnvelope::from_list(&plist)
}

/// End-to-end seam: HTML source produces a master texture of the
/// right shape via the production-shaped `SendPaintList` path.
#[test]
fn html_to_pixels_drives_full_pipeline() {
    let _gpu = gpu_serial();
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
    let _gpu = gpu_serial();
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

/// Per-span text color: a `<p>` (red text) containing a `<span>`
/// (blue text) flows on one line, and each run keeps its own color —
/// red glyph pixels AND blue glyph pixels both appear in the text
/// band. This is the receipt that per-run color rides the parley
/// brush through `Layout<ColorBrush>` and is read back per GlyphRun.
#[test]
fn html_to_pixels_inline_span_keeps_its_own_color() {
    let image = render_to_image(
        "<html><body><p>aaaaa <span>bbbbb</span></p></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "p { color: rgb(255, 0, 0); }",
            "span { color: rgb(0, 0, 255); }",
        ],
    );

    // Scan the top text band; classify glyph pixels by dominant channel.
    let mut saw_red = false;
    let mut saw_blue = false;
    for y in 0..32u32 {
        for x in 0..120u32 {
            let [r, g, b, _a] = image.get_pixel(x, y).0;
            // Reddish glyph pixel: red dominant, blue low.
            if r > 120 && b < 80 && g < 80 {
                saw_red = true;
            }
            // Bluish glyph pixel: blue dominant, red low.
            if b > 120 && r < 80 && g < 80 {
                saw_blue = true;
            }
        }
    }
    assert!(saw_red, "expected red glyph pixels from the <p> run");
    assert!(saw_blue, "expected blue glyph pixels from the <span> run");
}

/// Replaced inline content: an `<img>` mid-paragraph flows among the
/// text rather than forcing its own block. `<p>aaaaa <img> aaaaa</p>`
/// with a 16×16 blue data-URI image: the text is black on white, the
/// image is blue. The blue image pixels appear in the first line's
/// band, offset to the right of the leading "aaaaa " text (so the box
/// flowed inline, not at the paragraph origin), with dark glyph pixels
/// both before and after it on the same line.
///
/// This is the receipt that the inline-box path holds together:
/// `construct` gathers the `<img>` as an `InlineBoxItem` at its byte
/// offset, parley reserves + positions the box among the runs, and
/// emission resolves the box back to its decoded image and draws it at
/// the laid-out position.
#[test]
fn html_to_pixels_inline_img_flows_among_text() {
    use base64::Engine as _;

    // 16×16 solid-blue PNG → data URI.
    let blue = image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 255, 255]));
    let mut png_bytes = Vec::new();
    blue.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .expect("encode test PNG");
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let data_uri = format!("data:image/png;base64,{b64}");

    let html = format!(
        "<html><body><p>aaaaa <img src=\"{data_uri}\"> aaaaa</p></body></html>"
    );
    let image = render_to_image(
        &html,
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "p { color: rgb(0, 0, 0); }",
        ],
    );

    // Scan the first line's band. The img is 16px tall; text + img
    // share the line near the top-left. Collect the blue (image) pixel
    // columns and the dark (glyph) pixel columns.
    let mut blue_min_x: Option<u32> = None;
    let mut blue_max_x: Option<u32> = None;
    let mut dark_left_of_blue = false;
    let mut dark_right_of_blue = false;
    // First pass: locate the blue band.
    for y in 0..24u32 {
        for x in 0..120u32 {
            let [r, g, b, _a] = image.get_pixel(x, y).0;
            if b > 150 && r < 90 && g < 90 {
                blue_min_x = Some(blue_min_x.map_or(x, |m| m.min(x)));
                blue_max_x = Some(blue_max_x.map_or(x, |m| m.max(x)));
            }
        }
    }
    let blue_min_x = blue_min_x.expect("expected blue inline-img pixels in the first line band");
    let blue_max_x = blue_max_x.unwrap();

    // The image flowed *after* leading text, so it isn't flush at x=0.
    assert!(
        blue_min_x > 8,
        "inline img should be offset right by the leading text, got min x {blue_min_x}"
    );

    // Dark glyph pixels appear both before and after the blue box on
    // the line — the text wraps the image inline.
    for y in 0..24u32 {
        for x in 0..120u32 {
            let [r, g, b, _a] = image.get_pixel(x, y).0;
            let dark = r < 140 && g < 140 && b < 140;
            if dark {
                if x + 2 < blue_min_x {
                    dark_left_of_blue = true;
                }
                if x > blue_max_x + 2 {
                    dark_right_of_blue = true;
                }
            }
        }
    }
    assert!(
        dark_left_of_blue,
        "expected glyph pixels left of the inline img (the leading 'aaaaa ')"
    );
    assert!(
        dark_right_of_blue,
        "expected glyph pixels right of the inline img (the trailing 'aaaaa')"
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
    // Held for the whole GPU lifetime (boot → render → readback); all
    // helper-based tests funnel through here, so this serializes them
    // against each other and the direct-boot tests.
    let _gpu = gpu_serial();
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

/// A `DrawRepeatingImage` tiles its image across the extent via
/// `SceneOp::Pattern`. Synthesizes a `PaintEnvelope` with a 2×2 green
/// tile in the side-table and a `DrawRepeatingImage` over a 64×64 box
/// on a white backdrop; asserts the box fills green (the tile repeats
/// to cover it). The producer doesn't emit background-image yet (needs
/// cascade Image-list parsing + url decode), so the envelope is
/// hand-built — renderer-side receipt that the Pattern path works.
#[test]
fn draw_repeating_image_tiles_from_side_table() {
    use paint_list_api::{
        AlphaType, CommonPlacement, DeviceIntSize, EngineId, ImageRendering, ImageResource,
        LayoutPoint, LayoutRect, LayoutSize, PaintCmd, PrimitiveFlags, RectItem,
        RepeatingImageItem,
    };
    use paint_types::{ColorF, IdNamespace, ImageKey};

    let key = ImageKey::new(IdNamespace(0), 1);
    let green = [0u8, 255, 0, 255];
    let mut pixels = Vec::with_capacity(2 * 2 * 4);
    for _ in 0..4 {
        pixels.extend_from_slice(&green);
    }

    let b = |x: f32, y: f32, w: f32, h: f32| -> LayoutRect {
        LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
    };

    let envelope = PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![
            PaintCmd::DrawRect(RectItem {
                placement: CommonPlacement {
                    bounds: b(0.0, 0.0, VIEWPORT as f32, VIEWPORT as f32),
                    flags: PrimitiveFlags::empty(),
                },
                color: ColorF::WHITE,
            }),
            PaintCmd::DrawRepeatingImage(RepeatingImageItem {
                placement: CommonPlacement {
                    bounds: b(0.0, 0.0, 64.0, 64.0),
                    flags: PrimitiveFlags::empty(),
                },
                image_key: key,
                stretch_size: LayoutSize::new(2.0, 2.0),
                tile_spacing: LayoutSize::new(0.0, 0.0),
                image_rendering: ImageRendering::Auto,
                alpha_type: AlphaType::PremultipliedAlpha,
                color: ColorF::WHITE,
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

    // The tiled fill covers the 64×64 box — sample a few points.
    for (x, y) in [(5u32, 5u32), (32, 32), (60, 60)] {
        assert_eq!(
            image.get_pixel(x, y).0,
            [0, 255, 0, 255],
            "({x}, {y}) should be covered by the repeating green tile"
        );
    }
    // Outside the box: white backdrop.
    assert_eq!(
        image.get_pixel(100, 100).0,
        [255, 255, 255, 255],
        "(100, 100) is outside the pattern extent, should be white"
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

/// Remote `<img>` (http URL) renders via the ImageLoader seam. serval
/// doesn't fetch — a test loader stands in for the host's resource
/// cache, handing back PNG bytes for the URL. The producer decodes
/// them through the loader, sizes the box intrinsically, and emits
/// DrawImage. This is the receipt that `<img src="http://…">` works
/// once the host supplies the bytes (fetching stays Hekate's job).
#[test]
fn html_to_pixels_img_via_loader_renders() {
    /// Test fake: returns a fixed PNG for one known URL, nothing else.
    struct FakeLoader {
        url: String,
        png: Vec<u8>,
    }
    impl serval_layout::ImageLoader for FakeLoader {
        fn load(&self, url: &str) -> Option<Vec<u8>> {
            (url == self.url).then(|| self.png.clone())
        }
    }

    // A 16×16 solid-magenta PNG.
    let magenta = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 255, 255]));
    let mut png = Vec::new();
    magenta
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");

    let url = "http://example.test/icon.png";
    let loader = FakeLoader {
        url: url.to_string(),
        png,
    };

    let html = format!("<html><body><img src=\"{url}\"></body></html>");
    let envelope = html_to_envelope_with_loader(
        &html,
        &["body { background-color: rgb(255, 255, 255); }"],
        &loader,
    );
    let image = render_envelope_to_image(envelope);

    // Inside the 16×16 loaded image: magenta.
    assert_eq!(
        image.get_pixel(8, 8).0,
        [255, 0, 255, 255],
        "(8, 8) is inside the loader-supplied <img>, should be magenta"
    );
    // Outside: white body.
    assert_eq!(
        image.get_pixel(40, 40).0,
        [255, 255, 255, 255],
        "(40, 40) is outside the 16×16 <img>, should be white body"
    );
}

/// Full producer path: CSS `background-image: url(data:…)` tiles
/// across the element. The producer parses the cascaded
/// `background-image` url() layer, decodes it, and emits a
/// `DrawRepeatingImage` over the element box (CSS default
/// `background-repeat: repeat`). A `<div>` 40×40 with a 8×8 green
/// background tile on a white body: the div box fills green (the tile
/// repeats to cover it), the area outside stays white. No hand-built
/// envelope — decode → cascade read → emit → translator → pixels.
#[test]
fn html_to_pixels_background_image_tiles_from_css() {
    use base64::Engine as _;

    // 8×8 solid-green PNG → data URI.
    let green = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 200, 0, 255]));
    let mut png_bytes = Vec::new();
    green
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .expect("encode test PNG");
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let data_uri = format!("data:image/png;base64,{b64}");

    let css = format!(
        "div {{ width: 40px; height: 40px; background-image: url({data_uri}); }}"
    );
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &["body { background-color: rgb(255, 255, 255); }", &css],
    );

    // Inside the 40×40 div: the green tile repeats to cover it. Sample
    // a few points (the tile is solid green, so any covered pixel is
    // green regardless of tile boundaries).
    for (x, y) in [(4u32, 4u32), (20, 20), (36, 36)] {
        assert_eq!(
            image.get_pixel(x, y).0,
            [0, 200, 0, 255],
            "({x}, {y}) is inside the div, should be covered by the green background tile"
        );
    }
    // Outside the div, inside body: white.
    assert_eq!(
        image.get_pixel(80, 80).0,
        [255, 255, 255, 255],
        "(80, 80) is outside the div, should be white body"
    );
}

/// A hard `box-shadow` (blur 0) renders as an offset rect behind the
/// element. `body` white; a 30×30 black div with
/// `box-shadow: 20px 20px 0 rgb(255,0,0)` — the red shadow sits at the
/// div's box offset by (20, 20). Pixels in the offset shadow band
/// (right/below the div, outside it) are red; the div itself is black;
/// elsewhere is white. (True Gaussian blur needs the painter-side
/// mask pass; this first-cut renders hard shadows correctly and
/// blurred ones as a hard approximation.)
#[test]
fn html_to_pixels_box_shadow_hard_renders_offset() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "div {
                width: 30px;
                height: 30px;
                background-color: rgb(0, 0, 0);
                box-shadow: 20px 20px 0 rgb(255, 0, 0);
            }",
        ],
    );

    // The div is at (0,0)..(30,30) black. The shadow is the same box
    // offset by (20,20): (20,20)..(50,50) red, painted behind the div.
    // A point in the shadow band that the div doesn't cover — e.g.
    // (40, 40) — is red.
    assert_eq!(
        image.get_pixel(40, 40).0,
        [255, 0, 0, 255],
        "(40, 40) is in the offset shadow band, should be red"
    );
    // The div itself (painted over its shadow) is black.
    assert_eq!(
        image.get_pixel(10, 10).0,
        [0, 0, 0, 255],
        "(10, 10) is inside the div, should be black"
    );
    // Far from both: white body.
    assert_eq!(
        image.get_pixel(100, 100).0,
        [255, 255, 255, 255],
        "(100, 100) is clear of div + shadow, should be white"
    );
}

/// A blurred `box-shadow` renders a soft Gaussian halo via the
/// painter-side mask pass. `body` white; a 30×30 black div with
/// `box-shadow: 0 0 12px rgb(255,0,0)` — no offset, 12px blur. With a
/// hard shadow the (0,0,30,30) shadow box would sit exactly under the
/// div (black on top, nothing visible). The 12px blur spreads coverage
/// *outward* past the div edges, so reddish halo pixels appear beyond
/// the box, fading with distance. Their presence is the receipt that
/// `build_box_shadow_mask` ran and the mask composited.
#[test]
fn html_to_pixels_box_shadow_blur_renders_soft_halo() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "div {
                width: 30px;
                height: 30px;
                background-color: rgb(0, 0, 0);
                box-shadow: 0 0 12px rgb(255, 0, 0);
            }",
        ],
    );

    // Scan outward from the div's right edge along the vertical center
    // (y = 15). The div is (0,0)..(30,30); x in [31, 55) is outside it
    // but inside the blur halo. Count reddish pixels (red dominant over
    // the white backdrop — red channel high, green/blue pulled down by
    // the shadow's red tint).
    let mut halo_pixels = 0u32;
    for x in 31..55u32 {
        let [r, g, b, _a] = image.get_pixel(x, 15).0;
        if r > 200 && g < 235 && b < 235 && (r as i32 - g as i32) > 20 {
            halo_pixels += 1;
        }
    }
    assert!(
        halo_pixels >= 4,
        "expected a soft red halo beyond the div's right edge (blur spread), found {halo_pixels} reddish pixels"
    );

    // The div itself (painted over its shadow) is black.
    assert_eq!(
        image.get_pixel(15, 15).0,
        [0, 0, 0, 255],
        "(15, 15) is inside the div, should be black"
    );
    // Far from the div + halo: white body.
    assert_eq!(
        image.get_pixel(110, 110).0,
        [255, 255, 255, 255],
        "(110, 110) is clear of div + shadow halo, should be white"
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

/// Multiple block siblings stack vertically. Two 40px-tall `<div>`s
/// (red then blue) inside body lay out one above the other via Taffy's
/// block flow — the first fills the top band, the second sits directly
/// below it. This is the receipt that block-level siblings stack
/// (multi-paragraph flow) rather than overlapping at the origin.
#[test]
fn html_to_pixels_block_siblings_stack_vertically() {
    let image = render_to_image(
        "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            ".a { width: 60px; height: 40px; background-color: rgb(255, 0, 0); }",
            ".b { width: 60px; height: 40px; background-color: rgb(0, 0, 255); }",
        ],
    );

    // First div: top band (y ~20). Red.
    assert_eq!(
        image.get_pixel(20, 20).0,
        [255, 0, 0, 255],
        "(20, 20) is in the first (red) div"
    );
    // Second div stacks below the first (y ~60, since .a is 40px tall).
    assert_eq!(
        image.get_pixel(20, 60).0,
        [0, 0, 255, 255],
        "(20, 60) is in the second (blue) div, stacked below the first"
    );
}

/// Block-level floats: two `float: left` divs sit *side by side* on one
/// line, where plain blocks would stack vertically (cf.
/// `html_to_pixels_block_siblings_stack_vertically`). This is the
/// receipt that the cascade's `float` reaches Taffy's `float_layout`
/// algorithm through `stylo_taffy`'s converter and displaces sibling
/// blocks horizontally.
///
/// Limit (documented, not a bug): serval's inline content is a
/// parley-measured opaque leaf to Taffy, so *text wrapping around* a
/// float doesn't work yet — only block-level float displacement. Real
/// text reflow around floats needs integration at the IFC seam (see
/// docs/2026-05-20_stylo_taffy_adoption_plan.md).
#[test]
fn html_to_pixels_float_left_places_blocks_side_by_side() {
    let image = render_to_image(
        "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            ".a { float: left; width: 40px; height: 40px; background-color: rgb(255, 0, 0); }",
            ".b { float: left; width: 40px; height: 40px; background-color: rgb(0, 0, 255); }",
        ],
    );

    // First float at the top-left: (0,0)..(40,40) red.
    assert_eq!(
        image.get_pixel(20, 20).0,
        [255, 0, 0, 255],
        "(20, 20) is in the first float (red), top-left"
    );
    // Second float sits to the RIGHT of the first, same top band:
    // (40,0)..(80,40) blue. If floats didn't displace, .b would stack
    // below at y40-80 and (60, 20) would be white.
    assert_eq!(
        image.get_pixel(60, 20).0,
        [0, 0, 255, 255],
        "(60, 20) is in the second float (blue), placed beside the first"
    );
}

/// `position: relative` offsets a box from its in-flow position by its
/// inset, without removing it from flow. A relatively-positioned div
/// (`top: 20px; left: 20px`) at body's origin shifts down-right: its
/// blue lands at the offset position, and the in-flow origin (0,0) is
/// vacated (body white shows through).
#[test]
fn html_to_pixels_relative_position_offsets_box() {
    let image = render_to_image(
        "<html><body><div></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            "div {
                width: 30px;
                height: 30px;
                background-color: rgb(0, 0, 255);
                position: relative;
                top: 20px;
                left: 20px;
            }",
        ],
    );

    // Offset box: (20,20)..(50,50). (35, 35) is inside → blue.
    assert_eq!(
        image.get_pixel(35, 35).0,
        [0, 0, 255, 255],
        "(35, 35) is inside the relatively-offset div, should be blue"
    );
    // The in-flow origin the div vacated (5, 5) shows body white.
    assert_eq!(
        image.get_pixel(5, 5).0,
        [255, 255, 255, 255],
        "(5, 5) is above-left of the offset div, should be white (box shifted away)"
    );
}

/// `position: absolute` takes a box out of flow and places it at its
/// inset from the containing block. An absolutely-positioned div
/// (`top: 30px; left: 30px`) lands there regardless of its in-flow
/// siblings; a normal-flow sibling paints at the origin underneath.
#[test]
fn html_to_pixels_absolute_position_places_box() {
    let image = render_to_image(
        "<html><body><div class=\"flow\"></div><div class=\"abs\"></div></body></html>",
        &[
            "body { background-color: rgb(255, 255, 255); }",
            // In-flow sibling at the origin — green, 80x80.
            ".flow { width: 80px; height: 80px; background-color: rgb(0, 200, 0); }",
            // Out-of-flow box placed at (30, 30) — blue, 30x30.
            ".abs {
                width: 30px;
                height: 30px;
                background-color: rgb(0, 0, 255);
                position: absolute;
                top: 30px;
                left: 30px;
            }",
        ],
    );

    // Absolute box: (30,30)..(60,60). (45, 45) is inside → blue,
    // painted over the green in-flow sibling.
    assert_eq!(
        image.get_pixel(45, 45).0,
        [0, 0, 255, 255],
        "(45, 45) is inside the absolutely-positioned div, should be blue"
    );
    // The in-flow green sibling shows where the absolute box doesn't
    // cover it — e.g. (10, 10), top-left of the 80x80 green box.
    assert_eq!(
        image.get_pixel(10, 10).0,
        [0, 200, 0, 255],
        "(10, 10) is the in-flow green sibling, not covered by the absolute box"
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
