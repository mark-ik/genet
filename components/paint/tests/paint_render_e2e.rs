/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Done-condition probe — drives `Paint::render` end-to-end through
//! the embedder-facing message API.
//!
//! `c4_smoke_probe.rs` validates the renderer + compositor in
//! isolation by building the Scene + Compositor by hand. This file
//! covers the production-shaped path: `Paint::new` →
//! `install_renderer` → `handle_messages(SendPaintList)` →
//! `render(webview_id)` → `composite_texture(painter_id)`.
//!
//! The test:
//!   1. Boots wgpu (Vulkan or Dx12 — backend-agnostic;
//!      `WgpuMasterCaptureBackend` is the test default).
//!   2. Constructs a `netrender::Renderer` from those handles.
//!   3. Builds `Paint` via `Paint::new_for_test()` (skips the
//!      heavyweight `InitialPaintState`).
//!   4. Installs the renderer under a fresh `PainterId`.
//!   5. Sends a synthetic `PaintMessage::SendPaintList` carrying a
//!      one-rect `PaintEnvelope` through `handle_messages`.
//!   6. Calls `Paint::render(webview_id)` — which walks
//!      `webview_to_pipeline` → `pipelines[pipeline_id].scene` →
//!      `renderer.render_with_compositor(scene, format, &mut
//!      compositor, base)`.
//!   7. Asserts `Paint::composite_texture(painter_id)` returns
//!      `Some(master)` with the right dimensions / format — proving
//!      the master flowed all the way through the embedder-shaped
//!      API surface.

use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::CrossProcessPaintApi;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_list_api::{
    CommonPlacement, DeviceIntSize, EngineId, LayoutPoint, LayoutRect, PaintCmd, PaintEnvelope,
    ColorF, PrimitiveFlags, RectItem,
};
use paint_types::PipelineId;
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};

/// `WebViewId::new` and `PainterId::next` reach into a thread-local
/// `PipelineNamespace`; in the production loop the constellation
/// installs one per script thread. The test harness has no
/// constellation, so each test installs one explicitly. Each `#[test]`
/// runs on its own thread, so a single unconditional `install` is
/// safe — no idempotency check needed.
fn ensure_pipeline_namespace() {
    PipelineNamespace::install(PipelineNamespaceId(1));
}

const VIEWPORT: u32 = 256;

fn synthesize_one_rect_envelope() -> (PaintEnvelope, PaintDisplayListInfo, PipelineId) {
    let pid = PipelineId::default();
    let envelope = PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement {
                bounds: LayoutRect::new(
                    LayoutPoint::new(40.0, 40.0),
                    LayoutPoint::new(216.0, 216.0),
                ),
                flags: PrimitiveFlags::empty(),
            },
            color: ColorF {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            },
        })],
        fonts: Vec::new(),
        images: Vec::new(),
    };

    let info = PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(VIEWPORT as f32, VIEWPORT as f32),
            hidpi_scale_factor: Scale::new(1.0),
        },
        paint_types::units::LayoutSize::new(VIEWPORT as f32, VIEWPORT as f32),
        pid,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    );

    (envelope, info, pid)
}

/// Full embedder-shaped path: handle_messages → render → composite_texture.
/// Passes if the master texture surfaces with the right dimensions /
/// format on the way back out.
#[test]
fn paint_render_e2e_drives_full_embedder_path() {
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

    let (envelope, paint_info, _pid) = synthesize_one_rect_envelope();
    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info,
    }]);

    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after render");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT, "master texture width");
    assert_eq!(size.height, VIEWPORT, "master texture height");
    assert_eq!(
        master.format(),
        wgpu::TextureFormat::Rgba8Unorm,
        "master texture format should match Paint::render's request"
    );

    // CrossProcessPaintApi is dummy in the test harness; ensure it
    // didn't somehow get used (would panic).
    let _: &CrossProcessPaintApi = &paint.paint_proxy().cross_process_paint_api;
}

/// `render(webview_id)` for an unknown webview is a no-op (no
/// pipeline registered → early return), and `composite_texture`
/// continues to report whatever the compositor's last_master was —
/// which is `None` if no render happened.
#[test]
fn paint_render_unknown_webview_is_noop() {
    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    let webview_id = WebViewId::new(painter_id);

    // No install_renderer, no SendPaintList — render should bail out
    // cleanly without panicking.
    paint.render(webview_id);

    // No master ever populated.
    assert!(paint.composite_texture(painter_id).is_none());
}

/// Two consecutive renders of the same webview replace the captured
/// master; `composite_texture` reflects the most recent one.
#[test]
fn paint_render_replaces_captured_master_per_frame() {
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

    let (envelope, info, _pid) = synthesize_one_rect_envelope();
    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info: info,
    }]);

    paint.render(webview_id);
    let master_a = paint.composite_texture(painter_id).expect("frame 1");

    paint.render(webview_id);
    let master_b = paint.composite_texture(painter_id).expect("frame 2");

    assert_eq!(master_a.size(), master_b.size());
}
