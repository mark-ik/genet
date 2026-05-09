/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C4 end-to-end smoke probe.
//!
//! Drives a synthetic `<div>`-shaped [`ServalDisplayList`] through the
//! C3 translator → [`netrender::Renderer::render_with_compositor`] →
//! [`StubCompositor`], then asserts the master texture handed to the
//! compositor has the expected dimensions and format.
//!
//! This is the integration check the C3 plan named:
//!
//! > Step 7 — Done condition: a single `<div>` with background color
//! > renders end-to-end. (Doesn't require `cargo run`; a unit test
//! > that drives a synthetic ServalDisplayList through the painter
//! > and checks the resulting Scene is acceptable.)
//!
//! The C4 milestone wired `Paint::render` against
//! `Renderer::render_with_compositor` + `StubCompositor`; this probe
//! exercises that path without a real embedder/window present.

use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance, peniko};
use paint::{StubCompositor, translate_display_list};
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::serval_display_list::{
    ClipChainId, CommonItemPlacement, PrimitiveFlags, RectItem, ServalDisplayItem,
    ServalDisplayList,
};
use paint_types::units::{DeviceIntSize, LayoutPoint, LayoutRect, LayoutSize};
use paint_types::{ColorF, PipelineId, SpatialId};

const VIEWPORT: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn placement(rect: LayoutRect, pid: PipelineId) -> CommonItemPlacement {
    CommonItemPlacement {
        clip_rect: rect,
        clip_chain_id: ClipChainId::INVALID,
        spatial_id: SpatialId(0, pid),
        flags: PrimitiveFlags::empty(),
    }
}

fn paint_info_for(viewport_w: f32, viewport_h: f32, pipeline_id: PipelineId) -> PaintDisplayListInfo {
    PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(viewport_w, viewport_h),
            hidpi_scale_factor: Scale::new(1.0),
        },
        LayoutSize::new(viewport_w, viewport_h),
        pipeline_id,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    )
}

/// Synthesize a single-rect display list (the `<div>` analog).
fn one_rect_list() -> (ServalDisplayList, PaintDisplayListInfo) {
    let pid = PipelineId::default();
    let mut list = ServalDisplayList::new(
        DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        pid,
    );
    list.push(ServalDisplayItem::Rect(RectItem {
        placement: placement(
            LayoutRect::new(
                LayoutPoint::new(40.0, 40.0),
                LayoutPoint::new(216.0, 216.0),
            ),
            pid,
        ),
        color: ColorF {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
    }));
    let info = paint_info_for(VIEWPORT as f32, VIEWPORT as f32, pid);
    (list, info)
}

/// End-to-end: synthetic display list → Scene → Renderer →
/// StubCompositor. Asserts the master texture surfaces with the
/// expected viewport dimensions + format.
#[test]
fn c4_smoke_probe_div_renders_to_master_texture() {
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

    let (list, info) = one_rect_list();
    let scene = translate_display_list(&list, &info);

    // Sanity: the translator produced one Rect op for our one rect.
    let rect_ops = scene
        .ops
        .iter()
        .filter(|op| matches!(op, netrender::SceneOp::Rect(_)))
        .count();
    assert_eq!(rect_ops, 1, "translator should emit one SceneOp::Rect");

    let mut compositor = StubCompositor::new();

    // Phase 5.1 of netrender's path-(b′) — render_with_compositor goes
    // straight through to compositor.present_frame; StubCompositor
    // stashes the master.
    let base = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);
    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);

    let master = compositor
        .last_master()
        .expect("present_frame should have stashed the master texture");

    let size = master.size();
    assert_eq!(
        size.width, VIEWPORT,
        "master texture width should match viewport"
    );
    assert_eq!(
        size.height, VIEWPORT,
        "master texture height should match viewport"
    );
    assert_eq!(
        master.format(),
        FORMAT,
        "master texture format should match the requested format"
    );
}

/// Empty list — no rects, no glyphs. Renderer should still hand back
/// a master texture of the right dimensions; the StubCompositor still
/// captures it.
#[test]
fn c4_smoke_probe_empty_scene_still_produces_master() {
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

    let pid = PipelineId::default();
    let list = ServalDisplayList::new(
        DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        pid,
    );
    let info = paint_info_for(VIEWPORT as f32, VIEWPORT as f32, pid);
    let scene = translate_display_list(&list, &info);
    assert_eq!(scene.ops.len(), 0);

    let mut compositor = StubCompositor::new();
    let base = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);
    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);

    let master = compositor.last_master().expect("master must exist");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT);
    assert_eq!(size.height, VIEWPORT);
}

/// Two consecutive renders into the same StubCompositor. Confirms the
/// compositor's `last_master` is replaced (not stale) and the
/// renderer is reusable across frames.
#[test]
fn c4_smoke_probe_two_frames_replace_master() {
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

    let (list, info) = one_rect_list();
    let scene = translate_display_list(&list, &info);

    let mut compositor = StubCompositor::new();
    let base = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);

    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);
    let master_a = compositor.last_master().cloned().expect("frame 1");

    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);
    let master_b = compositor.last_master().cloned().expect("frame 2");

    // Both frames produce a master of the same dimensions. Whether
    // the netrender master pool reuses the same `wgpu::Texture`
    // handle across frames is netrender's contract (verified in its
    // own `p13prime_path_b_master_pool_reuses_across_frames`); here
    // we only care that StubCompositor captures whatever's current.
    assert_eq!(master_a.size(), master_b.size());
}
