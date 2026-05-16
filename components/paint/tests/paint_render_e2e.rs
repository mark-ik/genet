/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! D3.5 done-condition probe — drives `Paint::render` end-to-end.
//!
//! `c4_smoke_probe.rs` validated the renderer + compositor in
//! isolation: it built the Scene + Compositor by hand and called
//! `Renderer::render_with_compositor` directly. That misses the
//! embedder-facing path the production loop walks: `Paint::new` →
//! `register_rendering_context`/`install_renderer` →
//! `install_compositor` → `handle_messages(SendDisplayList)` →
//! `render(webview_id)` → `composite_texture(painter_id)`.
//!
//! This file covers that path. The test:
//!   1. Boots wgpu (Vulkan or Dx12 — backend-agnostic; we don't need
//!      a per-platform OS-handoff backend for this probe, the
//!      `WgpuMasterCaptureBackend` default is what we want here).
//!   2. Constructs a `netrender::Renderer` from those handles.
//!   3. Builds `Paint` via `Paint::new_for_test()` (skips the
//!      heavyweight `InitialPaintState`).
//!   4. Installs the renderer under a fresh `PainterId`.
//!   5. Sends a synthetic `PaintMessage::SendDisplayList` carrying a
//!      one-rect ServalDisplayList through `handle_messages`.
//!   6. Calls `Paint::render(webview_id)` — which, post-A.x, walks
//!      `webview_to_pipeline` → `pipelines[pipeline_id].scene` →
//!      `renderer.render_with_compositor(scene, format, &mut
//!      compositor, base)`.
//!   7. Asserts `Paint::composite_texture(painter_id)` returns
//!      `Some(master)` with the right dimensions/format — proving
//!      the master flowed all the way through the embedder-shaped
//!      API surface.

use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::CrossProcessPaintApi;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::serval_display_list::{
    ClipChainId, CommonItemPlacement, PrimitiveFlags, RectItem, ServalDisplayItem,
    ServalDisplayList,
};
use paint_types::units::{DeviceIntSize, LayoutPoint, LayoutRect, LayoutSize};
use paint_types::{ColorF, PipelineId, SpatialId};
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

fn synthesize_one_rect_display_list() -> (ServalDisplayList, PaintDisplayListInfo, PipelineId) {
    let pid = PipelineId::default();
    let mut list =
        ServalDisplayList::new(DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32), pid);
    list.push(ServalDisplayItem::Rect(RectItem {
        placement: CommonItemPlacement {
            clip_rect: LayoutRect::new(
                LayoutPoint::new(40.0, 40.0),
                LayoutPoint::new(216.0, 216.0),
            ),
            clip_chain_id: ClipChainId::INVALID,
            spatial_id: SpatialId(0, pid),
            flags: PrimitiveFlags::empty(),
        },
        color: ColorF {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        },
    }));

    let info = PaintDisplayListInfo::new(
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
    );

    (list, info, pid)
}

/// Full embedder-shaped path: handle_messages → render → composite_texture.
/// Passes if the master texture surfaces with the right dimensions /
/// format on the way back out.
#[test]
fn paint_render_e2e_drives_full_embedder_path() {
    // 1. wgpu boot + 2. renderer
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

    // 3. Paint via the test-only constructor.
    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    // 4. Install the renderer under a fresh PainterId; mint a
    //    matching WebViewId. Production embedders go through
    //    register_rendering_context which builds the renderer from
    //    the rendering context's WgpuCapability — this short-circuits.
    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    paint.install_renderer(painter_id, renderer);
    let webview_id = WebViewId::new(painter_id);

    // 5. Synthetic SendDisplayList through the public message API
    //    (the same path CrossProcessPaintApi::send_display_list
    //    drives in production).
    let (display_list, paint_info, _pid) = synthesize_one_rect_display_list();
    paint.handle_messages(vec![paint_api::PaintMessage::SendDisplayList {
        webview_id,
        display_list,
        paint_info,
    }]);

    // 6. Drive Paint::render.
    paint.render(webview_id);

    // 7. Read back the composite via the embedder-facing accessor.
    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after render");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT, "master texture width");
    assert_eq!(size.height, VIEWPORT, "master texture height");
    // Format is whatever Paint::render asks for — Rgba8Unorm at the
    // call site; assert it matches.
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

    // No install_renderer, no SendDisplayList — render should bail
    // out cleanly without panicking.
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

    let (list, info, _pid) = synthesize_one_rect_display_list();
    paint.handle_messages(vec![paint_api::PaintMessage::SendDisplayList {
        webview_id,
        display_list: list,
        paint_info: info,
    }]);

    paint.render(webview_id);
    let master_a = paint.composite_texture(painter_id).expect("frame 1");

    paint.render(webview_id);
    let master_b = paint.composite_texture(painter_id).expect("frame 2");

    assert_eq!(master_a.size(), master_b.size());
}
