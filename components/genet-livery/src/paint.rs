//! Paint-list emission for Livery's bounded structural lane.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        BackgroundImage, BackgroundRepeat, BorderStyle as CssBorderStyle,
        BoxShadow as CssBoxShadow, Color, ComputedColor, Display, FontSize, Length,
        LengthPercentage, LengthUnit, Matrix2D, Overflow as CssOverflow, Position, Radius,
        Visibility, ZIndex,
    },
};
use paint_list_api::{
    AlphaType, BorderDetails, BorderItem, BorderRadius, BorderSide, BorderStyle, BoxShadowClipMode,
    ClipKind, ClipSpec, ColorF, CommonPlacement, DeviceIntSize, EngineId, ExtendMode, FontResource,
    GradientStop, IdNamespace, ImageItem, ImageKey, ImageRendering, ImageResource, LayerSpec,
    LayoutPoint, LayoutRect, LayoutSideOffsets, LayoutSize, LayoutTransform, LayoutVector2D,
    LinearGradientItem, LinearGradientPayload, NormalBorder, PaintCmd, PaintList, RectItem,
    ShadowItem, TransformKind, TransformSpec,
};
use serde::{Deserialize, Serialize};

use crate::{
    LiveryLayout, StylePlane,
    layout::{Fragment, border_width_px},
    text::{TextFrame, TextSystem},
};

/// Genet paint output produced by the Livery CSS/layout path.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiveryPaintList {
    viewport: DeviceIntSize,
    generation: u64,
    commands: Vec<PaintCmd>,
    fonts: Vec<FontResource>,
    images: Vec<ImageResource>,
    #[serde(skip)]
    image_keys: HashMap<String, ImageKey>,
    #[serde(skip)]
    image_sources: HashMap<String, Vec<u8>>,
}

impl LiveryPaintList {
    pub fn new(viewport: DeviceIntSize, generation: u64) -> Self {
        Self::with_image_sources(viewport, generation, &HashMap::new())
    }

    fn with_image_sources(
        viewport: DeviceIntSize,
        generation: u64,
        image_sources: &HashMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            viewport,
            generation,
            commands: Vec::new(),
            fonts: Vec::new(),
            images: Vec::new(),
            image_keys: HashMap::new(),
            image_sources: image_sources.clone(),
        }
    }

    fn image_key_for(&mut self, url: &str) -> Option<ImageKey> {
        if let Some(key) = self.image_keys.get(url) {
            return Some(*key);
        }
        let bytes = if let Ok(data_url) = data_url::DataUrl::process(url) {
            data_url.decode_to_vec().ok()?.0
        } else {
            self.image_sources.get(url)?.clone()
        };
        let rgba = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = rgba.dimensions();
        let key = ImageKey::new(IdNamespace(0), self.images.len() as u32 + 1);
        self.images.push(ImageResource {
            key,
            width,
            height,
            data: rgba.into_raw(),
        });
        self.image_keys.insert(url.to_owned(), key);
        Some(key)
    }

    fn image_size(&self, key: ImageKey) -> Option<(f32, f32)> {
        self.images
            .iter()
            .find(|image| image.key == key)
            .map(|image| (image.width as f32, image.height as f32))
    }

    pub(crate) fn translated(mut self, x: f32, y: f32) -> Self {
        if x == 0.0 && y == 0.0 {
            return self;
        }
        let transform = TransformSpec {
            origin: LayoutPoint::new(0.0, 0.0),
            transform: LayoutTransform::translation(x, y, 0.0),
            kind: TransformKind::Standard,
        };
        self.commands.insert(0, PaintCmd::PushTransform(transform));
        self.commands.push(PaintCmd::PopTransform);
        self
    }
}

impl PaintList for LiveryPaintList {
    fn engine_id(&self) -> EngineId {
        EngineId::GENET
    }

    fn viewport(&self) -> DeviceIntSize {
        self.viewport
    }

    fn generation_id(&self) -> u64 {
        self.generation
    }

    fn commands(&self) -> &[PaintCmd] {
        &self.commands
    }

    fn fonts(&self) -> &[FontResource] {
        &self.fonts
    }

    fn images(&self) -> &[ImageResource] {
        &self.images
    }
}

/// One-shot convenience path. Retained sessions should use
/// [`emit_paint_list_with_text_system`] so font discovery, shaping scratch
/// space, and font resources survive between frames.
pub fn emit_paint_list<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    viewport: DeviceIntSize,
    generation: u64,
) -> LiveryPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_paint_list_with_text_system(
        dom,
        styles,
        fragments,
        viewport,
        generation,
        &mut TextSystem::new(),
    )
}

/// Emit structural boxes and shared inline formatting through a retained text
/// system. `generation` is supplied by the document/session owner.
pub fn emit_paint_list_with_text_system<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    viewport: DeviceIntSize,
    generation: u64,
    text: &mut TextSystem,
) -> LiveryPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_paint_list_with_text_system_scrolled(
        dom,
        styles,
        fragments,
        viewport,
        generation,
        text,
        &HashMap::new(),
    )
}

/// Emit a retained frame with per-element scroll offsets applied to descendant
/// paint. The public convenience path keeps this map empty; retained sessions
/// supply their wheel-owned offsets here.
pub(crate) fn emit_paint_list_with_text_system_scrolled<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    viewport: DeviceIntSize,
    generation: u64,
    text: &mut TextSystem,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
) -> LiveryPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_paint_list_with_text_system_scrolled_with_images(
        dom,
        styles,
        fragments,
        viewport,
        generation,
        text,
        scroll_offsets,
        &HashMap::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_paint_list_with_text_system_scrolled_with_images<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    viewport: DeviceIntSize,
    generation: u64,
    text: &mut TextSystem,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    image_sources: &HashMap<String, Vec<u8>>,
) -> LiveryPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut list = LiveryPaintList::with_image_sources(viewport, generation, image_sources);
    let mut text_frame = fragments
        .text_frame()
        .cloned()
        .unwrap_or_else(|| text.begin_frame());
    let mut text_state = PaintText {
        system: text,
        frame: &mut text_frame,
    };
    let canvas_background_source = emit_canvas_background(dom, styles, fragments, &mut list);
    emit_node(
        dom,
        styles,
        fragments,
        dom.document(),
        None,
        &mut text_state,
        &mut list,
        scroll_offsets,
        canvas_background_source,
    );
    list.fonts = text.fonts_for(&text_frame);
    list
}

struct PaintText<'a, Id> {
    system: &'a mut TextSystem,
    frame: &'a mut TextFrame<Id>,
}

#[allow(clippy::too_many_arguments)]
fn emit_node<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    id: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    canvas_background_source: Option<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // K4e4: `transform` and `opacity` anchor to `get`, the node's outermost
    // box. For a table element that is the wrapper, which carries both under
    // CSS Tables 3 section 3.6.1, so the layer and the coordinate space wrap
    // the captions along with the grid.
    let transform = styles
        .get(id)
        .filter(|style| style.display != Display::None && style.visibility == Visibility::Visible)
        .and_then(|style| {
            fragments
                .get(id)
                .and_then(|fragment| transform_spec(style, fragment))
        });
    if let Some(transform) = &transform {
        list.commands
            .push(PaintCmd::PushTransform(transform.clone()));
    }
    let opacity = styles
        .get(id)
        .filter(|style| style.display != Display::None && style.visibility == Visibility::Visible)
        .map(|style| style.opacity.value())
        .filter(|opacity| *opacity < 1.0);
    if let Some(opacity) = opacity {
        list.commands.push(PaintCmd::PushLayer(LayerSpec {
            opacity,
            ..LayerSpec::default()
        }));
    }
    let Some((inherited, clips_descendants)) = begin_node(
        dom,
        styles,
        fragments,
        id,
        PaintScope {
            inherited,
            stacking_roots: None,
            inline_owner: None,
            canvas_background_source,
        },
        text,
        list,
    ) else {
        return;
    };
    let scroll_transform = scroll_offsets.get(&id).copied().and_then(scroll_spec);
    if let Some(transform) = &scroll_transform {
        list.commands
            .push(PaintCmd::PushTransform(transform.clone()));
    }
    emit_children_in_stacking_order(
        dom,
        styles,
        fragments,
        id,
        inherited,
        text,
        list,
        scroll_offsets,
        canvas_background_source,
    );
    if scroll_transform.is_some() {
        list.commands.push(PaintCmd::PopTransform);
    }
    if clips_descendants {
        list.commands.push(PaintCmd::PopClip);
    }
    if opacity.is_some() {
        list.commands.push(PaintCmd::PopLayer);
    }
    if transform.is_some() {
        list.commands.push(PaintCmd::PopTransform);
    }
}

fn begin_node<'a, D>(
    dom: &D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    id: D::NodeId,
    scope: PaintScope<'a, D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
) -> Option<(Option<&'a ComputedValues>, bool)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut clips_descendants = false;
    let inherited = match dom.kind(id) {
        NodeKind::Element => {
            let style = styles.get(id)?;
            if style.display == Display::None || style.visibility != Visibility::Visible {
                return None;
            }
            text.system
                .prepare_inline_children(text.frame, dom, styles, fragments, id, style);
            let background_propagated = scope.canvas_background_source == Some(id);
            if matches!(style.display, Display::Inline | Display::InlineBlock) {
                if !background_propagated {
                    emit_inline_element_decoration(text.frame, fragments, id, style, list);
                }
                emit_inline_replaced_image(dom, text.frame, fragments, id, list);
            } else if let Some(fragment) = fragments
                // K4e4: decorations address the principal box. For a table
                // element that is the grid, which owns background and borders
                // under CSS 2.1 section 17.4 - painting them on the wrapper
                // would wrongly cover the captions.
                .principal_fragment(id)
                .filter(|fragment| paintable_fragment(fragment))
            {
                emit_shadow(list, style, fragment);
                if !background_propagated {
                    emit_background(list, style, fragment);
                }
                emit_replaced_image(dom, list, id, style, fragment);
                emit_border(list, style, fragment);
            }
            // The overflow clip stays on the outer box: CSS Tables 3 section
            // 3.6.1 puts `overflow` on the table wrapper box, so a clipping
            // table clips at the box that contains its captions.
            if !matches!(style.display, Display::Inline | Display::InlineBlock)
                && let Some(fragment) = fragments.get(id)
                && let Some(clip) = descendant_clip(style, fragment, list.viewport)
            {
                list.commands.push(PaintCmd::PushClip(clip));
                clips_descendants = true;
            }
            Some(style)
        },
        NodeKind::Text => {
            if let (Some(style), Some(value)) = (scope.inherited, dom.text(id))
                && !text.frame.drain(
                    id,
                    scope.inline_owner,
                    scope.stacking_roots,
                    &mut list.commands,
                )
                && let Some(fragment) = fragments.get(id)
                && paintable_fragment(fragment)
            {
                text.system
                    .emit_single(text.frame, value, style, fragment, &mut list.commands);
            }
            scope.inherited
        },
        _ => scope.inherited,
    };
    Some((inherited, clips_descendants))
}

struct StackingItem<Id> {
    id: Id,
    level: i32,
    // Flattening moves the subtree outside these ancestors' normal paint
    // walk, so their overflow clips must travel with it.
    ancestor_clips: Vec<ClipSpec>,
}

#[derive(Clone, Copy)]
struct PaintScope<'a, Id> {
    inherited: Option<&'a ComputedValues>,
    stacking_roots: Option<&'a HashSet<Id>>,
    inline_owner: Option<Id>,
    canvas_background_source: Option<Id>,
}

#[allow(clippy::too_many_arguments)]
fn emit_children_in_stacking_order<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    parent: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    canvas_background_source: Option<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut items = Vec::new();
    collect_stacking_items(
        dom,
        styles,
        fragments,
        parent,
        list.viewport,
        &mut Vec::new(),
        &mut items,
    );
    items.sort_by_key(|item| item.level);
    let roots = items.iter().map(|item| item.id).collect::<HashSet<_>>();

    for item in items.iter().filter(|item| item.level < 0) {
        emit_stacking_item(
            dom,
            styles,
            fragments,
            item,
            text,
            list,
            scroll_offsets,
            canvas_background_source,
        );
    }

    emit_normal_children(
        dom,
        styles,
        fragments,
        parent,
        PaintScope {
            inherited,
            stacking_roots: Some(&roots),
            inline_owner: styles
                .get(parent)
                .filter(|style| style.display == Display::Inline)
                .map(|_| parent),
            canvas_background_source,
        },
        text,
        list,
        scroll_offsets,
    );

    for item in items.iter().filter(|item| item.level >= 0) {
        emit_stacking_item(
            dom,
            styles,
            fragments,
            item,
            text,
            list,
            scroll_offsets,
            canvas_background_source,
        );
    }
}

fn collect_stacking_items<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    parent: D::NodeId,
    viewport: DeviceIntSize,
    ancestor_clips: &mut Vec<ClipSpec>,
    items: &mut Vec<StackingItem<D::NodeId>>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    for child in dom.dom_children(parent) {
        // A numeric positioned node starts a local context. Its descendants
        // are collected when that context is emitted, keeping it atomic here.
        if let Some(level) = stacking_level(styles, child) {
            items.push(StackingItem {
                id: child,
                level,
                ancestor_clips: ancestor_clips.clone(),
            });
            continue;
        }

        let added_clip = match dom.kind(child) {
            NodeKind::Element => {
                let Some(style) = styles.get(child) else {
                    continue;
                };
                if style.display == Display::None {
                    continue;
                }
                if matches!(style.display, Display::Inline | Display::InlineBlock) {
                    None
                } else {
                    fragments
                        .get(child)
                        .and_then(|fragment| descendant_clip(style, fragment, viewport))
                }
            },
            NodeKind::Text => continue,
            _ => None,
        };
        let pushed_clip = added_clip.is_some();
        if let Some(clip) = added_clip {
            ancestor_clips.push(clip);
        }
        collect_stacking_items(
            dom,
            styles,
            fragments,
            child,
            viewport,
            ancestor_clips,
            items,
        );
        if pushed_clip {
            ancestor_clips.pop();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_stacking_item<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    item: &StackingItem<D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    canvas_background_source: Option<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    for clip in &item.ancestor_clips {
        list.commands.push(PaintCmd::PushClip(clip.clone()));
    }
    emit_node(
        dom,
        styles,
        fragments,
        item.id,
        None,
        text,
        list,
        scroll_offsets,
        canvas_background_source,
    );
    for _ in &item.ancestor_clips {
        list.commands.push(PaintCmd::PopClip);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_normal_node<'a, D>(
    dom: &D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    id: D::NodeId,
    scope: PaintScope<'a, D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if scope
        .stacking_roots
        .is_some_and(|roots| roots.contains(&id))
    {
        return;
    }
    let Some((inherited, clips_descendants)) =
        begin_node(dom, styles, fragments, id, scope, text, list)
    else {
        return;
    };
    let scroll_transform = scroll_offsets.get(&id).copied().and_then(scroll_spec);
    if let Some(transform) = &scroll_transform {
        list.commands
            .push(PaintCmd::PushTransform(transform.clone()));
    }
    emit_normal_children(
        dom,
        styles,
        fragments,
        id,
        PaintScope { inherited, ..scope },
        text,
        list,
        scroll_offsets,
    );
    if scroll_transform.is_some() {
        list.commands.push(PaintCmd::PopTransform);
    }
    if clips_descendants {
        list.commands.push(PaintCmd::PopClip);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_normal_children<'a, D>(
    dom: &D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    parent: D::NodeId,
    scope: PaintScope<'a, D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let child_ids = dom.dom_children(parent).collect::<Vec<_>>();

    let mut inline_group = Vec::new();
    for (index, child) in child_ids.iter().copied().enumerate() {
        if scope
            .stacking_roots
            .is_some_and(|roots| roots.contains(&child))
        {
            continue;
        }
        if is_inline_node(dom, styles, child) {
            inline_group.push(child);
            continue;
        }
        emit_inline_group(
            dom,
            styles,
            fragments,
            &inline_group,
            scope,
            text,
            list,
            scroll_offsets,
        );
        inline_group.clear();
        if positioned_inline_overlay_is_covered(dom, styles, fragments, &child_ids, index, child) {
            continue;
        }
        emit_normal_node(
            dom,
            styles,
            fragments,
            child,
            scope,
            text,
            list,
            scroll_offsets,
        );
    }
    emit_inline_group(
        dom,
        styles,
        fragments,
        &inline_group,
        scope,
        text,
        list,
        scroll_offsets,
    );
}

fn positioned_inline_overlay_is_covered<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    siblings: &[D::NodeId],
    index: usize,
    child: D::NodeId,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // The retained text backend blends glyph edges.  When a later positioned
    // inline occupies the same rectangle, keeping the earlier text underneath
    // leaks its color through those anti-aliased edge pixels.  CSS paints the
    // later overlay as the visible result, so drop the covered earlier run.
    let Some(style) = styles.get(child) else {
        return false;
    };
    if !matches!(style.position, Position::Absolute | Position::Fixed) {
        return false;
    }
    let Some(fragment) = fragments.get(child) else {
        return false;
    };
    siblings[index.saturating_add(1)..].iter().any(|later| {
        let Some(later_style) = styles.get(*later) else {
            return false;
        };
        if later_style.display == Display::None
            || !matches!(later_style.position, Position::Absolute | Position::Fixed)
            || !has_text_descendant(dom, *later)
        {
            return false;
        }
        let Some(later_fragment) = fragments.get(*later) else {
            return false;
        };
        same_fragment(fragment, later_fragment)
    })
}

fn has_text_descendant<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.kind(id) == NodeKind::Text && dom.text(id).is_some_and(|text| !text.trim().is_empty())
        || dom
            .dom_children(id)
            .any(|child| has_text_descendant(dom, child))
}

fn same_fragment(left: &Fragment, right: &Fragment) -> bool {
    (left.x - right.x).abs() <= 0.5
        && (left.y - right.y).abs() <= 0.5
        && (left.width - right.width).abs() <= 0.5
        && (left.height - right.height).abs() <= 0.5
}

fn stacking_level<Id>(styles: &StylePlane<Id>, id: Id) -> Option<i32>
where
    Id: Copy + Eq + Hash,
{
    let style = styles.get(id)?;
    if style.position != Position::Static
        && let ZIndex::Integer(level) = style.z_index
    {
        return Some(level);
    }
    (style.opacity.value() < 1.0 || establishes_transform_context(style)).then_some(0)
}

fn establishes_transform_context(style: &ComputedValues) -> bool {
    style.display != Display::Inline
        && (!style.transform.is_none()
            || style.rotate.radians().is_some()
            || style.scale.factor().is_some())
}

fn scroll_spec(offset: (f32, f32)) -> Option<TransformSpec> {
    if offset.0 == 0.0 && offset.1 == 0.0 {
        return None;
    }
    Some(TransformSpec {
        origin: LayoutPoint::new(0.0, 0.0),
        transform: LayoutTransform::translation(-offset.0, -offset.1, 0.0),
        kind: TransformKind::Standard,
    })
}

fn transform_spec(style: &ComputedValues, fragment: &Fragment) -> Option<TransformSpec> {
    if !establishes_transform_context(style) {
        return None;
    }
    let em = used_font_size(style);
    let mut matrix = Matrix2D::IDENTITY;
    if let Some(angle) = style.rotate.radians() {
        let (sin, cos) = angle.sin_cos();
        matrix = matrix.multiply(Matrix2D::new(cos, sin, -sin, cos, 0.0, 0.0));
    }
    if let Some(factor) = style.scale.factor() {
        matrix = matrix.multiply(Matrix2D::new(factor, 0.0, 0.0, factor, 0.0, 0.0));
    }
    if let Some(transform) = style
        .transform
        .to_matrix(em, (fragment.width, fragment.height))
    {
        matrix = matrix.multiply(transform);
    }
    let authored = LayoutTransform::new(
        matrix.a, matrix.b, 0.0, 0.0, matrix.c, matrix.d, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, matrix.e,
        matrix.f, 0.0, 1.0,
    );

    let origin = LayoutPoint::new(
        fragment.x + fragment.width / 2.0,
        fragment.y + fragment.height / 2.0,
    );
    let transform = LayoutTransform::translation(-origin.x, -origin.y, 0.0).then(&authored);
    Some(TransformSpec {
        origin,
        transform,
        kind: TransformKind::Standard,
    })
}

fn descendant_clip(
    style: &ComputedValues,
    fragment: &Fragment,
    viewport: DeviceIntSize,
) -> Option<ClipSpec> {
    let clips_x = clips_overflow(style.overflow_x);
    let clips_y = clips_overflow(style.overflow_y);
    if !clips_x && !clips_y {
        return None;
    }
    let em = used_font_size(style);
    let left = border_width_px(style.border_left_style, style.border_left_width, em);
    let right = border_width_px(style.border_right_style, style.border_right_width, em);
    let top = border_width_px(style.border_top_style, style.border_top_width, em);
    let bottom = border_width_px(style.border_bottom_style, style.border_bottom_width, em);
    let min_x = if clips_x { fragment.x + left } else { 0.0 };
    let max_x = if clips_x {
        (fragment.x + fragment.width - right).max(min_x)
    } else {
        viewport.width as f32
    };
    let min_y = if clips_y { fragment.y + top } else { 0.0 };
    let max_y = if clips_y {
        (fragment.y + fragment.height - bottom).max(min_y)
    } else {
        viewport.height as f32
    };
    Some(ClipSpec {
        kind: ClipKind::Rect(LayoutRect::new(
            LayoutPoint::new(min_x, min_y),
            LayoutPoint::new(max_x, max_y),
        )),
    })
}

fn clips_overflow(overflow: CssOverflow) -> bool {
    overflow != CssOverflow::Visible
}

#[allow(clippy::too_many_arguments)]
fn emit_inline_group<'a, D>(
    dom: &D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    roots: &[D::NodeId],
    scope: PaintScope<'a, D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if roots.is_empty() {
        return;
    }
    if let Some(first_line) = roots
        .iter()
        .filter_map(|root| text.frame.first_inline_line(*root))
        .min_by(|left, right| left.total_cmp(right))
    {
        for root in roots {
            emit_inline_descendant_decorations(
                dom,
                styles,
                fragments,
                *root,
                scope.stacking_roots,
                first_line,
                text.frame,
                list,
            );
        }
    }
    for root in roots {
        emit_normal_node(
            dom,
            styles,
            fragments,
            *root,
            scope,
            text,
            list,
            scroll_offsets,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_inline_descendant_decorations<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    id: D::NodeId,
    stacking_roots: Option<&HashSet<D::NodeId>>,
    first_line: f32,
    frame: &mut TextFrame<D::NodeId>,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if stacking_roots.is_some_and(|roots| roots.contains(&id))
        || frame
            .first_inline_line(id)
            .is_some_and(|line| (line - first_line).abs() > 0.5)
    {
        return;
    }
    let NodeKind::Element = dom.kind(id) else {
        return;
    };
    let Some(style) = styles.get(id) else {
        return;
    };
    if style.display == Display::None {
        return;
    }
    emit_inline_element_decoration(frame, fragments, id, style, list);
    if style.display == Display::Inline {
        for child in dom.dom_children(id) {
            if is_inline_node(dom, styles, child) {
                emit_inline_descendant_decorations(
                    dom,
                    styles,
                    fragments,
                    child,
                    stacking_roots,
                    first_line,
                    frame,
                    list,
                );
            }
        }
    }
}

fn emit_inline_element_decoration<Id>(
    frame: &mut TextFrame<Id>,
    fragments: &LiveryLayout<Id>,
    id: Id,
    style: &ComputedValues,
    list: &mut LiveryPaintList,
) where
    Id: Copy + Eq + Hash,
{
    let paintable = frame
        .inline_fragments(id)
        .map(|inline_fragments| {
            inline_fragments
                .iter()
                .copied()
                .filter(paintable_fragment)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !paintable.is_empty() {
        if !frame.mark_decoration_painted(id) {
            return;
        }
        let last = paintable.len().saturating_sub(1);
        for (index, fragment) in paintable.iter().enumerate() {
            emit_shadow(list, style, fragment);
            emit_background(list, style, fragment);
            emit_inline_border(list, style, fragment, index == 0, index == last);
        }
    } else if style.display == Display::InlineBlock
        && let Some(fragment) = fragments
            .get(id)
            .filter(|fragment| paintable_fragment(fragment))
        && frame.mark_decoration_painted(id)
    {
        emit_shadow(list, style, fragment);
        emit_background(list, style, fragment);
        emit_border(list, style, fragment);
    }
}

fn emit_inline_replaced_image<D>(
    dom: &D,
    frame: &TextFrame<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    id: D::NodeId,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let Some(url) = replaced_image_url(dom, id) else {
        return;
    };
    let Some(image_key) = list.image_key_for(&url) else {
        return;
    };
    let paintable = frame
        .inline_fragments(id)
        .map(|fragments| fragments.to_vec())
        .or_else(|| {
            fragments
                .get(id)
                .map(|fragment| vec![fragment.physical_rect()])
        })
        .unwrap_or_default();
    for fragment in paintable
        .iter()
        .filter(|fragment| paintable_fragment(fragment))
    {
        list.commands.push(PaintCmd::DrawImage(ImageItem {
            placement: CommonPlacement::new(bounds(fragment)),
            image_key,
            image_rendering: ImageRendering::Auto,
            alpha_type: AlphaType::Alpha,
            color: ColorF::WHITE,
        }));
    }
}

fn emit_replaced_image<D>(
    dom: &D,
    list: &mut LiveryPaintList,
    id: D::NodeId,
    style: &ComputedValues,
    fragment: &Fragment,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let Some(url) = replaced_image_url(dom, id) else {
        return;
    };
    let Some(image_key) = list.image_key_for(&url) else {
        return;
    };
    let radius = border_radius(style, fragment);
    if !radius.is_zero() {
        list.commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::RoundedRect {
                rect: bounds(fragment),
                radius,
                clip_out: false,
            },
        }));
    }
    list.commands.push(PaintCmd::DrawImage(ImageItem {
        placement: CommonPlacement::new(bounds(fragment)),
        image_key,
        image_rendering: ImageRendering::Auto,
        alpha_type: AlphaType::Alpha,
        color: ColorF::WHITE,
    }));
    if !radius.is_zero() {
        list.commands.push(PaintCmd::PopClip);
    }
}

fn replaced_image_url<D>(dom: &D, id: D::NodeId) -> Option<String>
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    if dom.kind(id) != NodeKind::Element
        || !dom
            .element_name(id)
            .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
    {
        return None;
    }
    dom.attributes(id).find_map(|attribute| {
        (attribute.name.ns.as_ref().is_empty()
            && attribute.name.local.as_ref().eq_ignore_ascii_case("src"))
        .then(|| attribute.value.to_owned())
    })
}

fn is_inline_node<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(id) {
        NodeKind::Text => true,
        NodeKind::Element => styles.get(id).is_some_and(|style| {
            matches!(style.display, Display::Inline | Display::InlineBlock)
                && !matches!(style.position, Position::Absolute | Position::Fixed)
                && !(style.display == Display::Inline
                    && dom.dom_children(id).any(|child| {
                        !is_inline_node(dom, styles, child)
                            && !styles
                                .get(child)
                                .is_some_and(|child_style| child_style.display == Display::None)
                    }))
        }),
        _ => false,
    }
}

fn paintable_fragment(fragment: &Fragment) -> bool {
    fragment.width.is_finite()
        && fragment.height.is_finite()
        && fragment.width > 0.0
        && fragment.height > 0.0
}

pub(crate) fn bounds(fragment: &Fragment) -> LayoutRect {
    LayoutRect::new(
        LayoutPoint::new(fragment.x, fragment.y),
        LayoutPoint::new(fragment.x + fragment.width, fragment.y + fragment.height),
    )
}

/// Paint the CSS canvas background before the document tree and return the
/// element whose used background becomes transparent.
///
/// CSS Backgrounds 3 section 2.11 selects the document element, except that
/// an HTML document with a transparent, image-free `html` background takes the
/// first `body` child's background instead. The painting area is the viewport;
/// image sizing and positioning still use the root element's box.
fn emit_canvas_background<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    list: &mut LiveryPaintList,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut elements = dom
        .dom_children(dom.document())
        .filter(|child| dom.kind(*child) == NodeKind::Element);
    let root = elements.next()?;
    if elements.next().is_some() {
        return None;
    }

    let root_style = styles.get(root)?;
    // CSS Backgrounds 3 section 2.11: if the element selected as the
    // canvas-background source has `display: none`, the canvas background is
    // transparent. There is no generated box whose absence may be replaced by
    // the viewport here.
    if root_style.display == Display::None {
        return None;
    }
    let root_is_html = dom
        .element_name(root)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("html"));
    let (source, source_style) = if has_background(root_style) {
        (root, root_style)
    } else if root_is_html {
        let body = dom.dom_children(root).find(|child| {
            dom.kind(*child) == NodeKind::Element
                && dom
                    .element_name(*child)
                    .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("body"))
        })?;
        let body_style = styles.get(body)?;
        if root_style.contain.is_active()
            || body_style.contain.is_active()
            || !generates_visible_box(body_style)
            || !has_background(body_style)
        {
            return None;
        }
        (body, body_style)
    } else {
        return None;
    };

    let canvas = Fragment {
        x: 0.0,
        y: 0.0,
        width: list.viewport.width.max(0) as f32,
        height: list.viewport.height.max(0) as f32,
    };
    let positioning = fragments
        .get(root)
        .map(|fragment| fragment.physical_rect())
        .unwrap_or(canvas);
    let color = resolve_color(
        &source_style.background_color,
        used_text_color(source_style),
    );
    if color.a > 0.0 {
        list.commands.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(bounds(&canvas)),
            color,
        }));
    }
    emit_background_image_in(list, source_style, bounds(&positioning), bounds(&canvas));
    Some(source)
}

fn generates_visible_box(style: &ComputedValues) -> bool {
    !matches!(style.display, Display::None | Display::Contents)
        && style.visibility == Visibility::Visible
}

fn has_background(style: &ComputedValues) -> bool {
    !matches!(style.background_image, BackgroundImage::None)
        || resolve_color(&style.background_color, used_text_color(style)).a > 0.0
}

fn emit_background(list: &mut LiveryPaintList, style: &ComputedValues, fragment: &Fragment) {
    let color = resolve_color(&style.background_color, used_text_color(style));
    let radius = border_radius(style, fragment);
    let has_image = !matches!(&style.background_image, BackgroundImage::None);
    if color.a <= 0.0 && !has_image {
        return;
    }
    if !radius.is_zero() {
        list.commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::RoundedRect {
                rect: bounds(fragment),
                radius,
                clip_out: false,
            },
        }));
    }
    if color.a > 0.0 {
        list.commands.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(bounds(fragment)),
            color,
        }));
    }
    emit_background_image(list, style, fragment);
    if !radius.is_zero() {
        list.commands.push(PaintCmd::PopClip);
    }
}

fn emit_background_image(list: &mut LiveryPaintList, style: &ComputedValues, fragment: &Fragment) {
    let rect = bounds(fragment);
    emit_background_image_in(list, style, rect, rect);
}

fn emit_background_image_in(
    list: &mut LiveryPaintList,
    style: &ComputedValues,
    positioning_rect: LayoutRect,
    painting_rect: LayoutRect,
) {
    match &style.background_image {
        BackgroundImage::None => {},
        BackgroundImage::LinearGradient { from, to } => {
            let start_point = LayoutPoint::new(
                (positioning_rect.min.x + positioning_rect.max.x) * 0.5,
                positioning_rect.min.y,
            );
            let end_point = LayoutPoint::new(
                (positioning_rect.min.x + positioning_rect.max.x) * 0.5,
                positioning_rect.max.y,
            );
            list.commands
                .push(PaintCmd::DrawLinearGradient(LinearGradientItem {
                    placement: CommonPlacement::new(painting_rect),
                    gradient: LinearGradientPayload {
                        start_point,
                        end_point,
                        extend_mode: ExtendMode::Clamp,
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: resolve_color(from, used_text_color(style)),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: resolve_color(to, used_text_color(style)),
                            },
                        ],
                    },
                    tile_size: positioning_rect.size(),
                    tile_spacing: LayoutSize::zero(),
                }));
        },
        BackgroundImage::Url(url) => {
            let Some(image_key) = list.image_key_for(url) else {
                return;
            };
            let Some((image_width, image_height)) = list.image_size(image_key) else {
                return;
            };
            let em = used_font_size(style);
            let offset_x = resolve_length_percentage(
                style.background_position.x,
                positioning_rect.size().width - image_width,
                em,
            );
            let offset_y = resolve_length_percentage(
                style.background_position.y,
                positioning_rect.size().height - image_height,
                em,
            );
            let repeat_x = matches!(
                style.background_repeat,
                BackgroundRepeat::Repeat | BackgroundRepeat::RepeatX
            );
            let repeat_y = matches!(
                style.background_repeat,
                BackgroundRepeat::Repeat | BackgroundRepeat::RepeatY
            );
            let first_x = tile_origin(
                positioning_rect.min.x + offset_x,
                painting_rect.min.x,
                image_width,
                repeat_x,
            );
            let first_y = tile_origin(
                positioning_rect.min.y + offset_y,
                painting_rect.min.y,
                image_height,
                repeat_y,
            );
            let x_count = tile_count(first_x, painting_rect.max.x, image_width, repeat_x);
            let y_count = tile_count(first_y, painting_rect.max.y, image_height, repeat_y);
            if repeat_x || repeat_y {
                list.commands.push(PaintCmd::PushClip(ClipSpec {
                    kind: ClipKind::Rect(painting_rect),
                }));
            }
            for x_index in 0..x_count {
                let x = first_x + x_index as f32 * image_width;
                for y_index in 0..y_count {
                    let y = first_y + y_index as f32 * image_height;
                    let placement = LayoutRect::new(
                        LayoutPoint::new(x, y),
                        LayoutPoint::new(x + image_width, y + image_height),
                    );
                    list.commands.push(PaintCmd::DrawImage(ImageItem {
                        placement: CommonPlacement::new(placement),
                        image_key,
                        image_rendering: ImageRendering::Auto,
                        alpha_type: AlphaType::Alpha,
                        color: ColorF::WHITE,
                    }));
                }
            }
            if repeat_x || repeat_y {
                list.commands.push(PaintCmd::PopClip);
            }
        },
    }
}

fn resolve_length_percentage(value: LengthPercentage, basis: f32, em: f32) -> f32 {
    match value {
        LengthPercentage::Zero => 0.0,
        LengthPercentage::Length(length) => length.unit.to_px(length.value, em, 16.0),
        LengthPercentage::Percentage(value) => basis * value,
        LengthPercentage::Calc(calc) => {
            calc.percentage * basis + calc.px + calc.em * em + calc.rem * 16.0
        },
        LengthPercentage::Math(math) => LengthPercentage::Math(math).to_px(em, 16.0, basis),
    }
}

fn tile_origin(origin: f32, painting_min: f32, tile: f32, repeated: bool) -> f32 {
    if repeated && tile > 0.0 {
        origin - ((origin - painting_min) / tile).ceil() * tile
    } else {
        origin
    }
}

fn tile_count(first: f32, max: f32, tile: f32, repeated: bool) -> usize {
    if !repeated || tile <= 0.0 {
        return 1;
    }
    (((max - first) / tile).ceil().max(0.0) as usize).saturating_add(1)
}

fn emit_shadow(list: &mut LiveryPaintList, style: &ComputedValues, fragment: &Fragment) {
    let CssBoxShadow::Value(shadow) = &style.box_shadow else {
        return;
    };
    let em = used_font_size(style);
    let length = |value: Length| value.unit.to_px(value.value, em, 16.0);
    list.commands.push(PaintCmd::DrawShadow(ShadowItem {
        placement: CommonPlacement::new(bounds(fragment)),
        box_bounds: bounds(fragment),
        offset: LayoutVector2D::new(length(shadow.offset_x), length(shadow.offset_y)),
        color: resolve_color(&shadow.color, used_text_color(style)),
        blur_radius: length(shadow.blur_radius).max(0.0),
        spread_radius: length(shadow.spread_radius),
        border_radius: border_radius(style, fragment),
        clip_mode: if shadow.inset {
            BoxShadowClipMode::Inset
        } else {
            BoxShadowClipMode::Outset
        },
    }));
}

fn emit_border(list: &mut LiveryPaintList, style: &ComputedValues, fragment: &Fragment) {
    emit_inline_border(list, style, fragment, true, true);
}

fn emit_inline_border(
    list: &mut LiveryPaintList,
    style: &ComputedValues,
    fragment: &Fragment,
    paint_left: bool,
    paint_right: bool,
) {
    let em = used_font_size(style);
    let widths = LayoutSideOffsets::new(
        border_width_px(style.border_top_style, style.border_top_width, em),
        if paint_right {
            border_width_px(style.border_right_style, style.border_right_width, em)
        } else {
            0.0
        },
        border_width_px(style.border_bottom_style, style.border_bottom_width, em),
        if paint_left {
            border_width_px(style.border_left_style, style.border_left_width, em)
        } else {
            0.0
        },
    );
    if widths.top == 0.0 && widths.right == 0.0 && widths.bottom == 0.0 && widths.left == 0.0 {
        return;
    }
    let current = used_text_color(style);
    list.commands.push(PaintCmd::DrawBorder(BorderItem {
        placement: CommonPlacement::new(bounds(fragment)),
        widths,
        details: BorderDetails::Normal(NormalBorder {
            left: border_side(style.border_left_style, &style.border_left_color, current),
            right: border_side(style.border_right_style, &style.border_right_color, current),
            top: border_side(style.border_top_style, &style.border_top_color, current),
            bottom: border_side(
                style.border_bottom_style,
                &style.border_bottom_color,
                current,
            ),
            radius: border_radius(style, fragment),
            do_aa: true,
        }),
    }));
}

fn border_radius(style: &ComputedValues, fragment: &Fragment) -> BorderRadius {
    let em = used_font_size(style);
    let corner = |x: Radius, y: Radius| {
        LayoutSize::new(
            super::layout::length_percentage_px(x.0, em, fragment.width),
            super::layout::length_percentage_px(y.0, em, fragment.height),
        )
    };
    BorderRadius {
        top_left: corner(style.border_top_left_radius, style.border_top_left_radius),
        top_right: corner(style.border_top_right_radius, style.border_top_right_radius),
        bottom_left: corner(
            style.border_bottom_left_radius,
            style.border_bottom_left_radius,
        ),
        bottom_right: corner(
            style.border_bottom_right_radius,
            style.border_bottom_right_radius,
        ),
    }
}

fn border_side(style: CssBorderStyle, color: &ComputedColor, current: ColorF) -> BorderSide {
    BorderSide {
        color: resolve_color(color, current),
        style: match style {
            CssBorderStyle::None => BorderStyle::None,
            CssBorderStyle::Hidden => BorderStyle::Hidden,
            CssBorderStyle::Dotted => BorderStyle::Dotted,
            CssBorderStyle::Dashed => BorderStyle::Dashed,
            CssBorderStyle::Solid => BorderStyle::Solid,
            CssBorderStyle::Double => BorderStyle::Double,
            CssBorderStyle::Groove => BorderStyle::Groove,
            CssBorderStyle::Ridge => BorderStyle::Ridge,
            CssBorderStyle::Inset => BorderStyle::Inset,
            CssBorderStyle::Outset => BorderStyle::Outset,
        },
    }
}

fn used_text_color(style: &ComputedValues) -> ColorF {
    resolve_color(&style.color, ColorF::BLACK)
}

pub(crate) fn resolve_color(color: &ComputedColor, current: ColorF) -> ColorF {
    let current = Color::srgb(current.r, current.g, current.b, current.a);
    let (red, green, blue, alpha) = color
        .resolve_used(livery::values::UsedColorContext::legacy(current))
        .to_srgb()
        .expect("a used computed color is numeric");
    ColorF::new(
        red.clamp(0.0, 1.0),
        green.clamp(0.0, 1.0),
        blue.clamp(0.0, 1.0),
        alpha.clamp(0.0, 1.0),
    )
}

pub(crate) fn used_font_size(style: &ComputedValues) -> f32 {
    match style.font_size {
        FontSize::Value(LengthPercentage::Length(Length {
            value,
            unit: LengthUnit::Px,
        })) => value,
        _ => 16.0,
    }
}
