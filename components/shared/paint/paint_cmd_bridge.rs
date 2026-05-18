/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Producer-side bridge: lowers `ServalDisplayItem` variants to
//! `paint_list_api::PaintCmd` per the PM-3 audit table in
//! [docs/2026-05-17_paintlist_polyglot_renderer.md](../../../docs/2026-05-17_paintlist_polyglot_renderer.md).
//!
//! ## Scope
//!
//! Per-item lowering — given one `ServalDisplayItem`, returns the
//! corresponding `PaintCmd`(s). State-management (active clip palette
//! lookup, transform palette resolution, layered iframe pipeline
//! coordination) is **caller-side**; the functions here operate on
//! the renderable fields of each variant. The eventual full-list
//! walker (which threads clip / transform / spatial palettes through
//! a single pass over `ServalDisplayList.items`) is a separate
//! follow-up to this module.
//!
//! ## Why per-item first
//!
//! The PM-3 audit asserted each `ServalDisplayItem` variant collapses
//! to common `PaintCmd` ops (no `Extension` variant needed for v1).
//! This module is the validator for that assertion: every variant in
//! the audit table has a concrete mapping here, and if the mapping
//! has friction the audit needs revisiting before sinking work into
//! `ServalDisplayList` → `ServalPaintList` rename + full migration.
//!
//! ## Known TODOs the audit surfaced (logged inline in code)
//!
//! - `Iframe`: needs a producer-supplied `pipeline_id → texture_key`
//!   resolver. The placeholder strategy here records the pipeline id
//!   bytes directly into the texture_key for diagnosability.
//! - `PushReferenceFrame`: needs the `transforms` palette to resolve
//!   `ReferenceFrameId` to a concrete `LayoutTransform`. Functions
//!   that need it take the palette as an argument.
//! - `PushStackingContext` with `transform_style: Preserve3D` or
//!   `raster_space: Local`: dropped for v1 with `TransformStyle::Flat`
//!   / `RasterSpace::Screen` semantics. Real 3D / SVG-local support
//!   needs `LayerSpec` extensions; deferred until consumer pull.

use paint_list_api as ple;
use paint_types::PipelineId;
use paint_types::units::LayoutTransform;

use crate::serval_display_list as sdl;

// =============================================================================
// Top-level dispatch
// =============================================================================

/// Palettes needed to resolve indices referenced by item placements
/// (clip chains, spatial nodes, transforms). The bridge functions
/// take what they need; this struct bundles them for the top-level
/// dispatcher.
pub struct LowerContext<'a> {
    pub clip_defs: &'a [sdl::ClipDef],
    pub spatial_nodes: &'a [sdl::SpatialNodeDef],
    pub transforms: &'a [LayoutTransform],
}

/// Lower one `ServalDisplayItem` to its `PaintCmd` representation.
/// Most variants produce exactly one `PaintCmd`; a few
/// (state-stack pushes that also imply a clip change) may want
/// multiple — the eventual full-list walker will sequence those.
pub fn lower_item(item: &sdl::ServalDisplayItem, ctx: &LowerContext) -> ple::PaintCmd {
    use sdl::ServalDisplayItem as S;
    match item {
        S::Rect(it) => ple::PaintCmd::DrawRect(lower_rect(it)),
        S::RectWithAnimation(it) => ple::PaintCmd::DrawRect(lower_rect_with_animation(it)),
        S::Line(it) => ple::PaintCmd::DrawLine(lower_line(it)),
        S::Image(it) => ple::PaintCmd::DrawImage(lower_image(it)),
        S::ExternalTexture(it) => ple::PaintCmd::DrawExternalTexture(lower_external_texture(it)),
        S::RepeatingImage(it) => ple::PaintCmd::DrawRepeatingImage(lower_repeating_image(it)),
        S::Text(it) => ple::PaintCmd::DrawText(lower_text(it)),
        S::Border(it) => ple::PaintCmd::DrawBorder(lower_border(it)),
        S::BoxShadow(it) => ple::PaintCmd::DrawShadow(lower_box_shadow(it)),
        S::PushShadow(it) => ple::PaintCmd::PushShadow(lower_push_shadow(it)),
        S::PopAllShadows => ple::PaintCmd::PopAllShadows,
        S::Gradient(it) => ple::PaintCmd::DrawLinearGradient(lower_linear_gradient(it)),
        S::RadialGradient(it) => ple::PaintCmd::DrawRadialGradient(lower_radial_gradient(it)),
        S::ConicGradient(it) => ple::PaintCmd::DrawConicGradient(lower_conic_gradient(it)),
        S::Iframe(it) => ple::PaintCmd::DrawExternalTexture(lower_iframe(it)),
        S::PushStackingContext(it) => ple::PaintCmd::PushLayer(lower_push_stacking_context(it)),
        S::PopStackingContext => ple::PaintCmd::PopLayer,
        S::PushReferenceFrame(it) => {
            ple::PaintCmd::PushTransform(lower_push_reference_frame(it, ctx.transforms))
        },
        S::PopReferenceFrame => ple::PaintCmd::PopTransform,
        S::HitTest(it) => ple::PaintCmd::HitTest(lower_hit_test(it)),
    }
}

// =============================================================================
// Placement + flag helpers
// =============================================================================

/// `ServalDisplayItem::CommonItemPlacement` carries clip_chain_id and
/// spatial_id references that the `PaintCmd` model handles via
/// compositor primitives (`PushClip` / `PushTransform`), not per-item
/// state. The renderable fields that survive into `PaintCmd` are
/// `clip_rect` (used as the item's local bounds) and `flags`.
fn placement(p: &sdl::CommonItemPlacement) -> ple::CommonPlacement {
    ple::CommonPlacement {
        bounds: p.clip_rect,
        flags: convert_flags(p.flags),
    }
}

fn convert_flags(flags: sdl::PrimitiveFlags) -> ple::PrimitiveFlags {
    let mut out = ple::PrimitiveFlags::empty();
    if flags.contains(sdl::PrimitiveFlags::HIT_TESTABLE) {
        out |= ple::PrimitiveFlags::HIT_TESTABLE;
    }
    if flags.contains(sdl::PrimitiveFlags::IS_BACKFACE) {
        out |= ple::PrimitiveFlags::IS_BACKFACE;
    }
    if flags.contains(sdl::PrimitiveFlags::ANTIALIASED) {
        out |= ple::PrimitiveFlags::ANTIALIASED;
    }
    out
}

// =============================================================================
// Per-variant lowering
// =============================================================================

fn lower_rect(item: &sdl::RectItem) -> ple::RectItem {
    ple::RectItem {
        placement: placement(&item.placement),
        color: item.color,
    }
}

/// `RectWithAnimation`'s `animation` field is documented in
/// `serval_display_list.rs` as a stub always emitting `None` ("downstream
/// painter ignores"); per the PM-3 audit it collapses to a plain
/// `DrawRect`. If real animation property-binding lands later, this
/// is the call site to extend.
fn lower_rect_with_animation(item: &sdl::RectAnimItem) -> ple::RectItem {
    ple::RectItem {
        placement: placement(&item.placement),
        color: item.color,
    }
}

fn lower_line(item: &sdl::LineItem) -> ple::LineItem {
    ple::LineItem {
        placement: placement(&item.placement),
        color: item.color,
        style: item.style,
        orientation: match item.orientation {
            sdl::LineOrientation::Horizontal => ple::LineOrientation::Horizontal,
            sdl::LineOrientation::Vertical => ple::LineOrientation::Vertical,
        },
        wavy_thickness: item.wavy_thickness,
    }
}

fn lower_image(item: &sdl::ImageItem) -> ple::ImageItem {
    ple::ImageItem {
        placement: placement(&item.placement),
        image_key: item.image_key,
        image_rendering: item.image_rendering,
        alpha_type: convert_alpha_type(item.alpha_type),
        color: item.color,
    }
}

fn lower_repeating_image(item: &sdl::RepeatingImageItem) -> ple::RepeatingImageItem {
    ple::RepeatingImageItem {
        placement: placement(&item.placement),
        image_key: item.image_key,
        stretch_size: item.stretch_size,
        tile_spacing: item.tile_spacing,
        image_rendering: item.image_rendering,
        alpha_type: convert_alpha_type(item.alpha_type),
        color: item.color,
    }
}

fn lower_external_texture(item: &sdl::ExternalTextureItem) -> ple::ExternalTextureItem {
    ple::ExternalTextureItem {
        placement: placement(&item.placement),
        texture_key: item.texture_key,
        opacity: item.opacity,
        // PM-3 compositor-pass lowering: producer-supplied textures
        // (WebGL canvas, native form controls, etc.) compose via the
        // per-frame pass with no tile-cache invalidation needed.
        content_generation: None,
    }
}

fn lower_text(item: &sdl::TextItem) -> ple::TextRunItem {
    ple::TextRunItem {
        placement: placement(&item.placement),
        font_instance: item.font_instance,
        color: item.color,
        glyphs: item
            .glyphs
            .iter()
            .map(|g| ple::GlyphInstance {
                index: g.index,
                point: g.point,
            })
            .collect(),
        options: convert_glyph_options(item.glyph_options),
    }
}

fn lower_border(item: &sdl::BorderItem) -> ple::BorderItem {
    ple::BorderItem {
        placement: placement(&item.placement),
        widths: item.widths,
        details: match &item.details {
            sdl::BorderDetails::Normal(nb) => ple::BorderDetails::Normal(nb.clone()),
            sdl::BorderDetails::NinePatch(np) => {
                ple::BorderDetails::NinePatch(lower_nine_patch_border(np))
            },
        },
    }
}

fn lower_nine_patch_border(np: &sdl::NinePatchBorderDetails) -> ple::NinePatchBorder {
    ple::NinePatchBorder {
        source: match &np.source {
            sdl::NinePatchBorderSource::Image(key, rendering) => {
                ple::NinePatchSource::Image(*key, *rendering)
            },
            sdl::NinePatchBorderSource::Gradient(g) => {
                ple::NinePatchSource::LinearGradient(lower_linear_gradient_payload(g))
            },
            sdl::NinePatchBorderSource::RadialGradient(g) => {
                ple::NinePatchSource::RadialGradient(lower_radial_gradient_payload(g))
            },
            sdl::NinePatchBorderSource::ConicGradient(g) => {
                ple::NinePatchSource::ConicGradient(lower_conic_gradient_payload(g))
            },
        },
        width: np.width,
        height: np.height,
        slice: np.slice,
        fill: np.fill,
        repeat_horizontal: np.repeat_horizontal,
        repeat_vertical: np.repeat_vertical,
    }
}

fn lower_box_shadow(item: &sdl::BoxShadowItem) -> ple::ShadowItem {
    ple::ShadowItem {
        placement: placement(&item.placement),
        box_bounds: item.box_bounds,
        offset: item.offset,
        color: item.color,
        blur_radius: item.blur_radius,
        spread_radius: item.spread_radius,
        border_radius: item.border_radius,
        clip_mode: item.clip_mode,
    }
}

fn lower_push_shadow(item: &sdl::ShadowItem) -> ple::ShadowSpec {
    ple::ShadowSpec {
        offset: item.offset,
        color: item.color,
        blur_radius: item.blur_radius,
    }
}

fn lower_linear_gradient(item: &sdl::GradientItem) -> ple::LinearGradientItem {
    ple::LinearGradientItem {
        placement: placement(&item.placement),
        gradient: lower_linear_gradient_payload(&item.gradient),
        tile_size: item.tile_size,
        tile_spacing: item.tile_spacing,
    }
}

fn lower_linear_gradient_payload(g: &sdl::GradientPayload) -> ple::LinearGradientPayload {
    ple::LinearGradientPayload {
        start_point: g.start_point,
        end_point: g.end_point,
        extend_mode: g.extend_mode,
        stops: g.stops.clone(),
    }
}

fn lower_radial_gradient(item: &sdl::RadialGradientItem) -> ple::RadialGradientItem {
    ple::RadialGradientItem {
        placement: placement(&item.placement),
        gradient: lower_radial_gradient_payload(&item.gradient),
        tile_size: item.tile_size,
        tile_spacing: item.tile_spacing,
    }
}

fn lower_radial_gradient_payload(g: &sdl::RadialGradientPayload) -> ple::RadialGradientPayload {
    ple::RadialGradientPayload {
        center: g.center,
        radius: g.radius,
        extend_mode: g.extend_mode,
        stops: g.stops.clone(),
    }
}

fn lower_conic_gradient(item: &sdl::ConicGradientItem) -> ple::ConicGradientItem {
    ple::ConicGradientItem {
        placement: placement(&item.placement),
        gradient: lower_conic_gradient_payload(&item.gradient),
        tile_size: item.tile_size,
        tile_spacing: item.tile_spacing,
    }
}

fn lower_conic_gradient_payload(g: &sdl::ConicGradientPayload) -> ple::ConicGradientPayload {
    ple::ConicGradientPayload {
        center: g.center,
        angle: g.angle,
        extend_mode: g.extend_mode,
        stops: g.stops.clone(),
    }
}

/// Iframes lower to `DrawExternalTexture` per PM-3 — the embedded
/// pipeline's rendered output is a texture from the parent pipeline's
/// point of view.
///
/// **TODO (producer-pull):** the placeholder `texture_key` encoding
/// here packs the `PipelineId` bytes directly so the diagnostic is
/// readable; the real layout-side code needs to register the
/// iframe's render target with NetRender's external-texture registry
/// and supply the resulting stable key. Tracked at the `lower_iframe`
/// docstring and surfaced via `iframe_texture_key()`.
fn lower_iframe(item: &sdl::IframeItem) -> ple::ExternalTextureItem {
    ple::ExternalTextureItem {
        placement: placement(&item.placement),
        texture_key: iframe_texture_key(item.pipeline_id),
        opacity: 1.0,
        content_generation: None,
    }
}

/// Diagnosable-but-not-final encoding of a pipeline_id into an
/// external-texture key. See TODO on `lower_iframe`.
fn iframe_texture_key(pipeline_id: PipelineId) -> u64 {
    // PipelineId is a (namespace, index) pair. Pack into u64 so the
    // diagnostic carries both halves. Real impl resolves via the
    // external-texture registry.
    let bytes = format!("{pipeline_id:?}");
    let mut hash: u64 = 0xCBF29CE484222325; // FNV-1a basis
    for b in bytes.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001B3);
    }
    hash
}

/// PushStackingContext → PushLayer.
///
/// **Drops** (per PM-3 audit "CSS-3D and SVG-local are the only
/// genuinely Serval-only cases; defer until consumer pull"):
/// - `transform_style: Preserve3D` — silently treated as `Flat`. Real
///   3D rendering needs a `LayerSpec` extension or a parent
///   `TransformSpec` with `TransformKind::Preserve3D`. A debug warning
///   here would be appropriate once layout starts emitting 3D content.
/// - `raster_space: Local` — paint-list-api has `RasterSpace::Local`;
///   it does pass through.
/// - `snapshot` — not present in `StackingContextItem`'s actual
///   fields per the audit pass; the placeholder argument in
///   `push_stacking_context` is ignored.
fn lower_push_stacking_context(item: &sdl::StackingContextItem) -> ple::LayerSpec {
    // ServalDisplayList's `StackingContextFlags` is a bare `pub struct
    // StackingContextFlags(pub u32)` — bitwise check directly.
    let mut flags = ple::LayerFlags::empty();
    if (item.flags.0 & sdl::StackingContextFlags::IS_BLEND_CONTAINER.0) != 0 {
        flags |= ple::LayerFlags::BLEND_CONTAINER;
    }
    if (item.flags.0 & sdl::StackingContextFlags::IS_BACKDROP_ROOT.0) != 0 {
        flags |= ple::LayerFlags::BACKDROP_ROOT;
    }
    if (item.flags.0 & sdl::StackingContextFlags::HAS_SCROLL_LINKED_EFFECT.0) != 0 {
        flags |= ple::LayerFlags::HAS_SCROLL_LINKED_EFFECT;
    }
    ple::LayerSpec {
        origin: item.origin,
        opacity: 1.0,
        mix_blend_mode: item.mix_blend_mode,
        filters: item.filters.iter().map(convert_filter_op).collect(),
        raster_space: match item.raster_space {
            sdl::RasterSpace::Screen => ple::RasterSpace::Screen,
            sdl::RasterSpace::Local => ple::RasterSpace::Local,
        },
        flags,
        mask: None,
    }
}

fn convert_filter_op(f: &sdl::FilterOp) -> ple::FilterOp {
    match f {
        sdl::FilterOp::Identity => ple::FilterOp::Opacity(1.0),
        sdl::FilterOp::Blur(r) => ple::FilterOp::Blur(*r),
        sdl::FilterOp::Brightness(v) => ple::FilterOp::Brightness(*v),
        sdl::FilterOp::Contrast(v) => ple::FilterOp::Contrast(*v),
        sdl::FilterOp::Grayscale(v) => ple::FilterOp::Grayscale(*v),
        sdl::FilterOp::HueRotate(v) => ple::FilterOp::HueRotate(*v),
        sdl::FilterOp::Invert(v) => ple::FilterOp::Invert(*v),
        sdl::FilterOp::Opacity(v) => ple::FilterOp::Opacity(*v),
        sdl::FilterOp::Saturate(v) => ple::FilterOp::Saturate(*v),
        sdl::FilterOp::Sepia(v) => ple::FilterOp::Sepia(*v),
        sdl::FilterOp::DropShadow {
            offset,
            color,
            blur_radius,
        } => ple::FilterOp::DropShadow {
            offset: *offset,
            color: *color,
            blur_radius: *blur_radius,
        },
        sdl::FilterOp::ColorMatrix(m) => ple::FilterOp::ColorMatrix(*m),
    }
}

/// PushReferenceFrame → PushTransform. Needs the transforms palette
/// to resolve the `ReferenceFrameId` index to the concrete
/// `LayoutTransform`. Identity (index 0) is preserved.
fn lower_push_reference_frame(
    item: &sdl::ReferenceFramePushItem,
    transforms: &[LayoutTransform],
) -> ple::TransformSpec {
    let transform = transforms
        .get(item.transform_id.0 as usize)
        .copied()
        .unwrap_or_else(LayoutTransform::identity);
    ple::TransformSpec {
        origin: item.origin,
        transform,
        kind: convert_reference_frame_kind(&item.kind),
    }
}

fn convert_reference_frame_kind(
    kind: &paint_types::ReferenceFrameKind,
) -> ple::TransformKind {
    use paint_types::ReferenceFrameKind as R;
    match kind {
        R::Transform { .. } => ple::TransformKind::Standard,
        R::Perspective { .. } => ple::TransformKind::Perspective,
    }
}

fn lower_hit_test(item: &sdl::HitTestItem) -> ple::HitTestItem {
    ple::HitTestItem {
        placement: placement(&item.placement),
        tag: item.tag,
    }
}

fn convert_alpha_type(alpha: sdl::AlphaType) -> ple::AlphaType {
    match alpha {
        sdl::AlphaType::Alpha => ple::AlphaType::Alpha,
        sdl::AlphaType::PremultipliedAlpha => ple::AlphaType::PremultipliedAlpha,
    }
}

fn convert_glyph_options(opts: Option<sdl::GlyphOptions>) -> ple::TextOptions {
    let Some(opts) = opts else {
        return ple::TextOptions::default();
    };
    ple::TextOptions {
        render_mode: match opts.render_mode {
            sdl::FontRenderMode::Mono => ple::FontRenderMode::Mono,
            sdl::FontRenderMode::Alpha => ple::FontRenderMode::Alpha,
            sdl::FontRenderMode::Subpixel => ple::FontRenderMode::Subpixel,
        },
        // ServalDisplayList's GlyphOptions.flags is currently an opaque
        // u32; paint-list-api models hinting hints as a bool for v1.
        // Non-zero flags are treated as "hint metrics enabled" — refine
        // when the hint surface stabilizes on either side.
        hint_metrics: opts.flags != 0,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use paint_list_api as ple;
    use paint_types::ColorF;
    use paint_types::units::{DeviceIntSize, LayoutPoint, LayoutRect};

    use super::*;
    use crate::serval_display_list::ServalDisplayList;

    fn ctx<'a>(list: &'a ServalDisplayList) -> LowerContext<'a> {
        LowerContext {
            clip_defs: &list.clip_defs,
            spatial_nodes: &list.spatial_nodes,
            transforms: &list.transforms,
        }
    }

    fn box2d(x: f32, y: f32, w: f32, h: f32) -> LayoutRect {
        LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
    }

    fn fresh_list() -> ServalDisplayList {
        ServalDisplayList::new(DeviceIntSize::new(800, 600), PipelineId::default())
    }

    #[test]
    fn rect_lowers_to_draw_rect_with_bounds_preserved() {
        let mut list = fresh_list();
        let common = sdl::CommonItemPlacement {
            clip_rect: box2d(10.0, 20.0, 100.0, 50.0),
            clip_chain_id: sdl::ClipChainId::INVALID,
            spatial_id: list.root_spatial_id(),
            flags: sdl::PrimitiveFlags::HIT_TESTABLE,
        };
        list.push_rect(&common, box2d(10.0, 20.0, 100.0, 50.0), ColorF::default());
        let item = &list.items[0];
        match lower_item(item, &ctx(&list)) {
            ple::PaintCmd::DrawRect(r) => {
                assert_eq!(r.placement.bounds, box2d(10.0, 20.0, 100.0, 50.0));
                assert!(r.placement.flags.contains(ple::PrimitiveFlags::HIT_TESTABLE));
            },
            other => panic!("expected DrawRect, got {other:?}"),
        }
    }

    #[test]
    fn iframe_lowers_to_draw_external_texture_with_stable_key() {
        let mut list = fresh_list();
        let common = sdl::CommonItemPlacement {
            clip_rect: box2d(0.0, 0.0, 200.0, 200.0),
            clip_chain_id: sdl::ClipChainId::INVALID,
            spatial_id: list.root_spatial_id(),
            flags: sdl::PrimitiveFlags::empty(),
        };
        let info = sdl::SpaceAndClipInfo {
            spatial_id: common.spatial_id,
            clip_chain_id: common.clip_chain_id,
        };
        list.push_iframe(
            box2d(0.0, 0.0, 200.0, 200.0),
            box2d(0.0, 0.0, 200.0, 200.0),
            &info,
            PipelineId::default(),
            false,
        );
        let item = &list.items[0];
        match lower_item(item, &ctx(&list)) {
            ple::PaintCmd::DrawExternalTexture(et) => {
                assert_eq!(et.opacity, 1.0);
                assert_eq!(et.content_generation, None);
                // Stable across two lowerings of the same pipeline.
                let again = match lower_item(item, &ctx(&list)) {
                    ple::PaintCmd::DrawExternalTexture(x) => x,
                    _ => unreachable!(),
                };
                assert_eq!(et.texture_key, again.texture_key);
            },
            other => panic!("expected DrawExternalTexture, got {other:?}"),
        }
    }

    #[test]
    fn pop_reference_frame_lowers_to_pop_transform() {
        let list = fresh_list();
        let item = sdl::ServalDisplayItem::PopReferenceFrame;
        assert!(matches!(
            lower_item(&item, &ctx(&list)),
            ple::PaintCmd::PopTransform
        ));
    }

    #[test]
    fn pop_stacking_context_lowers_to_pop_layer() {
        let list = fresh_list();
        let item = sdl::ServalDisplayItem::PopStackingContext;
        assert!(matches!(
            lower_item(&item, &ctx(&list)),
            ple::PaintCmd::PopLayer
        ));
    }

    #[test]
    fn pop_all_shadows_lowers_to_pop_all_shadows() {
        let list = fresh_list();
        let item = sdl::ServalDisplayItem::PopAllShadows;
        assert!(matches!(
            lower_item(&item, &ctx(&list)),
            ple::PaintCmd::PopAllShadows
        ));
    }

    #[test]
    fn flags_round_trip_each_bit() {
        let combos = [
            sdl::PrimitiveFlags::HIT_TESTABLE,
            sdl::PrimitiveFlags::IS_BACKFACE,
            sdl::PrimitiveFlags::ANTIALIASED,
            sdl::PrimitiveFlags::HIT_TESTABLE | sdl::PrimitiveFlags::ANTIALIASED,
        ];
        for f in combos {
            let out = convert_flags(f);
            assert_eq!(
                out.contains(ple::PrimitiveFlags::HIT_TESTABLE),
                f.contains(sdl::PrimitiveFlags::HIT_TESTABLE)
            );
            assert_eq!(
                out.contains(ple::PrimitiveFlags::IS_BACKFACE),
                f.contains(sdl::PrimitiveFlags::IS_BACKFACE)
            );
            assert_eq!(
                out.contains(ple::PrimitiveFlags::ANTIALIASED),
                f.contains(sdl::PrimitiveFlags::ANTIALIASED)
            );
        }
    }
}
