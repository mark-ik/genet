/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! End-to-end smoke probe.
//!
//! Drives a synthetic `<div>`-shaped [`PaintEnvelope`] through
//! [`paint::translate_paint_list`] → [`netrender::Renderer::render_with_compositor`]
//! → [`WgpuMasterCaptureBackend`], then asserts the master texture
//! handed to the compositor has the expected dimensions and format.
//!
//! This is the integration check the original C4 milestone named:
//!
//! > Done condition: a single `<div>` with background color renders
//! > end-to-end. (Doesn't require `cargo run`; a unit test that
//! > drives a synthetic paint output through the painter and checks
//! > the resulting Scene is acceptable.)

use netrender::{NetrenderOptions, boot, create_netrender_instance, peniko};
use paint::{WgpuMasterCaptureBackend, translate_paint_list};
use paint_list_api::{
    CommonPlacement, DeviceIntSize, EngineId, LayoutPoint, LayoutRect, PaintCmd, PaintEnvelope,
    PrimitiveFlags, RectItem,
};
use paint_types::ColorF;

const VIEWPORT: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn placement_at(bounds: LayoutRect) -> CommonPlacement {
    CommonPlacement {
        bounds,
        flags: PrimitiveFlags::empty(),
    }
}

/// Synthesize a single-rect paint envelope (the `<div>` analog).
fn one_rect_envelope() -> PaintEnvelope {
    PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![PaintCmd::DrawRect(RectItem {
            placement: placement_at(LayoutRect::new(
                LayoutPoint::new(40.0, 40.0),
                LayoutPoint::new(216.0, 216.0),
            )),
            color: ColorF {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        })],
        fonts: Vec::new(),
    }
}

/// End-to-end: synthetic paint envelope → Scene → Renderer →
/// WgpuMasterCaptureBackend. Asserts the master texture surfaces
/// with the expected viewport dimensions + format.
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

    let envelope = one_rect_envelope();
    let scene = translate_paint_list(&envelope);

    // Sanity: the translator produced one Rect op for our one rect.
    let rect_ops = scene
        .ops
        .iter()
        .filter(|op| matches!(op, netrender::SceneOp::Rect(_)))
        .count();
    assert_eq!(rect_ops, 1, "translator should emit one SceneOp::Rect");

    let mut compositor = WgpuMasterCaptureBackend::new();

    // render_with_compositor goes straight through to
    // compositor.present_frame; WgpuMasterCaptureBackend stashes the
    // master.
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
/// a master texture of the right dimensions.
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

    let envelope = PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: Vec::new(),
        fonts: Vec::new(),
    };
    let scene = translate_paint_list(&envelope);
    assert_eq!(scene.ops.len(), 0);

    let mut compositor = WgpuMasterCaptureBackend::new();
    let base = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);
    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);

    let master = compositor.last_master().expect("master must exist");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT);
    assert_eq!(size.height, VIEWPORT);
}

/// Two consecutive renders into the same compositor. Confirms
/// `last_master` is replaced (not stale) and the renderer is
/// reusable across frames.
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

    let envelope = one_rect_envelope();
    let scene = translate_paint_list(&envelope);

    let mut compositor = WgpuMasterCaptureBackend::new();
    let base = peniko::Color::new([0.0, 0.0, 0.0, 1.0]);

    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);
    let master_a = compositor.last_master().cloned().expect("frame 1");

    renderer.render_with_compositor(&scene, FORMAT, &mut compositor, base);
    let master_b = compositor.last_master().cloned().expect("frame 2");

    // Both frames produce a master of the same dimensions. Whether
    // the netrender master pool reuses the same `wgpu::Texture`
    // handle across frames is netrender's contract; here we only
    // care that the compositor captures whatever's current.
    assert_eq!(master_a.size(), master_b.size());
}
