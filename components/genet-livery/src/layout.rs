use std::{collections::HashMap, error::Error, fmt, hash::Hash};

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        BorderStyle, BorderWidth, Display as CssDisplay, FontSize, Inset, Length,
        LengthPercentage as CssLengthPercentage, LengthUnit, LineHeight, Margin,
        Overflow as CssOverflow, Position as CssPosition, Size as CssSize,
    },
};
use taffy::{
    TaffyTree,
    geometry::{Point, Rect, Size},
    prelude::{AvailableSpace, Dimension, LengthPercentage, LengthPercentageAuto, NodeId},
    style::{BoxSizing, Display, Overflow, Position, Style},
};

use crate::StylePlane;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fragment {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct FragmentPlane<Id> {
    fragments: HashMap<Id, Fragment>,
}

impl<Id> Default for FragmentPlane<Id> {
    fn default() -> Self {
        Self {
            fragments: HashMap::new(),
        }
    }
}

impl<Id: Eq + Hash> FragmentPlane<Id> {
    pub fn get(&self, id: Id) -> Option<&Fragment> {
        self.fragments.get(&id)
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutError(String);

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LayoutError {}

#[derive(Clone, Debug)]
struct TextMeasure {
    width: f32,
    height: f32,
}

struct BuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    tree: TaffyTree<TextMeasure>,
    sources: HashMap<NodeId, D::NodeId>,
}

/// Lay out a Livery style plane through a standalone Taffy tree.
///
/// This first Cambium lane maps block, inline-block, and inline boxes onto
/// Taffy's block algorithm. Text uses a deterministic metric estimate so the
/// path remains font-system-independent; shaped text and inline formatting are
/// explicit later E3 work.
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
) -> Result<FragmentPlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut state = BuildState {
        dom,
        styles,
        tree: TaffyTree::new(),
        sources: HashMap::new(),
    };
    let document = state.build_node(dom.document(), None, 16.0)?;
    let children = document.into_iter().collect::<Vec<_>>();
    let root = state
        .tree
        .new_with_children(
            Style {
                display: Display::Block,
                size: Size {
                    width: Dimension::length(viewport_width),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &children,
        )
        .map_err(taffy_error)?;

    state
        .tree
        .compute_layout_with_measure(
            root,
            Size {
                width: AvailableSpace::Definite(viewport_width),
                height: AvailableSpace::Definite(viewport_height),
            },
            |known, available, _, context, _| {
                let Some(context) = context else {
                    return Size::ZERO;
                };
                let available_width = match available.width {
                    AvailableSpace::Definite(width) => width,
                    AvailableSpace::MinContent => 0.0,
                    AvailableSpace::MaxContent => context.width,
                };
                Size {
                    width: known
                        .width
                        .unwrap_or(context.width.min(available_width.max(0.0))),
                    height: known.height.unwrap_or(context.height),
                }
            },
        )
        .map_err(taffy_error)?;

    let mut fragments = FragmentPlane::default();
    collect_fragments(
        &state.tree,
        &state.sources,
        root,
        Point { x: 0.0, y: 0.0 },
        &mut fragments,
    )?;
    Ok(fragments)
}

impl<D> BuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn build_node(
        &mut self,
        id: D::NodeId,
        inherited: Option<&ComputedValues>,
        parent_font_size: f32,
    ) -> Result<Option<NodeId>, LayoutError> {
        match self.dom.kind(id) {
            NodeKind::Document | NodeKind::DocumentFragment => {
                let children = self
                    .dom
                    .dom_children(id)
                    .filter_map(|child| {
                        self.build_node(child, inherited, parent_font_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if children.is_empty() {
                    Ok(None)
                } else if children.len() == 1 {
                    Ok(children.into_iter().next())
                } else {
                    self.tree
                        .new_with_children(
                            Style {
                                display: Display::Block,
                                ..Style::default()
                            },
                            &children,
                        )
                        .map(Some)
                        .map_err(taffy_error)
                }
            },
            NodeKind::Element => {
                let computed = self.styles.get(id).cloned().unwrap_or_default();
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let children = self
                    .dom
                    .dom_children(id)
                    .filter_map(|child| {
                        self.build_node(child, Some(&computed), font_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let node = self
                    .tree
                    .new_with_children(to_taffy_style(&computed, font_size), &children)
                    .map_err(taffy_error)?;
                self.sources.insert(node, id);
                Ok(Some(node))
            },
            NodeKind::Text => {
                let text = self.dom.text(id).unwrap_or("");
                if text.is_empty() {
                    return Ok(None);
                }
                let font_size = parent_font_size;
                let line_height = inherited
                    .map(|style| line_height_px(&style.line_height, font_size))
                    .unwrap_or(font_size * 1.2);
                let width = text
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0) as f32
                    * font_size
                    * 0.6;
                let height = text.lines().count().max(1) as f32 * line_height;
                let node = self
                    .tree
                    .new_leaf_with_context(
                        Style {
                            display: Display::Block,
                            ..Style::default()
                        },
                        TextMeasure { width, height },
                    )
                    .map_err(taffy_error)?;
                self.sources.insert(node, id);
                Ok(Some(node))
            },
            _ => Ok(None),
        }
    }
}

fn collect_fragments<Id>(
    tree: &TaffyTree<TextMeasure>,
    sources: &HashMap<NodeId, Id>,
    node: NodeId,
    parent_origin: Point<f32>,
    fragments: &mut FragmentPlane<Id>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node).map_err(taffy_error)?;
    let origin = Point {
        x: parent_origin.x + computed.location.x,
        y: parent_origin.y + computed.location.y,
    };
    if let Some(source) = sources.get(&node) {
        fragments.fragments.insert(
            *source,
            Fragment {
                x: origin.x,
                y: origin.y,
                width: computed.size.width,
                height: computed.size.height,
            },
        );
    }
    for child in tree.children(node).map_err(taffy_error)? {
        collect_fragments(tree, sources, child, origin, fragments)?;
    }
    Ok(())
}

fn to_taffy_style(computed: &ComputedValues, font_size: f32) -> Style {
    Style {
        display: match computed.display {
            CssDisplay::None => Display::None,
            _ => Display::Block,
        },
        box_sizing: BoxSizing::ContentBox,
        overflow: Point {
            x: overflow(computed.overflow_x),
            y: overflow(computed.overflow_y),
        },
        position: match computed.position {
            CssPosition::Absolute | CssPosition::Fixed => Position::Absolute,
            _ => Position::Relative,
        },
        inset: Rect {
            left: inset(computed.left, font_size),
            right: LengthPercentageAuto::auto(),
            top: inset(computed.top, font_size),
            bottom: LengthPercentageAuto::auto(),
        },
        size: Size {
            width: dimension(computed.width, font_size),
            height: dimension(computed.height, font_size),
        },
        margin: Rect {
            left: margin(computed.margin_left, font_size),
            right: margin(computed.margin_right, font_size),
            top: margin(computed.margin_top, font_size),
            bottom: margin(computed.margin_bottom, font_size),
        },
        padding: Rect {
            left: length_percentage(computed.padding_left.0, font_size),
            right: length_percentage(computed.padding_right.0, font_size),
            top: length_percentage(computed.padding_top.0, font_size),
            bottom: length_percentage(computed.padding_bottom.0, font_size),
        },
        border: Rect {
            left: border(
                computed.border_left_style,
                computed.border_left_width,
                font_size,
            ),
            right: border(
                computed.border_right_style,
                computed.border_right_width,
                font_size,
            ),
            top: border(
                computed.border_top_style,
                computed.border_top_width,
                font_size,
            ),
            bottom: border(
                computed.border_bottom_style,
                computed.border_bottom_width,
                font_size,
            ),
        },
        ..Style::default()
    }
}

fn font_size_px(size: &FontSize, parent: f32) -> f32 {
    match size {
        FontSize::Medium => 16.0,
        FontSize::Value(value) => absolute_length_percentage(*value, parent, 16.0, parent),
    }
    .max(0.0)
}

pub(crate) fn line_height_px(height: &LineHeight, font_size: f32) -> f32 {
    match height {
        LineHeight::Normal => font_size * 1.2,
        LineHeight::Number(value) => font_size * value,
        LineHeight::Value(value) => absolute_length_percentage(*value, font_size, 16.0, font_size),
    }
}

fn dimension(size: CssSize, em: f32) -> Dimension {
    match size {
        CssSize::Value(value) => match value {
            CssLengthPercentage::Percentage(value) => Dimension::percent(value),
            _ => Dimension::length(absolute_length_percentage(value, em, 16.0, 0.0)),
        },
        _ => Dimension::auto(),
    }
}

fn inset(value: Inset, em: f32) -> LengthPercentageAuto {
    match value {
        Inset::Auto => LengthPercentageAuto::auto(),
        Inset::Value(value) => length_percentage_auto(value, em),
    }
}

fn margin(value: Margin, em: f32) -> LengthPercentageAuto {
    match value {
        Margin::Auto => LengthPercentageAuto::auto(),
        Margin::Value(value) => length_percentage_auto(value, em),
    }
}

fn length_percentage_auto(value: CssLengthPercentage, em: f32) -> LengthPercentageAuto {
    match value {
        CssLengthPercentage::Percentage(value) => LengthPercentageAuto::percent(value),
        _ => LengthPercentageAuto::length(absolute_length_percentage(value, em, 16.0, 0.0)),
    }
}

fn length_percentage(value: CssLengthPercentage, em: f32) -> LengthPercentage {
    match value {
        CssLengthPercentage::Percentage(value) => LengthPercentage::percent(value),
        _ => LengthPercentage::length(absolute_length_percentage(value, em, 16.0, 0.0)),
    }
}

fn absolute_length_percentage(
    value: CssLengthPercentage,
    em: f32,
    rem: f32,
    percentage_basis: f32,
) -> f32 {
    match value {
        CssLengthPercentage::Zero => 0.0,
        CssLengthPercentage::Length(length) => absolute_length(length, em, rem),
        CssLengthPercentage::Percentage(value) => percentage_basis * value,
        CssLengthPercentage::Calc(calc) => {
            percentage_basis * calc.percentage + calc.px + calc.em * em + calc.rem * rem
        },
    }
}

fn absolute_length(length: Length, em: f32, rem: f32) -> f32 {
    length.value
        * match length.unit {
            LengthUnit::Px => 1.0,
            LengthUnit::Em => em,
            LengthUnit::Rem => rem,
        }
}

pub(crate) fn border_width_px(style: BorderStyle, width: BorderWidth, em: f32) -> f32 {
    if matches!(style, BorderStyle::None | BorderStyle::Hidden) {
        return 0.0;
    }
    match width {
        BorderWidth::Thin => 1.0,
        BorderWidth::Medium => 3.0,
        BorderWidth::Thick => 5.0,
        BorderWidth::Length(length) => absolute_length(length, em, 16.0),
    }
    .max(0.0)
}

fn border(style: BorderStyle, width: BorderWidth, em: f32) -> LengthPercentage {
    LengthPercentage::length(border_width_px(style, width, em))
}

fn overflow(value: CssOverflow) -> Overflow {
    match value {
        CssOverflow::Visible => Overflow::Visible,
        CssOverflow::Hidden => Overflow::Hidden,
        CssOverflow::Clip => Overflow::Clip,
        CssOverflow::Scroll | CssOverflow::Auto => Overflow::Scroll,
    }
}

fn taffy_error(error: impl fmt::Debug) -> LayoutError {
    LayoutError(format!("Taffy layout error: {error:?}"))
}
