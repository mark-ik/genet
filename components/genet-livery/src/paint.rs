//! Paint-list emission for Livery's bounded structural lane.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use euclid::Angle;
use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        BackgroundImage, BackgroundRepeat, BorderStyle as CssBorderStyle,
        BoxShadow as CssBoxShadow, Color, Display, FontSize, Length, LengthPercentage, LengthUnit,
        Overflow as CssOverflow, Position, Radius, TransformFunction, Visibility, ZIndex,
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
    Fragment, FragmentPlane, StylePlane,
    layout::border_width_px,
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
    fragments: &FragmentPlane<D::NodeId>,
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
    fragments: &FragmentPlane<D::NodeId>,
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
    fragments: &FragmentPlane<D::NodeId>,
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
    fragments: &FragmentPlane<D::NodeId>,
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
    let mut text_frame = text.begin_frame();
    let mut text_state = PaintText {
        system: text,
        frame: &mut text_frame,
    };
    emit_node(
        dom,
        styles,
        fragments,
        dom.document(),
        None,
        &mut text_state,
        &mut list,
        scroll_offsets,
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
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
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
    fragments: &FragmentPlane<D::NodeId>,
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
            if matches!(style.display, Display::Inline | Display::InlineBlock) {
                emit_inline_element_decoration(text.frame, fragments, id, style, list);
                emit_inline_replaced_image(dom, text.frame, fragments, id, list);
            } else if let Some(fragment) = fragments
                .get(id)
                .filter(|fragment| paintable_fragment(fragment))
            {
                emit_shadow(list, style, fragment);
                emit_background(list, style, fragment);
                emit_replaced_image(dom, list, id, style, fragment);
                emit_border(list, style, fragment);
            }
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
}

#[allow(clippy::too_many_arguments)]
fn emit_children_in_stacking_order<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    parent: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
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
        emit_stacking_item(dom, styles, fragments, item, text, list, scroll_offsets);
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
        },
        text,
        list,
        scroll_offsets,
    );

    for item in items.iter().filter(|item| item.level >= 0) {
        emit_stacking_item(dom, styles, fragments, item, text, list, scroll_offsets);
    }
}

fn collect_stacking_items<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
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

fn emit_stacking_item<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    item: &StackingItem<D::NodeId>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
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
    );
    for _ in &item.ancestor_clips {
        list.commands.push(PaintCmd::PopClip);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_normal_node<'a, D>(
    dom: &D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
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
    fragments: &FragmentPlane<D::NodeId>,
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
    for child in child_ids {
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
    style.display != Display::Inline && !style.transform.is_none()
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
    let functions = style.transform.functions()?;
    let em = used_font_size(style);
    let mut authored = LayoutTransform::identity();
    for function in functions {
        let next = match *function {
            TransformFunction::Translate(x, y) => {
                LayoutTransform::translation(transform_length(x, em), transform_length(y, em), 0.0)
            },
            TransformFunction::Scale(x, y) => LayoutTransform::scale(x, y, 1.0),
            TransformFunction::Rotate(radians) => {
                LayoutTransform::rotation(0.0, 0.0, 1.0, Angle::radians(radians))
            },
        };
        authored = authored.then(&next);
    }

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

fn transform_length(length: Length, em: f32) -> f32 {
    length.unit.to_px(length.value, em, 16.0)
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
    fragments: &FragmentPlane<D::NodeId>,
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

fn emit_inline_descendant_decorations<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
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
    fragments: &FragmentPlane<Id>,
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
    fragments: &FragmentPlane<D::NodeId>,
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
        .or_else(|| fragments.get(id).copied().map(|fragment| vec![fragment]))
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

fn emit_background(list: &mut LiveryPaintList, style: &ComputedValues, fragment: &Fragment) {
    let color = resolve_color(style.background_color, used_text_color(style));
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
    match &style.background_image {
        BackgroundImage::None => {},
        BackgroundImage::LinearGradient { from, to } => {
            let start_point = LayoutPoint::new((rect.min.x + rect.max.x) * 0.5, rect.min.y);
            let end_point = LayoutPoint::new((rect.min.x + rect.max.x) * 0.5, rect.max.y);
            list.commands
                .push(PaintCmd::DrawLinearGradient(LinearGradientItem {
                    placement: CommonPlacement::new(rect),
                    gradient: LinearGradientPayload {
                        start_point,
                        end_point,
                        extend_mode: ExtendMode::Clamp,
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: resolve_color(*from, used_text_color(style)),
                            },
                            GradientStop {
                                offset: 1.0,
                                color: resolve_color(*to, used_text_color(style)),
                            },
                        ],
                    },
                    tile_size: LayoutSize::new(fragment.width, fragment.height),
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
                rect.size().width - image_width,
                em,
            );
            let offset_y = resolve_length_percentage(
                style.background_position.y,
                rect.size().height - image_height,
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
            let first_x = tile_origin(rect.min.x, offset_x, image_width, repeat_x);
            let first_y = tile_origin(rect.min.y, offset_y, image_height, repeat_y);
            let x_count = tile_count(first_x, rect.max.x, image_width, repeat_x);
            let y_count = tile_count(first_y, rect.max.y, image_height, repeat_y);
            if repeat_x || repeat_y {
                list.commands.push(PaintCmd::PushClip(ClipSpec {
                    kind: ClipKind::Rect(rect),
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
    }
}

fn tile_origin(min: f32, offset: f32, tile: f32, repeated: bool) -> f32 {
    let origin = min + offset;
    if repeated && tile > 0.0 {
        origin - (offset / tile).ceil() * tile
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
        color: resolve_color(shadow.color, used_text_color(style)),
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
            left: border_side(style.border_left_style, style.border_left_color, current),
            right: border_side(style.border_right_style, style.border_right_color, current),
            top: border_side(style.border_top_style, style.border_top_color, current),
            bottom: border_side(
                style.border_bottom_style,
                style.border_bottom_color,
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

fn border_side(style: CssBorderStyle, color: Color, current: ColorF) -> BorderSide {
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
    resolve_color(style.color, ColorF::BLACK)
}

pub(crate) fn resolve_color(color: Color, current: ColorF) -> ColorF {
    match color {
        Color::Transparent => ColorF::TRANSPARENT,
        Color::CurrentColor => current,
        Color::CanvasText => ColorF::BLACK,
        Color::Rgba {
            red,
            green,
            blue,
            alpha,
        } => ColorF::new(
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0,
            f32::from(alpha) / 255.0,
        ),
    }
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
