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

use crate::{Fragment, FragmentPlane, StylePlane, layout::border_width_px, text::TextPainter};

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

/// Emit the backgrounds, physical borders, and independently shaped text nodes
/// supported by the first Cambium lane. Inline formatting, clipping, and
/// stacking-context composition remain later E3 work. `generation` is supplied
/// by the retained document/session owner.
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
    let mut list = LiveryPaintList::new(viewport, generation);
    let mut text = TextPainter::new();
    emit_node(
        dom,
        styles,
        fragments,
        dom.document(),
        None,
        &mut text,
        &mut list,
    );
    list.fonts = text.into_fonts();
    list
}

fn emit_node<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    inherited: Option<&ComputedValues>,
    text: &mut TextPainter,
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
            if let Some(fragment) = fragments
                .get(id)
                .filter(|fragment| paintable_fragment(fragment))
            {
                emit_background(list, style, fragment);
                emit_border(list, style, fragment);
            }
            Some(style)
        },
        NodeKind::Text => {
            if let (Some(style), Some(fragment), Some(value)) =
                (inherited, fragments.get(id), dom.text(id))
                && paintable_fragment(fragment)
            {
                text.emit(value, style, fragment, &mut list.commands);
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
