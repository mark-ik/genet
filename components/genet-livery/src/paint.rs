//! Paint-list emission for Livery's bounded structural lane.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        BorderStyle as CssBorderStyle, Color, Display, FontSize, Length, LengthPercentage,
        LengthUnit, Overflow as CssOverflow, Position, ZIndex,
    },
};
use paint_list_api::{
    BorderDetails, BorderItem, BorderRadius, BorderSide, BorderStyle, ClipKind, ClipSpec, ColorF,
    CommonPlacement, DeviceIntSize, EngineId, FontResource, LayoutPoint, LayoutRect,
    LayoutSideOffsets, NormalBorder, PaintCmd, PaintList, RectItem,
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
}

impl LiveryPaintList {
    pub fn new(viewport: DeviceIntSize, generation: u64) -> Self {
        Self {
            viewport,
            generation,
            commands: Vec::new(),
            fonts: Vec::new(),
        }
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
    let mut list = LiveryPaintList::new(viewport, generation);
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
    );
    list.fonts = text.fonts_for(&text_frame);
    list
}

struct PaintText<'a, Id> {
    system: &'a mut TextSystem,
    frame: &'a mut TextFrame<Id>,
}

fn emit_node<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut clips_descendants = false;
    let inherited = match dom.kind(id) {
        NodeKind::Element => {
            let Some(style) = styles.get(id) else {
                return;
            };
            if style.display == Display::None {
                return;
            }
            text.system
                .prepare_inline_children(text.frame, dom, styles, fragments, id, style);
            if matches!(style.display, Display::Inline | Display::InlineBlock) {
                emit_inline_element_decoration(text.frame, fragments, id, style, list);
            } else if let Some(fragment) = fragments
                .get(id)
                .filter(|fragment| paintable_fragment(fragment))
            {
                emit_background(list, style, fragment);
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
            if let (Some(style), Some(value)) = (inherited, dom.text(id))
                && !text.frame.drain(id, &mut list.commands)
                && let Some(fragment) = fragments.get(id)
                && paintable_fragment(fragment)
            {
                text.system
                    .emit_single(text.frame, value, style, fragment, &mut list.commands);
            }
            inherited
        },
        _ => inherited,
    };

    emit_children_in_stacking_order(dom, styles, fragments, id, inherited, text, list);
    if clips_descendants {
        list.commands.push(PaintCmd::PopClip);
    }
}

fn emit_children_in_stacking_order<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    parent: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let child_ids = dom.dom_children(parent).collect::<Vec<_>>();
    let mut negative = child_ids
        .iter()
        .copied()
        .filter_map(|id| {
            stacking_level(styles, id)
                .filter(|level| *level < 0)
                .map(|level| (level, id))
        })
        .collect::<Vec<_>>();
    let mut nonnegative = child_ids
        .iter()
        .copied()
        .filter_map(|id| {
            stacking_level(styles, id)
                .filter(|level| *level >= 0)
                .map(|level| (level, id))
        })
        .collect::<Vec<_>>();
    negative.sort_by_key(|(level, _)| *level);
    nonnegative.sort_by_key(|(level, _)| *level);

    for (_, child) in negative {
        emit_node(dom, styles, fragments, child, inherited, text, list);
    }

    let mut inline_group = Vec::new();
    for child in child_ids {
        if stacking_level(styles, child).is_some() {
            continue;
        }
        if is_inline_node(dom, styles, child) {
            inline_group.push(child);
            continue;
        }
        emit_inline_group(dom, styles, fragments, &inline_group, inherited, text, list);
        inline_group.clear();
        emit_node(dom, styles, fragments, child, inherited, text, list);
    }
    emit_inline_group(dom, styles, fragments, &inline_group, inherited, text, list);

    for (_, child) in nonnegative {
        emit_node(dom, styles, fragments, child, inherited, text, list);
    }
}

fn stacking_level<Id>(styles: &StylePlane<Id>, id: Id) -> Option<i32>
where
    Id: Copy + Eq + Hash,
{
    let style = styles.get(id)?;
    if style.display == Display::Inline || style.position == Position::Static {
        return None;
    }
    match style.z_index {
        ZIndex::Integer(level) => Some(level),
        ZIndex::Auto => None,
    }
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

fn emit_inline_group<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    roots: &[D::NodeId],
    inherited: Option<&ComputedValues>,
    text: &mut PaintText<'_, D::NodeId>,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if roots.is_empty() {
        return;
    }
    for root in roots {
        emit_inline_descendant_decorations(dom, styles, fragments, *root, text.frame, list);
    }
    for root in roots {
        emit_node(dom, styles, fragments, *root, inherited, text, list);
    }
}

fn emit_inline_descendant_decorations<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    frame: &mut TextFrame<D::NodeId>,
    list: &mut LiveryPaintList,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
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
                emit_inline_descendant_decorations(dom, styles, fragments, child, frame, list);
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
            emit_background(list, style, fragment);
            emit_inline_border(list, style, fragment, index == 0, index == last);
        }
    } else if style.display == Display::InlineBlock
        && let Some(fragment) = fragments
            .get(id)
            .filter(|fragment| paintable_fragment(fragment))
        && frame.mark_decoration_painted(id)
    {
        emit_background(list, style, fragment);
        emit_border(list, style, fragment);
    }
}

fn is_inline_node<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(id) {
        NodeKind::Text => true,
        NodeKind::Element => styles
            .get(id)
            .is_some_and(|style| matches!(style.display, Display::Inline | Display::InlineBlock)),
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
    if color.a <= 0.0 {
        return;
    }
    list.commands.push(PaintCmd::DrawRect(RectItem {
        placement: CommonPlacement::new(bounds(fragment)),
        color,
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
            radius: BorderRadius::zero(),
            do_aa: true,
        }),
    }));
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
