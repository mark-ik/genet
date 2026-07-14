//! Paint-list emission for Livery's bounded structural lane.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        BorderStyle as CssBorderStyle, Color, Display, FontSize, Length, LengthPercentage,
        LengthUnit,
    },
};
use paint_list_api::{
    BorderDetails, BorderItem, BorderRadius, BorderSide, BorderStyle, ColorF, CommonPlacement,
    DeviceIntSize, EngineId, FontResource, LayoutPoint, LayoutRect, LayoutSideOffsets,
    NormalBorder, PaintCmd, PaintList, RectItem,
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
                if let Some(inline_fragments) = text.frame.inline_fragments(id) {
                    for fragment in inline_fragments
                        .iter()
                        .filter(|fragment| paintable_fragment(fragment))
                    {
                        emit_background(list, style, fragment);
                        emit_border(list, style, fragment);
                    }
                } else if style.display == Display::InlineBlock
                    && let Some(fragment) = fragments
                        .get(id)
                        .filter(|fragment| paintable_fragment(fragment))
                {
                    emit_background(list, style, fragment);
                    emit_border(list, style, fragment);
                }
            } else if let Some(fragment) = fragments
                .get(id)
                .filter(|fragment| paintable_fragment(fragment))
            {
                emit_background(list, style, fragment);
                emit_border(list, style, fragment);
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

    for child in dom.dom_children(id) {
        emit_node(dom, styles, fragments, child, inherited, text, list);
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
    let em = used_font_size(style);
    let widths = LayoutSideOffsets::new(
        border_width_px(style.border_top_style, style.border_top_width, em),
        border_width_px(style.border_right_style, style.border_right_width, em),
        border_width_px(style.border_bottom_style, style.border_bottom_width, em),
        border_width_px(style.border_left_style, style.border_left_width, em),
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
