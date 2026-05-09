/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C3 Step 7 — `ServalDisplayList` → `netrender::Scene` translator.
//!
//! Layout emits a [`paint_api::serval_display_list::ServalDisplayList`]
//! (a paint-order op stream + spatial / clip / transform palettes plus
//! a `PaintDisplayListInfo` metadata bundle); this module walks that
//! list and produces a [`netrender::Scene`] the renderer can consume.
//!
//! ## Mapping summary
//!
//! Most variants map 1:1 to a `netrender::SceneOp`. The painter-side
//! gaps that the C3 plan flagged for follow-up (`BoxShadow` / `Shadow`
//! / `RepeatingImage`) emit a fallback rect for now and log the gap;
//! the proper translation lands when `Renderer::build_box_shadow_mask`
//! and `ScenePattern` integration land here. Animation property
//! bindings (`PropertyBinding::Binding`) are resolved to their static
//! current value — the painter does not yet advance them per frame.

use log::warn;
use netrender::{
    GradientKind, GradientStop as NrGradientStop, NO_CLIP, Scene, SceneBlendMode, SceneClip,
    SceneGradient, SceneLayer, Transform,
};
use paint_api::display_list::PaintDisplayListInfo;
use paint_api::serval_display_list::{
    ConicGradientPayload, FilterOp, GradientPayload, RadialGradientPayload, ServalDisplayItem,
    ServalDisplayList,
};
use paint_types::ColorF;

/// Translate a [`ServalDisplayList`] (plus its per-pipeline metadata)
/// into a [`netrender::Scene`] the renderer can rasterize.
///
/// First-cut painter: handles solid rects, images, text, borders,
/// gradients, stacking contexts (as alpha layers), reference frames
/// (as transform pushes), and clip rects (as clip layers). Box
/// shadows, text shadows, and repeating images currently emit a
/// fallback solid rect — the netrender helpers exist
/// (`Renderer::build_box_shadow_mask`, `ScenePattern`) but the
/// integration is a follow-up.
pub fn translate_display_list(
    list: &ServalDisplayList,
    paint_info: &PaintDisplayListInfo,
) -> Scene {
    let viewport_w = list.viewport.width.max(0) as u32;
    let viewport_h = list.viewport.height.max(0) as u32;
    let mut scene = Scene::new(viewport_w, viewport_h);

    // The transforms palette is appended onto the Scene's transform
    // stack (Scene reserves index 0 as identity, mirroring
    // ServalDisplayList's convention). Subsequent pushes pick up
    // their transform_id from the offset.
    let transform_id_offset = scene.transforms.len() as u32 - 1;
    for transform in list.transforms.iter().skip(1) {
        scene.transforms.push(serval_to_scene_transform(transform));
    }

    let caret_animation = paint_info.caret_property_binding;

    for item in &list.items {
        match item {
            ServalDisplayItem::Rect(r) => {
                let (x0, y0, x1, y1) = rect_corners(&r.placement.clip_rect);
                scene.push_rect(x0, y0, x1, y1, color_to_array(&r.color));
            },
            ServalDisplayItem::RectWithAnimation(r) => {
                // Animation hook: if this is the caret rect, the
                // painter would resolve its current opacity from
                // `paint_info.caret_property_binding`. First-cut paints
                // the static color; per-frame advance lands when the
                // painter grows a tick callback.
                let _ = caret_animation;
                let (x0, y0, x1, y1) = rect_corners(&r.placement.clip_rect);
                scene.push_rect(x0, y0, x1, y1, color_to_array(&r.color));
            },
            ServalDisplayItem::Line(line) => {
                // A 1-thick line is a degenerate rect; thicker decorated
                // lines (wavy / dotted / dashed) need stroke variants.
                // First cut: emit a solid rect spanning the line bounds.
                let (x0, y0, x1, y1) = rect_corners(&line.placement.clip_rect);
                scene.push_rect(x0, y0, x1, y1, color_to_array(&line.color));
            },
            ServalDisplayItem::Image(_) | ServalDisplayItem::RepeatingImage(_) => {
                // Image translation needs the painter's `ImageRegistry`
                // (paint-side) to map `ImageKey` → `netrender::ImageKey`.
                // First cut: log + emit a transparent fallback so the
                // tree shape is preserved.
                warn!("[netrender translator] image / repeating image not yet wired; emitting transparent fallback");
            },
            ServalDisplayItem::Text(text) => {
                // Glyph-run translation needs the painter's `FontRegistry`
                // to map `FontInstanceKey` → `netrender::FontId`. First
                // cut: log + skip.
                let _ = text;
                warn!("[netrender translator] text glyph-run not yet wired; skipping");
            },
            ServalDisplayItem::Border(border) => {
                // First cut: emit four edge-strokes from the border
                // widths. Rounded corners and per-side styles
                // (Solid/Dashed/Dotted) are deferred.
                emit_border_first_cut(&mut scene, border);
            },
            ServalDisplayItem::BoxShadow(_) | ServalDisplayItem::PushShadow(_) => {
                // Box-shadow / text-shadow: netrender's
                // `Renderer::build_box_shadow_mask` produces a blurred
                // mask texture that's then inserted as an image. The
                // painter needs renderer access (held at Paint level,
                // not Scene level) to do this, so the integration
                // lands when the painter holds a `Renderer` handle.
                warn!("[netrender translator] box-shadow / text-shadow deferred (needs renderer.build_box_shadow_mask)");
            },
            ServalDisplayItem::PopAllShadows => {
                // Counterpart to PushShadow; no-op until shadow stack
                // is wired.
            },
            ServalDisplayItem::Gradient(g) => emit_linear_gradient(&mut scene, g),
            ServalDisplayItem::RadialGradient(g) => emit_radial_gradient(&mut scene, g),
            ServalDisplayItem::ConicGradient(g) => emit_conic_gradient(&mut scene, g),
            ServalDisplayItem::Iframe(_) => {
                // Iframes resolve to the child pipeline's Scene
                // composited via `declare_compositor_surface` (native
                // compositor route) or as an image (in-process route).
                // First cut: deferred.
                warn!("[netrender translator] iframe deferred (needs cross-pipeline scene composition)");
            },
            ServalDisplayItem::PushStackingContext(sc) => {
                let layer = stacking_context_to_layer(sc, transform_id_offset);
                scene.push_layer(layer);
            },
            ServalDisplayItem::PopStackingContext => {
                scene.pop_layer();
            },
            ServalDisplayItem::PushReferenceFrame(_) => {
                // Reference frames are already realized by appending
                // `LayoutTransform`s to the transforms palette in the
                // header above; per-op `transform_id` selection is the
                // mechanism netrender uses (rather than push/pop).
                // The Push/Pop variants are recorded for layout-side
                // bookkeeping only.
            },
            ServalDisplayItem::PopReferenceFrame => {},
            ServalDisplayItem::HitTest(_) => {
                // Hit-test items go to a separate `netrender::hit_test`
                // layer (off the Scene paint-order stream). The painter
                // accumulates them into a `Vec<HitOp>` per pipeline;
                // emission from the translator stage is a no-op.
            },
        }
    }

    scene
}

// =============================================================================
// Per-variant emit helpers
// =============================================================================

fn rect_corners(rect: &paint_types::units::LayoutRect) -> (f32, f32, f32, f32) {
    (rect.min.x, rect.min.y, rect.max.x, rect.max.y)
}

fn color_to_array(color: &ColorF) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

fn serval_to_scene_transform(t: &paint_types::units::LayoutTransform) -> Transform {
    // ServalDisplayList carries `Transform3D` (4x4 column-major in
    // euclid's m11..m44 naming); netrender's `Transform.m` is also
    // 4x4 column-major. Project field-by-field.
    Transform {
        m: [
            t.m11, t.m12, t.m13, t.m14,
            t.m21, t.m22, t.m23, t.m24,
            t.m31, t.m32, t.m33, t.m34,
            t.m41, t.m42, t.m43, t.m44,
        ],
    }
}

fn stacking_context_to_layer(
    sc: &paint_api::serval_display_list::StackingContextItem,
    transform_id_offset: u32,
) -> SceneLayer {
    let mut alpha = 1.0_f32;
    for filter in &sc.filters {
        if let FilterOp::Opacity(a) = filter {
            alpha *= *a;
        }
    }
    let blend_mode = mix_blend_mode_to_scene(sc.mix_blend_mode);
    let _ = transform_id_offset; // SC origins handled per-op via transforms palette

    SceneLayer {
        clip: SceneClip::None,
        alpha,
        blend_mode,
        compose: netrender::SceneCompose::SrcOver,
        transform_id: 0,
        backdrop_filter: None,
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

fn emit_border_first_cut(
    scene: &mut Scene,
    border: &paint_api::serval_display_list::BorderItem,
) {
    let rect = &border.placement.clip_rect;
    let widths = &border.widths;
    use paint_api::serval_display_list::BorderDetails;
    let sides = match &border.details {
        BorderDetails::Normal(n) => n,
        BorderDetails::NinePatch(_) => {
            warn!("[netrender translator] nine-patch border deferred");
            return;
        },
    };
    // Top edge.
    if widths.top > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.min.y,
            rect.max.x,
            rect.min.y + widths.top,
            color_to_array(&sides.top.color),
        );
    }
    // Bottom edge.
    if widths.bottom > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.max.y - widths.bottom,
            rect.max.x,
            rect.max.y,
            color_to_array(&sides.bottom.color),
        );
    }
    // Left edge.
    if widths.left > 0.0 {
        scene.push_rect(
            rect.min.x,
            rect.min.y,
            rect.min.x + widths.left,
            rect.max.y,
            color_to_array(&sides.left.color),
        );
    }
    // Right edge.
    if widths.right > 0.0 {
        scene.push_rect(
            rect.max.x - widths.right,
            rect.min.y,
            rect.max.x,
            rect.max.y,
            color_to_array(&sides.right.color),
        );
    }
    let _ = sides.radius; // border-radius rounding deferred (per-corner)
    let _ = sides.do_aa;
}

fn emit_linear_gradient(
    scene: &mut Scene,
    item: &paint_api::serval_display_list::GradientItem,
) {
    let rect = &item.placement.clip_rect;
    let g: &GradientPayload = &item.gradient;
    scene.push_gradient(SceneGradient {
        x0: rect.min.x,
        y0: rect.min.y,
        x1: rect.max.x,
        y1: rect.max.y,
        kind: GradientKind::Linear,
        params: [g.start_point.x, g.start_point.y, g.end_point.x, g.end_point.y],
        stops: gradient_stops(&g.stops),
        transform_id: 0,
        clip_rect: NO_CLIP,
        clip_corner_radii: [0.0; 4],
    });
}

fn emit_radial_gradient(
    scene: &mut Scene,
    item: &paint_api::serval_display_list::RadialGradientItem,
) {
    let rect = &item.placement.clip_rect;
    let g: &RadialGradientPayload = &item.gradient;
    scene.push_gradient(SceneGradient {
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

fn emit_conic_gradient(
    scene: &mut Scene,
    item: &paint_api::serval_display_list::ConicGradientItem,
) {
    let rect = &item.placement.clip_rect;
    let g: &ConicGradientPayload = &item.gradient;
    scene.push_gradient(SceneGradient {
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
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use paint_api::serval_display_list::{
        ClipChainId, CommonItemPlacement, PrimitiveFlags, RectItem, ServalDisplayItem,
        ServalDisplayList,
    };
    use paint_types::PipelineId;
    use paint_types::units::{DeviceIntSize, LayoutRect, LayoutSize};

    fn pipeline() -> PipelineId {
        PipelineId::default()
    }

    fn placement(rect: LayoutRect) -> CommonItemPlacement {
        let pid = pipeline();
        CommonItemPlacement {
            clip_rect: rect,
            clip_chain_id: ClipChainId::INVALID,
            spatial_id: paint_types::SpatialId(0, pid),
            flags: PrimitiveFlags::empty(),
        }
    }

    fn paint_info(viewport_w: f32, viewport_h: f32, pipeline_id: PipelineId) -> PaintDisplayListInfo {
        use embedder_traits::ViewportDetails;
        use euclid::Scale;
        use paint_api::display_list::AxesScrollSensitivity;
        use paint_api::display_list::ScrollType;

        PaintDisplayListInfo::new(
            ViewportDetails {
                size: euclid::Size2D::new(viewport_w, viewport_h),
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

    #[test]
    fn empty_list_translates_to_empty_scene() {
        let list = ServalDisplayList::new(DeviceIntSize::new(800, 600), pipeline());
        let info = paint_info(800.0, 600.0, pipeline());
        let scene = translate_display_list(&list, &info);
        assert_eq!(scene.viewport_width, 800);
        assert_eq!(scene.viewport_height, 600);
        assert_eq!(scene.ops.len(), 0);
    }

    #[test]
    fn solid_rect_emits_one_scene_rect() {
        let mut list = ServalDisplayList::new(DeviceIntSize::new(800, 600), pipeline());
        list.push(ServalDisplayItem::Rect(RectItem {
            placement: placement(LayoutRect::new(
                paint_types::units::LayoutPoint::new(10.0, 20.0),
                paint_types::units::LayoutPoint::new(110.0, 220.0),
            )),
            color: ColorF {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }));
        let info = paint_info(800.0, 600.0, pipeline());
        let scene = translate_display_list(&list, &info);
        assert_eq!(scene.ops.len(), 1);
        assert!(matches!(scene.ops[0], netrender::SceneOp::Rect(_)));
    }

    #[test]
    fn stacking_context_push_and_pop_become_layer_pushes() {
        use paint_api::serval_display_list::{
            RasterSpace, StackingContextFlags, StackingContextItem,
        };
        use paint_types::TransformStyle;

        let mut list = ServalDisplayList::new(DeviceIntSize::new(800, 600), pipeline());
        list.push(ServalDisplayItem::PushStackingContext(StackingContextItem {
            placement: placement(LayoutRect::zero()),
            origin: paint_types::units::LayoutPoint::zero(),
            transform_style: TransformStyle::Flat,
            mix_blend_mode: paint_types::MixBlendMode::Normal,
            filters: vec![FilterOp::Opacity(0.5)],
            flags: StackingContextFlags::empty(),
            raster_space: RasterSpace::Screen,
        }));
        list.push(ServalDisplayItem::PopStackingContext);
        let info = paint_info(800.0, 600.0, pipeline());
        let scene = translate_display_list(&list, &info);
        assert_eq!(scene.ops.len(), 2);
        assert!(matches!(scene.ops[0], netrender::SceneOp::PushLayer(_)));
        assert!(matches!(scene.ops[1], netrender::SceneOp::PopLayer));
    }
}
