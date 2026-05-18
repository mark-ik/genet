/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `PaintList` → `netrender::Scene` translator.
//!
//! Producers emit a [`paint_list_api::PaintList`] (the closed-set
//! `PaintCmd` vocabulary — compositor primitives plus `Draw*` items);
//! this module walks the command stream and produces a
//! [`netrender::Scene`] the renderer can rasterize.
//!
//! ## Mapping summary
//!
//! Most variants map 1:1 to a `netrender::SceneOp`. The painter-side
//! gaps flagged for follow-up (`DrawText`, `DrawImage`,
//! `DrawRepeatingImage`, `DrawShadow`, `DrawPath`, `DrawStroke`,
//! nine-patch borders) emit a fallback or `warn!`-and-skip; the
//! proper translation lands when the corresponding paint-side state
//! (font registry, image registry, shadow-mask integration) wires
//! through. The lowering itself is well-defined for the deferred
//! variants — only the painter-side resource plumbing is missing.

use log::warn;
use netrender::{
    ExternalTexturePlacement, GradientKind, GradientStop as NrGradientStop, NO_CLIP, Scene,
    SceneBlendMode, SceneClip, SceneLayer, Transform,
};
use paint_list_api::{self as ple, PaintCmd, PaintList};
use paint_types::ColorF;

#[derive(Clone, Debug)]
pub(crate) struct ExternalTextureDraw {
    pub texture_key: u64,
    pub placement: ExternalTexturePlacement,
    /// Number of ordinary NetRender ops emitted before this external
    /// texture draw. The renderer uses this to restore painter order
    /// without forcing the texture through Vello's atlas path.
    pub scene_op_boundary: usize,
}

pub(crate) struct TranslatedDisplayList {
    pub scene: Scene,
    pub external_textures: Vec<ExternalTextureDraw>,
}

// =============================================================================
// Shared utilities
// =============================================================================

fn rect_corners(rect: &paint_list_api::LayoutRect) -> (f32, f32, f32, f32) {
    (rect.min.x, rect.min.y, rect.max.x, rect.max.y)
}

fn color_to_array(color: &ColorF) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

fn layout_transform_to_scene(t: &paint_list_api::LayoutTransform) -> Transform {
    // PaintCmd carries `Transform3D` (4x4 column-major in euclid's
    // m11..m44 naming); netrender's `Transform.m` is also 4x4
    // column-major. Project field-by-field.
    Transform {
        m: [
            t.m11, t.m12, t.m13, t.m14, t.m21, t.m22, t.m23, t.m24, t.m31, t.m32, t.m33, t.m34,
            t.m41, t.m42, t.m43, t.m44,
        ],
    }
}

fn mix_blend_mode_to_scene(mode: paint_types::MixBlendMode) -> SceneBlendMode {
    use paint_types::MixBlendMode as M;
    match mode {
        M::Normal => SceneBlendMode::Normal,
        M::Multiply => SceneBlendMode::Multiply,
        M::Screen => SceneBlendMode::Screen,
        M::Overlay => SceneBlendMode::Overlay,
        M::Darken => SceneBlendMode::Darken,
        M::Lighten => SceneBlendMode::Lighten,
        // netrender's enum is the small CSS-canonical set; the
        // higher-fidelity modes (ColorDodge/ColorBurn/HardLight/etc.)
        // fall back to Normal until netrender grows full coverage.
        _ => SceneBlendMode::Normal,
    }
}

fn gradient_stops(stops: &[paint_types::GradientStop]) -> Vec<NrGradientStop> {
    stops
        .iter()
        .map(|s| NrGradientStop {
            offset: s.offset,
            color: [s.color.r, s.color.g, s.color.b, s.color.a],
        })
        .collect()
}

// =============================================================================
// PaintList → Scene entry points
// =============================================================================

/// Translate a [`PaintList`] into a [`netrender::Scene`]. External-
/// texture composite metadata stays renderer-private (used by
/// `Paint::render` to drive `render_with_compositor_and_external_textures`);
/// the public entry point returns just the Scene for testability.
pub fn translate_paint_list<L: PaintList>(list: &L) -> Scene {
    translate_paint_cmd_stream(list.viewport(), list.commands()).scene
}

/// Receive-side companion: translate a wire envelope. Thin wrapper
/// since `PaintEnvelope` itself impls `PaintList`.
pub fn translate_envelope(envelope: &paint_list_api::PaintEnvelope) -> Scene {
    translate_paint_list(envelope)
}

/// Internal variant that also returns the external-texture composite
/// list. Used by `Paint::render` to drive
/// `render_with_compositor_and_external_textures`.
pub(crate) fn translate_envelope_with_external_textures(
    envelope: &paint_list_api::PaintEnvelope,
) -> TranslatedDisplayList {
    translate_paint_cmd_stream(envelope.viewport(), envelope.commands())
}

/// Stream-form: take a viewport and a flat `PaintCmd` slice.
pub(crate) fn translate_paint_cmd_stream(
    viewport: paint_list_api::DeviceIntSize,
    commands: &[PaintCmd],
) -> TranslatedDisplayList {
    let viewport_w = viewport.width.max(0) as u32;
    let viewport_h = viewport.height.max(0) as u32;
    let mut scene = Scene::new(viewport_w, viewport_h);
    let mut external_textures = Vec::new();

    for cmd in commands {
        match cmd {
            // ----- Compositor primitives ---------------------------------
            PaintCmd::PushClip(spec) => emit_push_clip(&mut scene, spec),
            PaintCmd::PopClip => {
                // Clips ride on layers in netrender's model; PushClip pairs with PopLayer.
                scene.pop_layer();
            },
            PaintCmd::PushTransform(spec) => emit_push_transform(&mut scene, spec),
            PaintCmd::PopTransform => {
                // PushTransform → SceneLayer carrying the transform;
                // PopTransform returns to the parent layer.
                scene.pop_layer();
            },
            PaintCmd::PushLayer(spec) => emit_push_layer(&mut scene, spec),
            PaintCmd::PopLayer => {
                scene.pop_layer();
            },

            // ----- Paint primitives --------------------------------------
            PaintCmd::DrawRect(r) => {
                let (x0, y0, x1, y1) = rect_corners(&r.placement.bounds);
                scene.push_rect(x0, y0, x1, y1, color_to_array(&r.color));
            },
            PaintCmd::DrawStroke(_) => {
                // Stroke requires `kurbo::BezPath` reconstruction + a
                // netrender stroke primitive (`SceneStroke`).
                // Painter-side wiring needs the same plumbing as
                // DrawPath; deferred together.
                warn!("[paint translator] DrawStroke deferred (needs kurbo::BezPath wiring)");
            },
            PaintCmd::DrawLine(line) => {
                // First-cut: emit a solid rect spanning the line's
                // local bounds. Decorated styles (wavy/dotted/dashed)
                // need stroke variants.
                let (x0, y0, x1, y1) = rect_corners(&line.placement.bounds);
                scene.push_rect(x0, y0, x1, y1, color_to_array(&line.color));
            },
            PaintCmd::DrawPath(_) => {
                // PM-3 common variant; renderer side needs
                // kurbo::BezPath reconstruction from PathData + vello
                // path emission (netrender's `SceneOp::Shape` exists).
                warn!("[paint translator] DrawPath deferred (needs kurbo::BezPath wiring)");
            },
            PaintCmd::DrawBorder(border) => emit_border_first_cut(&mut scene, border),
            PaintCmd::DrawLinearGradient(g) => emit_linear_gradient(&mut scene, g),
            PaintCmd::DrawRadialGradient(g) => emit_radial_gradient(&mut scene, g),
            PaintCmd::DrawConicGradient(g) => emit_conic_gradient(&mut scene, g),
            PaintCmd::DrawText(_) => {
                // Needs FontRegistry to map FontInstanceKey →
                // netrender::FontId.
                warn!("[paint translator] DrawText deferred (needs FontRegistry wiring)");
            },
            PaintCmd::DrawImage(_) | PaintCmd::DrawRepeatingImage(_) => {
                // Needs ImageRegistry to map ImageKey → texture handle.
                warn!(
                    "[paint translator] DrawImage / DrawRepeatingImage deferred (needs ImageRegistry wiring)"
                );
            },
            PaintCmd::DrawExternalTexture(et) => {
                let (x0, y0, x1, y1) = rect_corners(&et.placement.bounds);
                external_textures.push(ExternalTextureDraw {
                    texture_key: et.texture_key,
                    placement: ExternalTexturePlacement::new([x0, y0, x1, y1])
                        .with_opacity(et.opacity),
                    scene_op_boundary: scene.ops.len(),
                });
            },
            PaintCmd::DrawShadow(_) => {
                // Box-shadow → needs `Renderer::build_box_shadow_mask`.
                warn!("[paint translator] DrawShadow deferred (needs build_box_shadow_mask)");
            },
            PaintCmd::PushShadow(_) | PaintCmd::PopAllShadows => {
                // State-stack pair; no-op until shadow integration lands.
            },
            PaintCmd::HitTest(_) => {
                // Hit-test items route to a separate netrender::hit_test
                // pass, not the Scene paint-order stream. No-op here.
            },
        }
    }

    TranslatedDisplayList {
        scene,
        external_textures,
    }
}

// =============================================================================
// PaintCmd per-variant emit helpers
// =============================================================================

fn emit_push_clip(scene: &mut Scene, spec: &ple::ClipSpec) {
    let clip = match &spec.kind {
        ple::ClipKind::Rect(rect) => {
            let (x0, y0, x1, y1) = rect_corners(rect);
            SceneClip::Rect {
                rect: [x0, y0, x1, y1],
                radii: [0.0, 0.0, 0.0, 0.0],
            }
        },
        ple::ClipKind::RoundedRect { rect, radius, .. } => {
            let (x0, y0, x1, y1) = rect_corners(rect);
            SceneClip::Rect {
                rect: [x0, y0, x1, y1],
                radii: [
                    radius.top_left.width,
                    radius.top_right.width,
                    radius.bottom_right.width,
                    radius.bottom_left.width,
                ],
            }
        },
        ple::ClipKind::Path(_) => {
            // Path clips need kurbo::BezPath reconstruction; same
            // deferred plumbing as DrawPath. Fall back to no-clip so
            // the layer still pushes and pairs correctly with PopClip.
            warn!("[paint translator] PushClip(Path) deferred; pushing unclipped layer");
            SceneClip::None
        },
    };
    scene.push_layer(SceneLayer {
        clip,
        alpha: 1.0,
        blend_mode: SceneBlendMode::Normal,
        compose: netrender::SceneCompose::SrcOver,
        transform_id: 0,
        backdrop_filter: None,
    });
}

fn emit_push_transform(scene: &mut Scene, spec: &ple::TransformSpec) {
    // Push the transform onto the Scene's transforms palette and wrap
    // in a layer that references it. The Scene model uses per-op
    // `transform_id` rather than push/pop semantics; the layer carries
    // the new transform_id so child ops pick it up.
    let nr_transform = layout_transform_to_scene(&spec.transform);
    // Translate by origin if non-zero. Compose origin into the
    // transform via pre-multiplication (translate then user transform).
    let composed = if spec.origin.x != 0.0 || spec.origin.y != 0.0 {
        compose_with_origin(&nr_transform, spec.origin.x, spec.origin.y)
    } else {
        nr_transform
    };
    scene.transforms.push(composed);
    let transform_id = (scene.transforms.len() - 1) as u32;
    scene.push_layer(SceneLayer {
        clip: SceneClip::None,
        alpha: 1.0,
        blend_mode: SceneBlendMode::Normal,
        compose: netrender::SceneCompose::SrcOver,
        transform_id,
        backdrop_filter: None,
    });
    // `spec.kind` (Standard / Preserve3D / Perspective) is recorded
    // for future stack-state handling; netrender treats the transform
    // as opaque math regardless.
    let _ = spec.kind;
}

fn compose_with_origin(t: &Transform, ox: f32, oy: f32) -> Transform {
    // The netrender Transform is a flat 16-float array, conceptually
    // row-major. The "translate by (ox, oy) then apply t" composition
    // is: t_with_translation[12] += ox, t_with_translation[13] += oy
    // (translation columns).
    let mut out = t.m;
    out[12] += ox;
    out[13] += oy;
    Transform { m: out }
}

fn emit_push_layer(scene: &mut Scene, spec: &ple::LayerSpec) {
    let blend_mode = mix_blend_mode_to_scene(spec.mix_blend_mode);
    let mut alpha = spec.opacity;
    // Filter-chain opacity collapses into the layer's alpha; other
    // filters need backdrop machinery and are deferred.
    for filter in &spec.filters {
        if let ple::FilterOp::Opacity(a) = filter {
            alpha *= *a;
        }
    }
    let _ = spec.raster_space; // Local vs Screen — deferred
    let _ = spec.flags;        // BLEND_CONTAINER etc. — deferred
    let _ = &spec.mask;        // alpha-mask layer — deferred
    scene.push_layer(SceneLayer {
        clip: SceneClip::None,
        alpha,
        blend_mode,
        compose: netrender::SceneCompose::SrcOver,
        transform_id: 0,
        backdrop_filter: None,
    });
}

fn emit_border_first_cut(scene: &mut Scene, border: &ple::BorderItem) {
    let rect = &border.placement.bounds;
    let widths = &border.widths;
    let sides = match &border.details {
        ple::BorderDetails::Normal(n) => n,
        ple::BorderDetails::NinePatch(_) => {
            warn!("[paint translator] nine-patch border deferred");
            return;
        },
    };
    if widths.top > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.min.y,
            rect.max.x,
            rect.min.y + widths.top,
            color_to_array(&sides.top.color),
        );
    }
    if widths.bottom > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.max.y - widths.bottom,
            rect.max.x,
            rect.max.y,
            color_to_array(&sides.bottom.color),
        );
    }
    if widths.left > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.min.y,
            rect.min.x + widths.left,
            rect.max.y,
            color_to_array(&sides.left.color),
        );
    }
    if widths.right > 0.0 {
        scene.push_rect(
            rect.max.x - widths.right,
            rect.min.y,
            rect.max.x,
            rect.max.y,
            color_to_array(&sides.right.color),
        );
    }
    let _ = sides.radius;
    let _ = sides.do_aa;
}

fn emit_linear_gradient(scene: &mut Scene, item: &ple::LinearGradientItem) {
    let rect = &item.placement.bounds;
    let g = &item.gradient;
    scene.push_gradient(netrender::SceneGradient {
        x0: rect.min.x,
        y0: rect.min.y,
        x1: rect.max.x,
        y1: rect.max.y,
        kind: GradientKind::Linear,
        params: [
            g.start_point.x,
            g.start_point.y,
            g.end_point.x,
            g.end_point.y,
        ],
        stops: gradient_stops(&g.stops),
        transform_id: 0,
        clip_rect: NO_CLIP,
        clip_corner_radii: [0.0; 4],
    });
}

fn emit_radial_gradient(scene: &mut Scene, item: &ple::RadialGradientItem) {
    let rect = &item.placement.bounds;
    let g = &item.gradient;
    scene.push_gradient(netrender::SceneGradient {
        x0: rect.min.x,
        y0: rect.min.y,
        x1: rect.max.x,
        y1: rect.max.y,
        kind: GradientKind::Radial,
        params: [g.center.x, g.center.y, g.radius.width, g.radius.height],
        stops: gradient_stops(&g.stops),
        transform_id: 0,
        clip_rect: NO_CLIP,
        clip_corner_radii: [0.0; 4],
    });
}

fn emit_conic_gradient(scene: &mut Scene, item: &ple::ConicGradientItem) {
    let rect = &item.placement.bounds;
    let g = &item.gradient;
    scene.push_gradient(netrender::SceneGradient {
        x0: rect.min.x,
        y0: rect.min.y,
        x1: rect.max.x,
        y1: rect.max.y,
        kind: GradientKind::Conic,
        params: [g.center.x, g.center.y, g.angle, 0.0],
        stops: gradient_stops(&g.stops),
        transform_id: 0,
        clip_rect: NO_CLIP,
        clip_corner_radii: [0.0; 4],
    });
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use malloc_size_of_derive::MallocSizeOf;
    use paint_list_api::{
        CommonPlacement, DeviceIntSize, EngineId, LayoutPoint, LayoutRect, PaintCmd, PaintList,
        PrimitiveFlags, RectItem,
    };
    use paint_types::ColorF;
    use serde::{Deserialize, Serialize};

    use super::*;

    /// Minimal `PaintList` impl for driving `translate_paint_list`
    /// from tests without pulling in a producer crate.
    #[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, Serialize)]
    struct StubPaintList {
        viewport: DeviceIntSize,
        commands: Vec<PaintCmd>,
    }

    impl PaintList for StubPaintList {
        fn engine_id(&self) -> EngineId {
            EngineId::SERVAL
        }
        fn viewport(&self) -> DeviceIntSize {
            self.viewport
        }
        fn generation_id(&self) -> u64 {
            0
        }
        fn commands(&self) -> &[PaintCmd] {
            &self.commands
        }
    }

    fn box2d(x: f32, y: f32, w: f32, h: f32) -> LayoutRect {
        LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
    }

    fn placement_at(bounds: LayoutRect) -> CommonPlacement {
        CommonPlacement {
            bounds,
            flags: PrimitiveFlags::empty(),
        }
    }

    fn list_with(viewport: DeviceIntSize, cmds: Vec<PaintCmd>) -> StubPaintList {
        StubPaintList {
            viewport,
            commands: cmds,
        }
    }

    #[test]
    fn empty_list_translates_to_empty_scene() {
        let list = list_with(DeviceIntSize::new(800, 600), Vec::new());
        let scene = translate_paint_list(&list);
        assert_eq!(scene.viewport_width, 800);
        assert_eq!(scene.viewport_height, 600);
        assert_eq!(scene.ops.len(), 0);
    }

    #[test]
    fn draw_rect_emits_scene_rect() {
        let list = list_with(
            DeviceIntSize::new(800, 600),
            vec![PaintCmd::DrawRect(RectItem {
                placement: placement_at(box2d(10.0, 20.0, 100.0, 50.0)),
                color: ColorF {
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
            })],
        );
        let scene = translate_paint_list(&list);
        assert_eq!(scene.ops.len(), 1);
        assert!(matches!(scene.ops[0], netrender::SceneOp::Rect(_)));
    }

    #[test]
    fn push_pop_layer_emits_layer_pair() {
        let list = list_with(
            DeviceIntSize::new(800, 600),
            vec![
                PaintCmd::PushLayer(ple::LayerSpec {
                    opacity: 0.5,
                    ..ple::LayerSpec::default()
                }),
                PaintCmd::PopLayer,
            ],
        );
        let scene = translate_paint_list(&list);
        assert_eq!(scene.ops.len(), 2);
        assert!(matches!(scene.ops[0], netrender::SceneOp::PushLayer(_)));
        assert!(matches!(scene.ops[1], netrender::SceneOp::PopLayer));
    }

    #[test]
    fn push_pop_transform_emits_layer_pair_with_transform_id() {
        let list = list_with(
            DeviceIntSize::new(800, 600),
            vec![
                PaintCmd::PushTransform(ple::TransformSpec {
                    origin: LayoutPoint::new(10.0, 20.0),
                    transform: paint_list_api::LayoutTransform::identity(),
                    kind: ple::TransformKind::Standard,
                }),
                PaintCmd::PopTransform,
            ],
        );
        let scene = translate_paint_list(&list);
        // Push emits one transform palette entry beyond identity, plus
        // a PushLayer carrying that transform_id; Pop emits PopLayer.
        assert!(
            scene.transforms.len() >= 2,
            "transforms: {:?}",
            scene.transforms
        );
        assert_eq!(scene.ops.len(), 2);
        let push = match &scene.ops[0] {
            netrender::SceneOp::PushLayer(l) => l,
            other => panic!("expected PushLayer, got {other:?}"),
        };
        assert!(
            push.transform_id > 0,
            "transform_id should reference new entry"
        );
        assert!(matches!(scene.ops[1], netrender::SceneOp::PopLayer));
    }

    #[test]
    fn push_clip_rect_emits_clipped_layer() {
        let list = list_with(
            DeviceIntSize::new(800, 600),
            vec![
                PaintCmd::PushClip(ple::ClipSpec {
                    kind: ple::ClipKind::Rect(box2d(0.0, 0.0, 100.0, 100.0)),
                }),
                PaintCmd::PopClip,
            ],
        );
        let scene = translate_paint_list(&list);
        assert_eq!(scene.ops.len(), 2);
        let layer = match &scene.ops[0] {
            netrender::SceneOp::PushLayer(l) => l,
            other => panic!("expected PushLayer, got {other:?}"),
        };
        assert!(matches!(layer.clip, netrender::SceneClip::Rect { .. }));
        assert!(matches!(scene.ops[1], netrender::SceneOp::PopLayer));
    }

    #[test]
    fn external_texture_routes_to_external_textures_vec() {
        use paint_list_api::ExternalTextureItem;
        let list = list_with(
            DeviceIntSize::new(800, 600),
            vec![PaintCmd::DrawExternalTexture(ExternalTextureItem {
                placement: placement_at(box2d(0.0, 0.0, 200.0, 200.0)),
                texture_key: 0xC0FFEE,
                opacity: 0.75,
                content_generation: None,
            })],
        );
        // External texture metadata lives on the pub(crate) full-shape
        // translator output; use translate_paint_cmd_stream to inspect it.
        let out = translate_paint_cmd_stream(list.viewport, &list.commands);
        // External texture doesn't add to scene.ops; it goes into the
        // separate compositor vector via the PM-3 lowering contract.
        assert_eq!(out.scene.ops.len(), 0);
        assert_eq!(out.external_textures.len(), 1);
        assert_eq!(out.external_textures[0].texture_key, 0xC0FFEE);
        assert_eq!(out.external_textures[0].scene_op_boundary, 0);
    }
}
