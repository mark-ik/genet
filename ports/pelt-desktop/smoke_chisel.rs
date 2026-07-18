/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sprigging first-pixels smoke: three `<custom-leaf>` elements (Knob, Meter,
//! GraphGlyph) laid out by a retained session, rendered through the session
//! leaf path (`scene_from_session_dom_with_leaves`) to an offscreen wgpu
//! target, read back, color-checked, and PNG-encoded as the human-eyes
//! receipt. Mirrors [`crate::headless::render_png`]'s GPU harness. See
//! `docs/2026-07-08_chisel_widget_catalog.md` (build-order step 2's on-screen
//! half) and `docs/2026-07-07_chisel_widget_leaf_design.md`.

use genet_layout::IncrementalLayout;
use genet_scripted_dom::ScriptedDom;
use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};
use sprigging::{
    ColorF, GraphGlyph, GraphGlyphNode, Knob, LeafRegistry, Meter, RenderedLeaves, Size,
};

use crate::tile_surface::scene_from_session_dom_with_leaves;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChiselSmokeOutcome {
    pub width: u32,
    pub height: u32,
    /// `<custom-leaf>` boxes the session laid out (attribution: layout seam).
    pub leaf_boxes: usize,
    /// Leaves the registry actually painted (attribution: render gate).
    pub leaves_painted: usize,
    /// Pixels where the knob's value-arc blue dominates.
    pub blue_pixels: usize,
    /// Pixels where the meter's fill green dominates.
    pub green_pixels: usize,
    /// Pixels where the graph glyph's magenta nodes dominate.
    pub magenta_pixels: usize,
}

fn html(local: &str) -> QualName {
    QualName::new(
        None,
        Namespace::from("http://www.w3.org/1999/xhtml"),
        LocalName::from(local),
    )
}

fn attr(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

/// Render the three-leaf document at 200x80 and return the color counts,
/// writing the PNG receipt to `out_png` when given.
pub fn run_chisel_smoke(out_png: Option<&std::path::Path>) -> Result<ChiselSmokeOutcome, String> {
    const W: u32 = 200;
    const H: u32 = 80;

    // The document: three keyed leaves in a row.
    let mut dom = ScriptedDom::new();
    let root = dom.document();
    let body = dom.create_element(html("body"));
    dom.append_child(root, body);
    for key in ["1", "2", "3"] {
        let leaf = dom.create_element(html("custom-leaf"));
        dom.set_attribute(leaf, attr("key"), key);
        dom.append_child(body, leaf);
    }
    // Flex row: flex items blockify, so each leaf takes the block-level
    // replaced path (where `custom_leaf_key` is stamped onto its box). Multiple
    // inline-flowing replaced leaves ride `InlineContent` instead and are a
    // known chisel gap (see the catalog doc's open questions).
    let sheets = [
        "body { display: flex; padding: 8px; }",
        "custom-leaf { display: block; width: 48px; height: 48px; margin: 4px; }",
    ];

    // The leaves: knob at 2/3, meter at 0.7 with a 0.9 peak, a 4-node glyph
    // with magenta nodes.
    let mut registry: LeafRegistry<u64> = LeafRegistry::new();
    let mut cache = RenderedLeaves::new();
    let square = Size {
        width: 48.0,
        height: 48.0,
    };
    let mut knob = Knob::new(square);
    knob.set_value(0.66);
    registry.insert(1, Box::new(knob));
    let mut meter = Meter::new(true, square);
    meter.set_level(0.7, Some(0.9));
    registry.insert(2, Box::new(meter));
    let magenta = ColorF {
        r: 0.9,
        g: 0.15,
        b: 0.9,
        a: 1.0,
    };
    let node = |x: f32, y: f32| GraphGlyphNode {
        x,
        y,
        color: magenta,
    };
    let mut glyph = GraphGlyph::new(
        vec![
            node(0.1, 0.2),
            node(0.9, 0.1),
            node(0.5, 0.9),
            node(0.2, 0.8),
        ],
        vec![(0, 1), (1, 2), (2, 3), (3, 0)],
        square,
    );
    glyph.node_radius = 4.0;
    registry.insert(3, Box::new(glyph));

    // Retained session -> session leaf path -> Scene. The pre-render here fills
    // the cache and yields the attribution counts; the scene call's internal
    // render_into then no-ops on the clean cache (the retention gate).
    let session = IncrementalLayout::new(&dom, &sheets, W as f32, H as f32);
    let boxes = session.custom_leaf_boxes();
    let sizes: std::collections::HashMap<u64, (f32, f32)> = boxes.iter().copied().collect();
    let leaves_painted = registry.render_into(
        |key| {
            sizes.get(&key).map(|&(w, h)| Size {
                width: w,
                height: h,
            })
        },
        &mut cache,
    );
    let scene = scene_from_session_dom_with_leaves(&session, &dom, W, H, &mut registry, &mut cache);

    // Offscreen render + readback (the render_png harness shape).
    let handles = netrender::boot().map_err(|e| format!("netrender wgpu boot failed: {e}"))?;
    let device = handles.device.clone();
    let renderer = netrender::create_netrender_instance(
        handles,
        netrender::NetrenderOptions {
            tile_cache_size: Some(32),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("netrender renderer init failed: {e:?}"))?;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("chisel smoke target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("chisel smoke view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    renderer.render_vello(
        &scene,
        &view,
        netrender::ColorLoad::Clear(wgpu::Color::WHITE),
    );
    let bytes = renderer.wgpu_device.read_rgba8_texture(&target, W, H);

    // Color-dominance counts (robust to sRGB/linear encoding): the knob's value
    // arc is blue-dominant, the meter fill green-dominant, the glyph nodes
    // magenta (red+blue over green). The white clear matches none.
    let mut blue = 0usize;
    let mut green = 0usize;
    let mut magenta_px = 0usize;
    for px in bytes.chunks_exact(4) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        if b > r + 40 && b > g + 40 {
            blue += 1;
        } else if g > r + 40 && g > b + 40 {
            green += 1;
        } else if r > g + 40 && b > g + 40 {
            magenta_px += 1;
        }
    }

    if let Some(path) = out_png {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let img = image::RgbaImage::from_raw(W, H, bytes)
            .ok_or_else(|| "readback byte length mismatch".to_string())?;
        img.save(path)
            .map_err(|e| format!("png write failed: {e}"))?;
    }

    Ok(ChiselSmokeOutcome {
        width: W,
        height: H,
        leaf_boxes: boxes.len(),
        leaves_painted,
        blue_pixels: blue,
        green_pixels: green,
        magenta_pixels: magenta_px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First pixels: all three leaves render through the retained-session leaf
    /// path, each identifiable by its dominant color in the readback. Writes
    /// the PNG receipt to the shared testing media tree.
    #[test]
    fn chisel_leaves_render_to_pixels() {
        let receipt = std::path::Path::new(
            "C:/Users/mark_/Code/testing/genet/images/2026-07-08_chisel_first_pixels.png",
        );
        let out = run_chisel_smoke(Some(receipt)).expect("smoke renders");
        assert_eq!(out.leaf_boxes, 3, "layout seam found all leaves: {out:?}");
        assert_eq!(
            out.leaves_painted, 3,
            "render gate painted all leaves: {out:?}"
        );
        assert!(out.blue_pixels > 20, "knob value arc visible: {out:?}");
        assert!(out.green_pixels > 40, "meter fill visible: {out:?}");
        assert!(
            out.magenta_pixels > 10,
            "graph glyph nodes visible: {out:?}"
        );
    }
}
