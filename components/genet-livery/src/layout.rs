use std::{collections::HashMap, error::Error, fmt, hash::Hash};

use buckram::{
    AlgorithmAvailableSpace, AlgorithmKind, AlgorithmNodeId, AlgorithmSize, AlgorithmTree,
    BlockBoxSizing, BlockDimensions, BlockPosition as BuckramBlockPosition, BlockSizeValue,
    BlockStyle, BoxId, BoxOrigin, ClearSide, CssBox, DisplayInside, DisplayOutside,
    FloatLineConstraints, FloatSide, FlowAxes, FlowLength, FlowLengthAuto, FormattingContextKind,
    Fragment as TreeFragment, FragmentId, FragmentTree, InternalTableRole, IntrinsicSizeCache,
    IntrinsicSizeKind, IntrinsicSizeQuery, IntrinsicSizes, LayoutResult, LogicalAxis, PhysicalRect,
    PhysicalSides, PositioningScheme,
};
use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    media::{Device, ViewportSizes},
    stylesheet::ContainerSnapshot,
    values::{
        Alignment as CssAlignment, AspectRatio, BorderStyle, BorderWidth,
        BoxSizing as CssBoxSizing, Clear as CssClear, ContainerType, Display as CssDisplay,
        FlexDirection as CssFlexDirection, FlexWrap as CssFlexWrap, Float as CssFloat, FontSize,
        Gap as CssGap, GridAutoFlow as CssGridAutoFlow, GridPlacement as CssGridPlacement,
        GridTemplate as CssGridTemplate, GridTrack as CssGridTrack, Inset, Length,
        LengthPercentage as CssLengthPercentage, LineHeight, Margin, Overflow as CssOverflow,
        Position as CssPosition, RelativeLengthEnvironment, Size as CssSize,
        TableLayout as CssTableLayout, VerticalAlign, WhiteSpaceCollapse,
    },
};
use taffy::{
    geometry::{Line, Point, Rect, Size},
    prelude::{
        Dimension, LengthPercentage, LengthPercentageAuto, auto, fr, length, line, max_content,
        min_content, percent, span,
    },
    style::{
        AlignContent, AlignContentKeyword, AlignItems, AlignItemsKeyword, BoxSizing, Display,
        FlexDirection, FlexWrap, Float as TaffyFloat, GridAutoFlow, GridPlacement,
        GridTemplateComponent, JustifyContent, Overflow, Position, Style,
    },
};

type ImageSources = HashMap<String, Vec<u8>>;

use crate::{
    InteractionStates, StylePlane, StyleSet, TextSystem,
    box_tree::GeneratedBoxTree,
    style::resolve_styles_with_containers,
    text::{InlineLayout, InlineRequest, TextFrame},
};

/// Physical geometry used at the DOM compatibility edge and by inline atoms.
pub(crate) type Fragment = PhysicalRect;

#[derive(Clone, Debug)]
struct AtomicSubtree {
    root: BoxId,
    fragments: Vec<(BoxId, Fragment)>,
}

#[derive(Clone, Debug, Default)]
struct AtomicLayoutPlane {
    fragments: HashMap<BoxId, Fragment>,
    subtrees: Vec<AtomicSubtree>,
}

impl AtomicLayoutPlane {
    pub fn get(&self, box_id: BoxId) -> Option<&Fragment> {
        self.fragments.get(&box_id)
    }
}

impl<Id> crate::text::FragmentLookup<Id> for AtomicLayoutPlane
where
    Id: Copy + Eq + Hash,
{
    fn rect(&self, _id: Id) -> Option<&Fragment> {
        None
    }

    fn atomic_box_rect(&self, box_id: BoxId) -> Option<&Fragment> {
        self.get(box_id)
    }
}

/// Livery's retained wrapper around Buckram's standards-owned layout result.
#[derive(Clone, Debug)]
pub struct LiveryLayout<Id> {
    buckram: LayoutResult<Id>,
    text_frame: Option<TextFrame<Id>>,
    block_algorithms: BlockAlgorithmCounts,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockAlgorithmCounts {
    pub buckram: usize,
    pub taffy: usize,
}

impl<Id> LiveryLayout<Id>
where
    Id: Copy + Eq + Hash,
{
    fn new(
        buckram: LayoutResult<Id>,
        text_frame: Option<TextFrame<Id>>,
        block_algorithms: BlockAlgorithmCounts,
    ) -> Self {
        Self {
            buckram,
            text_frame,
            block_algorithms,
        }
    }

    pub fn buckram(&self) -> &LayoutResult<Id> {
        &self.buckram
    }

    pub fn boxes(&self) -> &buckram::CssBoxTree<Id> {
        self.buckram.boxes()
    }

    pub fn fragments(&self) -> &FragmentTree {
        self.buckram.fragments()
    }

    pub fn fragments_for_node(&self, node: Id) -> impl Iterator<Item = &TreeFragment> {
        self.buckram.fragments_for_node(node)
    }

    pub fn get(&self, node: Id) -> Option<&TreeFragment> {
        self.buckram.get(node)
    }

    pub fn len(&self) -> usize {
        self.buckram.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckram.is_empty()
    }

    pub fn block_algorithm_counts(&self) -> BlockAlgorithmCounts {
        self.block_algorithms
    }

    pub(crate) fn text_frame(&self) -> Option<&TextFrame<Id>> {
        self.text_frame.as_ref()
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
    min_width: f32,
    max_width: f32,
    height: f32,
}

struct BuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    boxes: &'a GeneratedBoxTree<D::NodeId>,
    tree: AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
    image_sources: &'a ImageSources,
}

struct InlineMeasure {
    owner: Option<BoxId>,
    roots: Vec<BoxId>,
    style: ComputedValues,
    width: f32,
    height: f32,
    layouts: Vec<InlineLayoutEntry>,
    placement_constraints: Option<FloatLineConstraints>,
}

struct InlineLayoutEntry {
    width: f32,
    constraints: Option<FloatLineConstraints>,
    layout: InlineLayout<BoxId>,
}

#[derive(Clone, Copy)]
struct InlineMeasureGeometry<'a> {
    width: f32,
    line_constraints: Option<&'a FloatLineConstraints>,
}

impl InlineMeasure {
    fn cached_size(
        &self,
        width: f32,
        constraints: Option<&FloatLineConstraints>,
    ) -> Option<(f32, f32)> {
        self.layouts
            .iter()
            .find(|entry| {
                (entry.width - width).abs() <= 0.01 && entry.constraints.as_ref() == constraints
            })
            .map(|entry| entry.layout.size())
    }

    fn remember(
        &mut self,
        width: f32,
        constraints: Option<&FloatLineConstraints>,
        layout: InlineLayout<BoxId>,
    ) -> (f32, f32) {
        let size = layout.size();
        self.layouts.push(InlineLayoutEntry {
            width,
            constraints: constraints.cloned(),
            layout,
        });
        size
    }

    fn layout_for_width(&self, width: f32) -> Option<&InlineLayout<BoxId>> {
        self.layouts
            .iter()
            .filter(|entry| entry.constraints.as_ref() == self.placement_constraints.as_ref())
            .min_by(|left, right| {
                (left.width - width)
                    .abs()
                    .total_cmp(&(right.width - width).abs())
            })
            .map(|entry| &entry.layout)
    }
}

fn measure_inline_context<D>(
    text: &mut TextSystem,
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    boxes: &GeneratedBoxTree<D::NodeId>,
    atomic: &AtomicLayoutPlane,
    context: &mut InlineMeasure,
    geometry: InlineMeasureGeometry<'_>,
) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let InlineMeasureGeometry {
        width,
        line_constraints: constraints,
    } = geometry;
    if let Some(constraints) = constraints {
        context.placement_constraints = Some(constraints.clone());
    }
    context.cached_size(width, constraints).unwrap_or_else(|| {
        let formatted = text.format_inline_group(
            dom,
            styles,
            boxes,
            atomic,
            InlineRequest {
                roots: &context.roots,
                parent_style: &context.style,
                width,
                line_constraints: constraints,
            },
        );
        formatted.map_or((context.width, context.height), |layout| {
            context.remember(width, constraints, layout)
        })
    })
}

struct InlineBuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    boxes: &'a GeneratedBoxTree<D::NodeId>,
    atomic: &'a AtomicLayoutPlane,
    tree: AlgorithmTree<Style, InlineMeasure, Vec<BoxId>>,
    image_sources: &'a ImageSources,
}

type ResolvedLayout<Id> = (StylePlane<Id>, LiveryLayout<Id>);

#[derive(Clone, Copy, Debug, Default)]
struct ContainerBases {
    width: Option<f32>,
    height: Option<f32>,
    inline: Option<f32>,
    block: Option<f32>,
}

/// Lay out a Livery style plane through Buckram's scratch algorithm tree.
///
/// This stateless entry point uses deterministic text estimates. Retained
/// Livery sessions call [`layout_with_text_system`] so Parley's shaped line
/// height participates in parent block flow.
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
) -> Result<LiveryLayout<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let image_sources = ImageSources::new();
    let viewport = ViewportSizes::uniform(viewport_width, viewport_height);
    let resolved =
        resolve_container_relative_styles_with_images(dom, styles, viewport, &image_sources)?;
    layout_impl(
        dom,
        &resolved,
        viewport_width,
        viewport_height,
        &image_sources,
    )
}

/// Produce the layout bases needed by resolved-value CSSOM reads without
/// letting the queried element's own margin expression participate in the
/// measurement. This matters for percentage-bearing margin math: its basis is
/// the containing block, which must be known before the expression can be
/// evaluated.
pub fn used_value_context<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    node: D::NodeId,
) -> Result<Option<crate::UsedValueContext>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut measuring = styles.clone();
    if let Some(style) = measuring.get_mut(node) {
        let zero = Margin::Value(CssLengthPercentage::ZERO);
        style.margin_top = zero;
        style.margin_right = zero;
        style.margin_bottom = zero;
        style.margin_left = zero;
    }
    let fragments = layout(dom, &measuring, viewport_width, viewport_height)?;
    let Some(fragment) = fragments.get(node) else {
        return Ok(None);
    };
    let containing_inline_size = dom.parent(node).and_then(|parent| {
        let style = measuring.get(parent)?;
        let fragment = fragments.get(parent)?;
        Some(content_box_size(style, fragment).0)
    });
    Ok(Some(crate::UsedValueContext {
        border_box: (fragment.width, fragment.height),
        containing_inline_size,
    }))
}

pub(crate) fn layout_with_text_system<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    viewport: ViewportSizes,
    text: &mut TextSystem,
    image_sources: &ImageSources,
) -> Result<ResolvedLayout<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let styles =
        resolve_container_relative_styles_with_images(dom, styles, viewport, image_sources)?;
    let boxes = GeneratedBoxTree::from_dom(dom, &styles);
    let atomic = layout_atomic_subtrees(
        dom,
        &styles,
        &boxes,
        viewport_width,
        viewport_height,
        image_sources,
    )?;
    let fragments = layout_inline_groups(
        dom,
        &styles,
        boxes,
        (viewport_width, viewport_height),
        text,
        &atomic,
        image_sources,
    )?;
    Ok((styles, fragments))
}

/// Resolve deferred container-relative units from the nearest eligible
/// ancestor content boxes. A fallback pass supplies small-viewport values so
/// Taffy can establish those boxes without consuming unresolved units.
pub fn resolve_container_relative_styles<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport: ViewportSizes,
) -> Result<StylePlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    resolve_container_relative_styles_with_images(dom, styles, viewport, &ImageSources::new())
}

/// Iterate size-query cascade and container-unit resolution until the style
/// plane stabilizes. The pass is bounded so cyclic queries cannot hang a
/// frame; the final bounded state is laid out normally.
pub fn resolve_container_query_styles<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    style_set: &StyleSet,
    device: &Device,
    interactions: &InteractionStates<D::NodeId>,
) -> Result<StylePlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    resolve_container_query_styles_with_images(
        dom,
        styles,
        style_set,
        device,
        interactions,
        &ImageSources::new(),
    )
}

pub(crate) fn resolve_container_query_styles_with_images<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    style_set: &StyleSet,
    device: &Device,
    interactions: &InteractionStates<D::NodeId>,
    image_sources: &ImageSources,
) -> Result<StylePlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if !style_set.has_container_queries() {
        return resolve_container_relative_styles_with_images(
            dom,
            styles,
            device.viewport_sizes,
            image_sources,
        );
    }
    let mut current = styles.clone();
    for _ in 0..8 {
        let resolved = resolve_container_relative_styles_with_images(
            dom,
            &current,
            device.viewport_sizes,
            image_sources,
        )?;
        let fragments = layout_impl(
            dom,
            &resolved,
            device.viewport_width,
            device.viewport_height,
            image_sources,
        )?;
        let containers = container_snapshots(dom, &resolved, &fragments);
        let next =
            resolve_styles_with_containers(dom, style_set, device, interactions, &containers);
        if next == current {
            return Ok(resolved);
        }
        current = next;
    }
    resolve_container_relative_styles_with_images(
        dom,
        &current,
        device.viewport_sizes,
        image_sources,
    )
}

fn container_snapshots<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
) -> HashMap<D::NodeId, Vec<ContainerSnapshot>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut snapshots = HashMap::new();
    collect_container_snapshots(dom, dom.document(), styles, fragments, &[], &mut snapshots);
    snapshots
}

fn collect_container_snapshots<D>(
    dom: &D,
    id: D::NodeId,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    ancestors: &[ContainerSnapshot],
    snapshots: &mut HashMap<D::NodeId, Vec<ContainerSnapshot>>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut descendants = ancestors.to_vec();
    if dom.kind(id) == NodeKind::Element {
        snapshots.insert(id, ancestors.to_vec());
        if let (Some(style), Some(fragment)) = (styles.get(id), fragments.get(id))
            && style.container_type != ContainerType::Normal
        {
            let (width, height) = content_box_size(style, fragment);
            let (inline_size, block_size) = if style.writing_mode.is_vertical() {
                (height, width)
            } else {
                (width, height)
            };
            descendants.insert(
                0,
                ContainerSnapshot {
                    names: style.container_name.names().to_vec(),
                    container_type: style.container_type,
                    writing_mode: style.writing_mode,
                    width,
                    height,
                    inline_size,
                    block_size,
                },
            );
        }
    }
    for child in dom.dom_children(id) {
        collect_container_snapshots(dom, child, styles, fragments, &descendants, snapshots);
    }
}

fn resolve_container_relative_styles_with_images<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport: ViewportSizes,
    image_sources: &ImageSources,
) -> Result<StylePlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut fallback = styles.clone();
    resolve_relative_subtree(
        dom,
        dom.document(),
        &mut fallback,
        RelativeLengthEnvironment::container_fallback(viewport),
    );
    if fallback == *styles {
        return Ok(styles.clone());
    }
    let fragments = layout_impl(
        dom,
        &fallback,
        viewport.dynamic.width,
        viewport.dynamic.height,
        image_sources,
    )?;

    let mut resolved = styles.clone();
    resolve_container_subtree(
        dom,
        dom.document(),
        &mut resolved,
        &fragments,
        viewport,
        ContainerBases::default(),
    );
    Ok(resolved)
}

fn resolve_relative_subtree<D>(
    dom: &D,
    id: D::NodeId,
    styles: &mut StylePlane<D::NodeId>,
    environment: RelativeLengthEnvironment,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let environment = environment.with_vertical_writing(
        styles
            .get(id)
            .is_some_and(|style| style.writing_mode.is_vertical()),
    );
    styles.resolve_relative_lengths(id, environment);
    for child in dom.dom_children(id) {
        resolve_relative_subtree(dom, child, styles, environment);
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_container_subtree<D>(
    dom: &D,
    id: D::NodeId,
    styles: &mut StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    viewport: ViewportSizes,
    bases: ContainerBases,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let vertical_writing = styles
        .get(id)
        .is_some_and(|style| style.writing_mode.is_vertical());
    styles.resolve_relative_lengths(
        id,
        RelativeLengthEnvironment::container_axes(
            viewport,
            bases.width,
            bases.height,
            bases.inline,
            bases.block,
            vertical_writing,
        ),
    );

    let mut next = bases;
    if let (Some(style), Some(fragment)) = (styles.get(id), fragments.get(id)) {
        let (width, height) = content_box_size(style, fragment);
        let vertical = style.writing_mode.is_vertical();
        let (inline_size, block_size) = if vertical {
            (height, width)
        } else {
            (width, height)
        };
        match style.container_type {
            ContainerType::Normal => {},
            ContainerType::InlineSize => {
                next.inline = Some(inline_size);
                if vertical {
                    next.height = Some(height);
                } else {
                    next.width = Some(width);
                }
            },
            ContainerType::Size => {
                next.width = Some(width);
                next.height = Some(height);
                next.inline = Some(inline_size);
                next.block = Some(block_size);
            },
        }
    }

    for child in dom.dom_children(id) {
        resolve_container_subtree(dom, child, styles, fragments, viewport, next);
    }
}

/// Return a fragment's physical content-box size after its computed padding
/// and borders are removed.
pub fn content_box_size(style: &ComputedValues, fragment: &TreeFragment) -> (f32, f32) {
    let em = match style.font_size {
        FontSize::Value(CssLengthPercentage::Length(Length {
            value,
            unit: livery::values::LengthUnit::Px,
        })) => value,
        _ => 16.0,
    };
    let padding_left = length_percentage_px(style.padding_left.0, em, fragment.width);
    let padding_right = length_percentage_px(style.padding_right.0, em, fragment.width);
    let padding_top = length_percentage_px(style.padding_top.0, em, fragment.width);
    let padding_bottom = length_percentage_px(style.padding_bottom.0, em, fragment.width);
    let border_left = border_width_px(style.border_left_style, style.border_left_width, em);
    let border_right = border_width_px(style.border_right_style, style.border_right_width, em);
    let border_top = border_width_px(style.border_top_style, style.border_top_width, em);
    let border_bottom = border_width_px(style.border_bottom_style, style.border_bottom_width, em);
    (
        (fragment.width - padding_left - padding_right - border_left - border_right).max(0.0),
        (fragment.height - padding_top - padding_bottom - border_top - border_bottom).max(0.0),
    )
}

fn layout_impl<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    image_sources: &ImageSources,
) -> Result<LiveryLayout<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let boxes = GeneratedBoxTree::from_dom(dom, styles);
    let mut state = BuildState {
        dom,
        styles,
        boxes: &boxes,
        tree: AlgorithmTree::new(),
        image_sources,
    };
    let children = boxes
        .roots()
        .iter()
        .filter_map(|box_id| {
            state
                .build_box(
                    *box_id,
                    None,
                    16.0,
                    (Some(viewport_width), Some(viewport_height)),
                )
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    // This synthetic box is the initial containing block, not an ordinary
    // auto-height document box. Its definite viewport dimensions are the
    // percentage basis for the root element and its definite-height chain.
    let initial_containing_block = BlockStyle {
        size: BlockDimensions::new(
            BlockSizeValue::Length(FlowLength::px(viewport_width)),
            BlockSizeValue::Length(FlowLength::px(viewport_height)),
        ),
        ..BlockStyle::default()
    };
    let root = state.tree.new_with_children_and_block_style(
        AlgorithmKind::Block,
        initial_containing_block,
        Style {
            display: Display::Block,
            size: Size {
                width: Dimension::length(viewport_width),
                height: Dimension::length(viewport_height),
            },
            ..Style::default()
        },
        &children,
        None,
    );

    state.tree.compute_layout_with_measure(
        root,
        AlgorithmSize::new(
            AlgorithmAvailableSpace::Definite(viewport_width),
            AlgorithmAvailableSpace::Definite(viewport_height),
        ),
        |known, available, _, context, _| {
            let Some(context) = context else {
                return AlgorithmSize::new(0.0, 0.0);
            };
            let available_width = match available.width {
                AlgorithmAvailableSpace::Definite(width) => width,
                AlgorithmAvailableSpace::MinContent => context.min_width,
                AlgorithmAvailableSpace::MaxContent => context.max_width,
            };
            AlgorithmSize::new(
                known
                    .width
                    .unwrap_or(context.max_width.min(available_width.max(0.0))),
                known.height.unwrap_or(context.height),
            )
        },
    );
    let (buckram_blocks, taffy_blocks) = state.tree.block_algorithm_counts();

    let mut fragments = FragmentTree::default();
    let mut output = FragmentOutput {
        fragments: &mut fragments,
    };
    collect_fragments(
        &state.tree,
        &boxes,
        root,
        Point { x: 0.0, y: 0.0 },
        None,
        &mut output,
    )?;
    drop(state);
    Ok(LiveryLayout::new(
        LayoutResult::new(boxes.into_tree(), fragments),
        None,
        BlockAlgorithmCounts {
            buckram: buckram_blocks,
            taffy: taffy_blocks,
        },
    ))
}

fn layout_atomic_subtrees<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    boxes: &GeneratedBoxTree<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    image_sources: &ImageSources,
) -> Result<AtomicLayoutPlane, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let roots = boxes
        .iter()
        .filter_map(|(box_id, css_box)| {
            let BoxOrigin::Element(node) = css_box.origin else {
                return None;
            };
            if boxes.principal_box(node) != Some(box_id)
                || css_box.display.outside != Some(DisplayOutside::Inline)
                || !is_atomic_inline_box(dom, styles, node)
            {
                return None;
            }
            (!has_atomic_inline_ancestor(dom, styles, boxes, node)).then_some(box_id)
        })
        .collect::<Vec<_>>();
    let mut plane = AtomicLayoutPlane::default();

    for box_id in roots {
        let mut state = BuildState {
            dom,
            styles,
            boxes,
            tree: AlgorithmTree::new(),
            image_sources,
        };
        let Some(atomic_root) = state.build_box(
            box_id,
            None,
            16.0,
            (Some(viewport_width), Some(viewport_height)),
        )?
        else {
            continue;
        };
        // An admitted atomic inline root needs a containing block so its
        // shrink-to-fit query runs as a child formatting context. Keep the
        // established direct-root path for the deferred cases, whose inline
        // placement may depend on unsupported vertical alignment behavior.
        let root = if state.tree.uses_intrinsic_shrink_to_fit(atomic_root) {
            state.tree.new_with_children_and_block_style(
                AlgorithmKind::Block,
                BlockStyle {
                    size: BlockDimensions::new(
                        BlockSizeValue::Length(FlowLength::px(viewport_width)),
                        BlockSizeValue::Length(FlowLength::px(viewport_height)),
                    ),
                    ..BlockStyle::default()
                },
                Style {
                    display: Display::Block,
                    size: Size {
                        width: Dimension::length(viewport_width),
                        height: Dimension::length(viewport_height),
                    },
                    ..Style::default()
                },
                &[atomic_root],
                None,
            )
        } else {
            atomic_root
        };
        state.tree.compute_layout_with_measure(
            root,
            AlgorithmSize::new(
                AlgorithmAvailableSpace::Definite(viewport_width),
                AlgorithmAvailableSpace::Definite(viewport_height),
            ),
            |known, available, _, context, _| {
                let Some(context) = context else {
                    return AlgorithmSize::new(0.0, 0.0);
                };
                let available_width = match available.width {
                    AlgorithmAvailableSpace::Definite(width) => width,
                    AlgorithmAvailableSpace::MinContent => context.min_width,
                    AlgorithmAvailableSpace::MaxContent => context.max_width,
                };
                AlgorithmSize::new(
                    known
                        .width
                        .unwrap_or(context.max_width.min(available_width.max(0.0))),
                    known.height.unwrap_or(context.height),
                )
            },
        );

        let mut fragments = Vec::new();
        collect_atomic_fragments(&state.tree, root, Point { x: 0.0, y: 0.0 }, &mut fragments);
        let Some(root_rect) = fragments
            .iter()
            .find_map(|(candidate, rect)| (*candidate == box_id).then_some(*rect))
        else {
            continue;
        };
        for (candidate, rect) in &mut fragments {
            rect.x -= root_rect.x;
            rect.y -= root_rect.y;
            plane.fragments.insert(*candidate, *rect);
        }
        plane.subtrees.push(AtomicSubtree {
            root: box_id,
            fragments,
        });
    }
    Ok(plane)
}

fn collect_atomic_fragments(
    tree: &AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
    node: AlgorithmNodeId,
    parent_origin: Point<f32>,
    output: &mut Vec<(BoxId, Fragment)>,
) {
    let computed = tree.layout(node);
    let origin = Point {
        x: parent_origin.x + computed.x,
        y: parent_origin.y + computed.y,
    };
    if let Some(box_id) = *tree.source(node) {
        output.push((
            box_id,
            Fragment {
                x: origin.x,
                y: origin.y,
                width: computed.width,
                height: computed.height,
            },
        ));
    }
    for child in tree.children(node) {
        collect_atomic_fragments(tree, *child, origin, output);
    }
}

fn merge_atomic_subtrees<Id>(
    atomic: &AtomicLayoutPlane,
    boxes: &GeneratedBoxTree<Id>,
    fragments: &mut FragmentTree,
) where
    Id: Copy + Eq + Hash,
{
    for subtree in &atomic.subtrees {
        let Some(root_id) = fragments
            .fragment_ids_for_box(subtree.root)
            .first()
            .copied()
        else {
            continue;
        };
        let Some(root_fragment) = fragments.get(root_id) else {
            continue;
        };
        let final_root = root_fragment.physical_rect();
        let local_root = subtree
            .fragments
            .iter()
            .find_map(|(box_id, rect)| (*box_id == subtree.root).then_some(*rect))
            .unwrap_or_default();
        let offset = (final_root.x - local_root.x, final_root.y - local_root.y);

        for (box_id, local) in &subtree.fragments {
            if *box_id == subtree.root || !fragments.fragment_ids_for_box(*box_id).is_empty() {
                continue;
            }
            let rect = Fragment {
                x: local.x + offset.0,
                y: local.y + offset.1,
                width: local.width,
                height: local.height,
            };
            let parent = boxes[*box_id]
                .parent()
                .and_then(|parent_box| fragments.fragment_ids_for_box(parent_box).last().copied())
                .or(Some(root_id));
            fragments.push(
                TreeFragment::from_horizontal_physical(*box_id, rect),
                parent,
                parent,
            );
        }
    }
}

fn layout_inline_groups<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    boxes: GeneratedBoxTree<D::NodeId>,
    viewport: (f32, f32),
    text: &mut TextSystem,
    atomic: &AtomicLayoutPlane,
    image_sources: &ImageSources,
) -> Result<LiveryLayout<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let (viewport_width, viewport_height) = viewport;
    let mut state = InlineBuildState {
        dom,
        styles,
        boxes: &boxes,
        atomic,
        tree: AlgorithmTree::new(),
        image_sources,
    };
    let children = boxes
        .roots()
        .iter()
        .filter_map(|box_id| state.build_box(*box_id, None, 16.0).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let root = state.tree.new_with_children_and_block_style(
        AlgorithmKind::Block,
        BlockStyle {
            size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(viewport_width)),
                BlockSizeValue::Length(FlowLength::px(viewport_height)),
            ),
            ..BlockStyle::default()
        },
        Style {
            display: Display::Block,
            size: Size {
                width: Dimension::length(viewport_width),
                height: Dimension::length(viewport_height),
            },
            ..Style::default()
        },
        &children,
        Vec::new(),
    );

    let mut intrinsic_sizes = IntrinsicSizeCache::default();
    state.tree.compute_layout_with_measure(
        root,
        AlgorithmSize::new(
            AlgorithmAvailableSpace::Definite(viewport_width),
            AlgorithmAvailableSpace::Definite(viewport_height),
        ),
        |known, available, _, context, line_constraints| {
            let Some(context) = context else {
                return AlgorithmSize::new(0.0, 0.0);
            };
            let (query_width, definite_cap, intrinsic_kind) = match available.width {
                AlgorithmAvailableSpace::Definite(width) => (width, Some(width), None),
                // A nearly-zero line asks Parley to break at every legal
                // opportunity while retaining each unbreakable item's width.
                AlgorithmAvailableSpace::MinContent => {
                    (0.01, None, Some(IntrinsicSizeKind::MinContent))
                },
                // An infinite line suppresses wrapping and yields max-content.
                AlgorithmAvailableSpace::MaxContent => {
                    (f32::INFINITY, None, Some(IntrinsicSizeKind::MaxContent))
                },
            };
            let intrinsic_width = intrinsic_kind.and_then(|kind| {
                let owner = context.owner?;
                let query = IntrinsicSizeQuery::new(owner, LogicalAxis::Inline, kind);
                intrinsic_sizes.get(query).or_else(|| {
                    let min_content = measure_inline_context(
                        text,
                        dom,
                        styles,
                        &boxes,
                        atomic,
                        context,
                        InlineMeasureGeometry {
                            width: 0.01,
                            line_constraints: None,
                        },
                    )
                    .0;
                    let max_content = measure_inline_context(
                        text,
                        dom,
                        styles,
                        &boxes,
                        atomic,
                        context,
                        InlineMeasureGeometry {
                            width: f32::INFINITY,
                            line_constraints: None,
                        },
                    )
                    .0;
                    let sizes = IntrinsicSizes::new(min_content, max_content)?;
                    let result = sizes.get(kind);
                    intrinsic_sizes.insert(owner, LogicalAxis::Inline, sizes);
                    Some(result)
                })
            });
            let requested_width = known.width.or(intrinsic_width).unwrap_or(query_width);
            let (measured_width, measured_height) = measure_inline_context(
                text,
                dom,
                styles,
                &boxes,
                atomic,
                context,
                InlineMeasureGeometry {
                    width: requested_width,
                    line_constraints: intrinsic_kind
                        .is_none()
                        .then_some(line_constraints)
                        .flatten(),
                },
            );
            AlgorithmSize::new(
                known.width.unwrap_or_else(|| {
                    intrinsic_width.unwrap_or_else(|| {
                        definite_cap.map_or(measured_width, |cap| measured_width.min(cap.max(0.0)))
                    })
                }),
                known.height.unwrap_or(measured_height),
            )
        },
    );
    let (buckram_blocks, taffy_blocks) = state.tree.block_algorithm_counts();

    let mut text_frame = TextFrame::default();
    let mut fragments = FragmentTree::default();
    let mut output = FragmentOutput {
        fragments: &mut fragments,
    };
    collect_inline_fragments(
        &state.tree,
        &boxes,
        root,
        FragmentCursor {
            origin: Point { x: 0.0, y: 0.0 },
            parent: None,
        },
        &mut output,
        &mut text_frame,
        styles,
    )?;
    drop(state);
    merge_atomic_subtrees(atomic, &boxes, &mut fragments);
    Ok(LiveryLayout::new(
        LayoutResult::new(boxes.into_tree(), fragments),
        Some(text_frame),
        BlockAlgorithmCounts {
            buckram: buckram_blocks,
            taffy: taffy_blocks,
        },
    ))
}

impl<D> InlineBuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn build_box(
        &mut self,
        box_id: BoxId,
        inherited: Option<&ComputedValues>,
        parent_font_size: f32,
    ) -> Result<Option<AlgorithmNodeId>, LayoutError> {
        match self.boxes[box_id].origin {
            BoxOrigin::Element(node) => {
                let computed = self.styles.get(node).cloned().unwrap_or_default();
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let mut inline_container_style = computed.clone();
                if matches!(
                    computed.position,
                    CssPosition::Absolute | CssPosition::Fixed
                ) {
                    // Once positioned, an inline element establishes a block
                    // container; its own vertical-align does not offset the
                    // text inside that container.
                    inline_container_style.vertical_align = VerticalAlign::Baseline;
                }
                let children = self.build_children(box_id, &inline_container_style, font_size)?;
                let mut taffy_style = to_taffy_style(&computed, font_size);
                apply_replaced_image_size(
                    &mut taffy_style,
                    self.dom,
                    node,
                    &computed,
                    self.image_sources,
                    font_size,
                );
                let block_style = to_block_style(self.boxes, box_id, &computed, font_size);
                let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    taffy_style,
                    &children,
                    vec![box_id],
                );
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                if block_style.float != FloatSide::None
                    && float_is_nested_in_inline(self.boxes, box_id)
                {
                    self.tree.mark_inline_context_float(node);
                }
                if supports_float_avoidance(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_float_avoidance(node);
                }
                if supports_intrinsic_shrink_to_fit(
                    &self.tree,
                    node,
                    &self.boxes[box_id],
                    &computed,
                    block_style,
                    kind,
                ) {
                    self.tree.enable_intrinsic_shrink_to_fit(node);
                }
                Ok(Some(node))
            },
            BoxOrigin::Text(_) => {
                let style = inherited.cloned().unwrap_or_default();
                self.build_inline_group(Some(box_id), &[box_id], &style, parent_font_size)
                    .map(Some)
            },
            BoxOrigin::Pseudo { .. } | BoxOrigin::Anonymous { .. } => {
                let computed = inherited.cloned().unwrap_or_default();
                let children = self.build_children(box_id, &computed, parent_font_size)?;
                let block_style = anonymous_block_style(self.boxes, box_id);
                let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    anonymous_taffy_style(&self.boxes[box_id]),
                    &children,
                    vec![box_id],
                );
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                Ok(Some(node))
            },
        }
    }

    fn build_children(
        &mut self,
        parent: BoxId,
        parent_style: &ComputedValues,
        parent_font_size: f32,
    ) -> Result<Vec<AlgorithmNodeId>, LayoutError> {
        // A `display: table` box takes its flattened cells directly, matching
        // the precomputed atomic subtree.
        if parent_style.display == CssDisplay::Table
            && self
                .boxes
                .origin_node(parent)
                .is_some_and(|node| table_is_flattenable(self.dom, self.styles, node))
        {
            let table = self
                .boxes
                .origin_node(parent)
                .expect("an element box has an origin node");
            let cells = table_cells(self.dom, self.styles, table);
            let mut children = Vec::with_capacity(cells.len());
            for (cell_node, row, column) in cells {
                let Some(cell) = self.boxes.principal_box(cell_node) else {
                    continue;
                };
                let Some(node) = self.build_box(cell, Some(parent_style), parent_font_size)? else {
                    continue;
                };
                place_table_cell(self.tree.style_mut(node), row, column);
                children.push(node);
            }
            return Ok(children);
        }
        self.build_flow_children(
            parent,
            self.boxes[parent].children().to_vec(),
            parent_style,
            parent_font_size,
        )
    }

    fn build_flow_children(
        &mut self,
        parent: BoxId,
        child_ids: Vec<BoxId>,
        parent_style: &ComputedValues,
        parent_font_size: f32,
    ) -> Result<Vec<AlgorithmNodeId>, LayoutError> {
        let intrinsic_owner = intrinsic_owner_for_flow_children(self.boxes, parent, &child_ids);
        let mut children = Vec::new();
        let mut inline_group = Vec::new();
        for child in child_ids {
            if box_is_inline(self.boxes, child) {
                inline_group.push(child);
                continue;
            }
            if !self.inline_group_is_blank(&inline_group, parent_style) {
                children.push(self.build_inline_group(
                    intrinsic_owner,
                    &inline_group,
                    parent_style,
                    parent_font_size,
                )?);
            }
            inline_group.clear();
            if let Some(node) = self.build_box(child, Some(parent_style), parent_font_size)? {
                children.push(node);
            }
        }
        if !self.inline_group_is_blank(&inline_group, parent_style) {
            children.push(self.build_inline_group(
                intrinsic_owner,
                &inline_group,
                parent_style,
                parent_font_size,
            )?);
        }
        Ok(children)
    }

    /// Whether a pending inline run generates no box at all.
    ///
    /// css-flexbox section 4 and css-grid section 6 both say a run of
    /// collapsible white space between two items generates no anonymous item.
    /// That matters because a flex or grid container turns every in-flow
    /// child into an item, so the ordinary newline-and-indent between two
    /// items would otherwise consume a cell and shift every following item by
    /// one position.
    ///
    /// **Deliberately scoped to those two container types.** White-space
    /// Buckram has already removed whitespace-only anonymous items before
    /// this lowering step.
    fn inline_group_is_blank(&self, roots: &[BoxId], _parent_style: &ComputedValues) -> bool {
        roots.is_empty()
    }

    fn build_inline_group(
        &mut self,
        owner: Option<BoxId>,
        roots: &[BoxId],
        parent_style: &ComputedValues,
        _parent_font_size: f32,
    ) -> Result<AlgorithmNodeId, LayoutError> {
        let width = roots
            .iter()
            .filter_map(|box_id| self.atomic.get(*box_id))
            .map(|fragment| fragment.width)
            .sum();
        let height = roots
            .iter()
            .filter_map(|box_id| self.atomic.get(*box_id))
            .map(|fragment| fragment.height)
            .fold(0.0_f32, f32::max);
        let flow = roots
            .first()
            .map_or(FlowAxes::HORIZONTAL_LTR, |root| self.boxes[*root].flow);
        let containing_flow = roots
            .first()
            .and_then(|root| self.boxes[*root].parent())
            .map_or(flow, |parent| self.boxes[parent].flow);
        let node = self.tree.new_leaf_with_context_and_block_style(
            BlockStyle::anonymous(flow, containing_flow),
            Style {
                display: Display::Block,
                ..Style::default()
            },
            InlineMeasure {
                owner,
                roots: roots.to_vec(),
                style: parent_style.clone(),
                width,
                height,
                layouts: Vec::new(),
                placement_constraints: None,
            },
            roots.to_vec(),
        );
        if parent_style.text_wrap_mode == livery::values::TextWrapMode::Wrap {
            self.tree.enable_float_line_constraints(node);
        }
        Ok(node)
    }
}

impl<D> BuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn build_box(
        &mut self,
        box_id: BoxId,
        inherited: Option<&ComputedValues>,
        parent_font_size: f32,
        containing_size: (Option<f32>, Option<f32>),
    ) -> Result<Option<AlgorithmNodeId>, LayoutError> {
        match self.boxes[box_id].origin {
            BoxOrigin::Element(node) => {
                let computed = self.styles.get(node).cloned().unwrap_or_default();
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let mut child_containing_size =
                    resolved_child_containing_size(&computed, font_size, containing_size);
                if self
                    .dom
                    .parent(node)
                    .is_some_and(|parent| self.dom.kind(parent) == NodeKind::Document)
                {
                    // The root element's containing block is the initial
                    // containing block. Preserve its definite block size for
                    // percentage-height descendants even when the root's own
                    // height is auto.
                    child_containing_size.1 = child_containing_size.1.or(containing_size.1);
                }
                // A `display: table` box takes its flattened cells directly,
                // so the row-group and row boxes never enter the tree.
                let children = if computed.display == CssDisplay::Table
                    && table_is_flattenable(self.dom, self.styles, node)
                {
                    let cells = table_cells(self.dom, self.styles, node);
                    let mut children = Vec::with_capacity(cells.len());
                    for (cell_node, row, column) in cells {
                        let Some(cell) = self.boxes.principal_box(cell_node) else {
                            continue;
                        };
                        let Some(taffy_node) = self.build_box(
                            cell,
                            Some(&computed),
                            font_size,
                            child_containing_size,
                        )?
                        else {
                            continue;
                        };
                        place_table_cell(self.tree.style_mut(taffy_node), row, column);
                        children.push(taffy_node);
                    }
                    children
                } else {
                    self.boxes[box_id]
                        .children()
                        .iter()
                        .filter_map(|child| {
                            self.build_box(
                                *child,
                                Some(&computed),
                                font_size,
                                child_containing_size,
                            )
                            .transpose()
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
                let mut taffy_style = to_taffy_style(&computed, font_size);
                // CSS 2.1 section 17.5.2.1: a fixed table's columns are sized
                // from the first row, so they can be pinned as explicit grid
                // tracks before anything is measured.
                if let Some(columns) = fixed_column_widths(
                    self.dom,
                    self.styles,
                    node,
                    &computed,
                    font_size,
                    containing_size.0,
                ) {
                    taffy_style.grid_template_columns = columns.into_iter().map(length).collect();
                }
                taffy_style.size.width =
                    dimension_with_basis(computed.width, font_size, containing_size.0);
                taffy_style.size.height =
                    dimension_with_basis(computed.height, font_size, containing_size.1);
                taffy_style.min_size.width =
                    dimension_with_basis(computed.min_width, font_size, containing_size.0);
                taffy_style.min_size.height =
                    dimension_with_basis(computed.min_height, font_size, containing_size.1);
                taffy_style.max_size.width =
                    dimension_with_basis(computed.max_width, font_size, containing_size.0);
                taffy_style.max_size.height =
                    dimension_with_basis(computed.max_height, font_size, containing_size.1);
                apply_replaced_image_size(
                    &mut taffy_style,
                    self.dom,
                    node,
                    &computed,
                    self.image_sources,
                    font_size,
                );
                let block_style = to_block_style(self.boxes, box_id, &computed, font_size);
                let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    taffy_style,
                    &children,
                    Some(box_id),
                );
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                if block_style.float != FloatSide::None
                    && float_is_nested_in_inline(self.boxes, box_id)
                {
                    self.tree.mark_inline_context_float(node);
                }
                if supports_float_avoidance(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_float_avoidance(node);
                }
                if supports_intrinsic_shrink_to_fit(
                    &self.tree,
                    node,
                    &self.boxes[box_id],
                    &computed,
                    block_style,
                    kind,
                ) {
                    self.tree.enable_intrinsic_shrink_to_fit(node);
                }
                Ok(Some(node))
            },
            BoxOrigin::Text(node) => {
                let text = self.dom.text(node).unwrap_or("");
                let preserves_whitespace = inherited.is_some_and(|style| {
                    matches!(
                        style.white_space_collapse,
                        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
                    )
                });
                if text.is_empty() || (!preserves_whitespace && is_collapsible_whitespace(text)) {
                    return Ok(None);
                }
                let font_size = parent_font_size;
                let line_height = inherited
                    .map(|style| line_height_px(&style.line_height, font_size))
                    .unwrap_or(font_size * 1.2);
                let min_width = if preserves_whitespace {
                    text.lines()
                        .map(|line| line.chars().count())
                        .max()
                        .unwrap_or(0)
                } else {
                    collapsed_word_width(text)
                } as f32
                    * font_size
                    * 0.6;
                let max_width = if preserves_whitespace {
                    min_width
                } else {
                    collapsed_text_width(text) as f32 * font_size * 0.6
                };
                let line_count = if preserves_whitespace {
                    text.lines().count().max(1)
                } else {
                    1
                };
                let height = line_count as f32 * line_height;
                let node = self.tree.new_leaf_with_context_and_block_style(
                    anonymous_block_style(self.boxes, box_id),
                    Style {
                        display: Display::Block,
                        ..Style::default()
                    },
                    TextMeasure {
                        min_width,
                        max_width,
                        height,
                    },
                    Some(box_id),
                );
                Ok(Some(node))
            },
            BoxOrigin::Pseudo { .. } | BoxOrigin::Anonymous { .. } => {
                let computed = inherited.cloned().unwrap_or_default();
                let children = self.boxes[box_id]
                    .children()
                    .iter()
                    .filter_map(|child| {
                        self.build_box(*child, Some(&computed), parent_font_size, containing_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let block_style = anonymous_block_style(self.boxes, box_id);
                let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    anonymous_taffy_style(&self.boxes[box_id]),
                    &children,
                    Some(box_id),
                );
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                Ok(Some(node))
            },
        }
    }
}

struct FragmentOutput<'a> {
    fragments: &'a mut FragmentTree,
}

#[derive(Clone, Copy)]
struct FragmentCursor {
    origin: Point<f32>,
    parent: Option<FragmentId>,
}

fn intrinsic_owner_for_flow_children<Id>(
    boxes: &GeneratedBoxTree<Id>,
    parent: BoxId,
    children: &[BoxId],
) -> Option<BoxId>
where
    Id: Copy + Eq + Hash,
{
    let mut groups = 0;
    let mut inside_group = false;
    for child in children {
        if box_is_inline(boxes, *child) {
            if !inside_group {
                groups += 1;
                inside_group = true;
            }
        } else {
            inside_group = false;
        }
    }
    (groups == 1).then_some(parent)
}

fn box_is_inline<Id>(boxes: &GeneratedBoxTree<Id>, box_id: BoxId) -> bool
where
    Id: Copy + Eq + Hash,
{
    let css_box = &boxes[box_id];
    css_box.display.outside == Some(DisplayOutside::Inline)
        && css_box.float == FloatSide::None
        && matches!(
            css_box.positioning,
            PositioningScheme::Static | PositioningScheme::Relative | PositioningScheme::Sticky
        )
}

fn float_is_nested_in_inline<Id>(boxes: &GeneratedBoxTree<Id>, box_id: BoxId) -> bool
where
    Id: Copy + Eq + Hash,
{
    let mut ancestor = boxes[box_id].parent();
    while let Some(box_id) = ancestor {
        let css_box = &boxes[box_id];
        if css_box.float != FloatSide::None
            || matches!(
                css_box.positioning,
                PositioningScheme::Absolute | PositioningScheme::Fixed
            )
            || matches!(
                css_box.display.inside,
                Some(
                    DisplayInside::FlowRoot
                        | DisplayInside::Flex
                        | DisplayInside::Grid
                        | DisplayInside::Table
                )
            )
        {
            return false;
        }
        match css_box.display.outside {
            Some(DisplayOutside::Inline) => return true,
            Some(DisplayOutside::Block) => return false,
            Some(DisplayOutside::RunIn) | None => ancestor = css_box.parent(),
        }
    }
    false
}

fn anonymous_block_style<Id>(boxes: &GeneratedBoxTree<Id>, box_id: BoxId) -> BlockStyle
where
    Id: Copy + Eq + Hash,
{
    let flow = boxes[box_id].flow;
    let containing_flow = boxes[box_id]
        .parent()
        .map_or(flow, |parent| boxes[parent].flow);
    BlockStyle::anonymous(flow, containing_flow)
}

fn to_block_style<Id>(
    boxes: &GeneratedBoxTree<Id>,
    box_id: BoxId,
    computed: &ComputedValues,
    font_size: f32,
) -> BlockStyle
where
    Id: Copy + Eq + Hash,
{
    let css_box = &boxes[box_id];
    let containing_flow = css_box
        .parent()
        .map_or(FlowAxes::HORIZONTAL_LTR, |parent| boxes[parent].flow);
    let mut size_containment = match computed.container_type {
        ContainerType::Normal => BlockDimensions::new(false, false),
        ContainerType::InlineSize if computed.writing_mode.is_vertical() => {
            BlockDimensions::new(false, true)
        },
        ContainerType::InlineSize => BlockDimensions::new(true, false),
        ContainerType::Size => BlockDimensions::new(true, true),
    };
    if computed.contain.has_size() {
        size_containment = BlockDimensions::new(true, true);
    } else if computed.contain.has_inline_size() {
        if computed.writing_mode.is_vertical() {
            size_containment.height = true;
        } else {
            size_containment.width = true;
        }
    }
    let establishes_bfc = matches!(
        css_box.display.inside,
        Some(DisplayInside::FlowRoot | DisplayInside::Flex | DisplayInside::Grid)
    ) || matches!(
        computed.display,
        CssDisplay::InlineBlock
            | CssDisplay::Table
            | CssDisplay::TableCell
            | CssDisplay::TableCaption
    ) || matches!(
        computed.position,
        CssPosition::Absolute | CssPosition::Fixed
    ) || computed.float != CssFloat::None
        || computed.overflow_x != CssOverflow::Visible
        || computed.overflow_y != CssOverflow::Visible
        || computed.contain.has_layout()
        || computed.contain.has_paint();

    BlockStyle {
        flow: css_box.flow,
        containing_flow,
        size: BlockDimensions::new(
            block_size_value(computed.width, font_size),
            block_size_value(computed.height, font_size),
        ),
        min_size: BlockDimensions::new(
            block_size_value(computed.min_width, font_size),
            block_size_value(computed.min_height, font_size),
        ),
        max_size: BlockDimensions::new(
            block_size_value(computed.max_width, font_size),
            block_size_value(computed.max_height, font_size),
        ),
        margin: PhysicalSides {
            top: block_margin(computed.margin_top, font_size),
            right: block_margin(computed.margin_right, font_size),
            bottom: block_margin(computed.margin_bottom, font_size),
            left: block_margin(computed.margin_left, font_size),
        },
        padding: PhysicalSides {
            top: flow_length(computed.padding_top.0, font_size),
            right: flow_length(computed.padding_right.0, font_size),
            bottom: flow_length(computed.padding_bottom.0, font_size),
            left: flow_length(computed.padding_left.0, font_size),
        },
        border: PhysicalSides {
            top: border_width_px(
                computed.border_top_style,
                computed.border_top_width,
                font_size,
            ),
            right: border_width_px(
                computed.border_right_style,
                computed.border_right_width,
                font_size,
            ),
            bottom: border_width_px(
                computed.border_bottom_style,
                computed.border_bottom_width,
                font_size,
            ),
            left: border_width_px(
                computed.border_left_style,
                computed.border_left_width,
                font_size,
            ),
        },
        box_sizing: match computed.box_sizing {
            CssBoxSizing::ContentBox => BlockBoxSizing::ContentBox,
            CssBoxSizing::BorderBox => BlockBoxSizing::BorderBox,
        },
        position: match computed.position {
            CssPosition::Static => BuckramBlockPosition::Static,
            CssPosition::Relative => BuckramBlockPosition::Relative,
            CssPosition::Absolute => BuckramBlockPosition::Absolute,
            CssPosition::Fixed => BuckramBlockPosition::Fixed,
            CssPosition::Sticky => BuckramBlockPosition::Sticky,
        },
        float: match computed.float {
            CssFloat::None => FloatSide::None,
            CssFloat::Left => FloatSide::Left,
            CssFloat::Right => FloatSide::Right,
        },
        clear: match computed.clear {
            CssClear::None => ClearSide::None,
            CssClear::Left => ClearSide::Left,
            CssClear::Right => ClearSide::Right,
            CssClear::Both => ClearSide::Both,
        },
        establishes_bfc,
        shrink_to_fit: matches!(computed.width, CssSize::Auto)
            && (computed.display == CssDisplay::InlineBlock || computed.float != CssFloat::None),
        replaced: css_box.replaced,
        aspect_ratio: match computed.aspect_ratio {
            AspectRatio::Auto => None,
            AspectRatio::Ratio(value) => Some(value),
        },
        size_containment,
        has_nonlinear_lengths: block_style_has_nonlinear_lengths(computed),
        is_root_element: css_box.parent().is_none()
            && matches!(css_box.origin, BoxOrigin::Element(_)),
    }
}

fn block_size_value(value: CssSize, em: f32) -> BlockSizeValue {
    match value {
        CssSize::Auto => BlockSizeValue::Auto,
        CssSize::None => BlockSizeValue::None,
        CssSize::MinContent => BlockSizeValue::MinContent,
        CssSize::MaxContent => BlockSizeValue::MaxContent,
        CssSize::FitContent(value) => BlockSizeValue::FitContent(flow_length(value, em)),
        CssSize::Value(value) => BlockSizeValue::Length(flow_length(value, em)),
    }
}

fn block_margin(value: Margin, em: f32) -> FlowLengthAuto {
    match value {
        Margin::Auto => FlowLengthAuto::Auto,
        Margin::Value(value) => FlowLengthAuto::Value(flow_length(value, em)),
    }
}

fn flow_length(value: CssLengthPercentage, em: f32) -> FlowLength {
    let px = absolute_length_percentage(value, em, 16.0, 0.0);
    let with_unit_basis = absolute_length_percentage(value, em, 16.0, 1.0);
    FlowLength {
        px,
        percentage: with_unit_basis - px,
    }
}

fn block_style_has_nonlinear_lengths(computed: &ComputedValues) -> bool {
    let size_has_math = |size| match size {
        CssSize::FitContent(value) | CssSize::Value(value) => length_has_math(value),
        CssSize::Auto | CssSize::None | CssSize::MinContent | CssSize::MaxContent => false,
    };
    let margin_has_math = |margin| match margin {
        Margin::Value(value) => length_has_math(value),
        Margin::Auto => false,
    };

    [
        computed.width,
        computed.height,
        computed.min_width,
        computed.min_height,
        computed.max_width,
        computed.max_height,
    ]
    .into_iter()
    .any(size_has_math)
        || [
            computed.margin_top,
            computed.margin_right,
            computed.margin_bottom,
            computed.margin_left,
        ]
        .into_iter()
        .any(margin_has_math)
        || [
            computed.padding_top.0,
            computed.padding_right.0,
            computed.padding_bottom.0,
            computed.padding_left.0,
        ]
        .into_iter()
        .any(length_has_math)
}

fn length_has_math(value: CssLengthPercentage) -> bool {
    matches!(value, CssLengthPercentage::Math(_))
}

fn supports_nested_float_state<Id>(
    css_box: &CssBox<Id>,
    block_style: BlockStyle,
    kind: AlgorithmKind,
) -> bool {
    kind == AlgorithmKind::Block
        && css_box.display.outside == Some(DisplayOutside::Block)
        // A generated block-formatting root can contain split inline
        // continuations whose floated descendants no longer retain enough
        // inline provenance for safe float-state transfer.
        && css_box.formatting_context != Some(FormattingContextKind::Block)
        && css_box.display.internal_table.is_none()
        && !block_style.establishes_bfc
        && block_style.position == BuckramBlockPosition::Static
        && block_style.float == FloatSide::None
        && !block_style.replaced
        && block_style.flow == block_style.containing_flow
}

fn supports_float_avoidance<Id>(
    css_box: &CssBox<Id>,
    block_style: BlockStyle,
    kind: AlgorithmKind,
) -> bool {
    matches!(
        kind,
        AlgorithmKind::Leaf | AlgorithmKind::Block | AlgorithmKind::Flex | AlgorithmKind::Grid
    ) && (css_box.display.outside == Some(DisplayOutside::Block)
        || (css_box.display.outside == Some(DisplayOutside::Inline) && block_style.shrink_to_fit))
        && matches!(
            css_box.display.inside,
            Some(
                DisplayInside::Flow
                    | DisplayInside::FlowRoot
                    | DisplayInside::Flex
                    | DisplayInside::Grid
            ) | None
        )
        && css_box.display.internal_table.is_none()
        && block_style.establishes_bfc
        && block_style.position == BuckramBlockPosition::Static
        && block_style.float == FloatSide::None
        && !block_style.replaced
        && block_style.flow.is_horizontal()
        && block_style.containing_flow.is_horizontal()
}

fn supports_intrinsic_shrink_to_fit<Id, Context, Source>(
    tree: &AlgorithmTree<Style, Context, Source>,
    node: AlgorithmNodeId,
    css_box: &CssBox<Id>,
    computed: &ComputedValues,
    block_style: BlockStyle,
    kind: AlgorithmKind,
) -> bool {
    let float_root = css_box.display.outside == Some(DisplayOutside::Block)
        && block_style.float != FloatSide::None;
    let atomic_inline_root = css_box.display.outside == Some(DisplayOutside::Inline)
        && block_style.float == FloatSide::None;
    kind == AlgorithmKind::Block
        && matches!(
            css_box.display.inside,
            Some(DisplayInside::Flow | DisplayInside::FlowRoot)
        )
        && css_box.display.internal_table.is_none()
        && block_style.position == BuckramBlockPosition::Static
        && block_style.shrink_to_fit
        && !block_style.replaced
        && computed.vertical_align == VerticalAlign::Baseline
        && block_style.flow.is_horizontal()
        && block_style.containing_flow.is_horizontal()
        && (float_root || atomic_inline_root)
        && tree.supports_intrinsic_shrink_to_fit(node)
}

fn algorithm_kind<Id>(css_box: &CssBox<Id>, leaf: bool) -> AlgorithmKind {
    if leaf {
        return AlgorithmKind::Leaf;
    }
    match (css_box.formatting_context, css_box.display.internal_table) {
        (_, Some(InternalTableRole::Row)) => AlgorithmKind::Flex,
        (Some(FormattingContextKind::Flex), _) => AlgorithmKind::Flex,
        (Some(FormattingContextKind::Grid | FormattingContextKind::Table), _) => {
            AlgorithmKind::Grid
        },
        _ => AlgorithmKind::Block,
    }
}

fn anonymous_taffy_style<Id>(css_box: &CssBox<Id>) -> Style {
    let display = match (css_box.formatting_context, css_box.display.internal_table) {
        (_, Some(InternalTableRole::Row)) => Display::Flex,
        (Some(FormattingContextKind::Flex), _) => Display::Flex,
        (Some(FormattingContextKind::Grid | FormattingContextKind::Table), _) => Display::Grid,
        _ => Display::Block,
    };
    Style {
        display,
        ..Style::default()
    }
}

fn legacy_origin_node<Id>(boxes: &GeneratedBoxTree<Id>, box_id: BoxId) -> Option<Id>
where
    Id: Copy + Eq + Hash,
{
    match boxes[box_id].origin {
        BoxOrigin::Element(node) if boxes.principal_box(node) == Some(box_id) => Some(node),
        BoxOrigin::Text(node) => Some(node),
        BoxOrigin::Element(_) | BoxOrigin::Pseudo { .. } | BoxOrigin::Anonymous { .. } => None,
    }
}

fn collect_fragments<Id>(
    tree: &AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
    boxes: &GeneratedBoxTree<Id>,
    node: AlgorithmNodeId,
    parent_origin: Point<f32>,
    parent_fragment: Option<FragmentId>,
    output: &mut FragmentOutput<'_>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node);
    let origin = Point {
        x: parent_origin.x + computed.x,
        y: parent_origin.y + computed.y,
    };
    let mut child_parent = parent_fragment;
    {
        let source = *tree.source(node);
        let rect = Fragment {
            x: origin.x,
            y: origin.y,
            width: computed.width,
            height: computed.height,
        };
        let origin_node = match source {
            Some(box_id) => {
                let structural_parent = boxes[box_id].parent().and_then(|parent_box| {
                    output
                        .fragments
                        .fragment_ids_for_box(parent_box)
                        .last()
                        .copied()
                });
                let parent = structural_parent.or(parent_fragment);
                child_parent = Some(output.fragments.push(
                    TreeFragment::from_horizontal_physical(box_id, rect),
                    parent,
                    parent,
                ));
                legacy_origin_node(boxes, box_id)
            },
            None => None,
        };
        let _ = origin_node;
    }
    for child in tree.children(node) {
        collect_fragments(tree, boxes, *child, origin, child_parent, output)?;
    }
    Ok(())
}

fn collect_inline_fragments<Id>(
    tree: &AlgorithmTree<Style, InlineMeasure, Vec<BoxId>>,
    boxes: &GeneratedBoxTree<Id>,
    node: AlgorithmNodeId,
    cursor: FragmentCursor,
    output: &mut FragmentOutput<'_>,
    text_frame: &mut TextFrame<Id>,
    styles: &StylePlane<Id>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node);
    let origin = Point {
        x: cursor.origin.x + computed.x,
        y: cursor.origin.y + computed.y,
    };
    let placement = if let Some(context) = tree.context(node)
        && let Some(layout) = context.layout_for_width(computed.width)
    {
        Some(layout.place(
            text_frame,
            styles,
            |box_id| boxes.origin_node(box_id),
            (origin.x, origin.y),
            computed.width,
        ))
    } else {
        None
    };
    let mut child_parent = cursor.parent;
    {
        let mut source_ids = tree.source(node).clone();
        if let Some(placement) = &placement {
            source_ids.extend(placement.fragments.keys().copied());
        }
        source_ids.sort_unstable();
        source_ids.dedup();
        let rect = Fragment {
            x: origin.x,
            y: origin.y,
            width: computed.width,
            height: computed.height,
        };
        for box_id in source_ids {
            let structural_parent = boxes[box_id].parent().and_then(|parent_box| {
                output
                    .fragments
                    .fragment_ids_for_box(parent_box)
                    .last()
                    .copied()
            });
            let parent = structural_parent.or(cursor.parent);
            let line_fragments = placement
                .as_ref()
                .and_then(|placement| placement.fragments.get(&box_id))
                .filter(|fragments| !fragments.is_empty());
            if let Some(line_fragments) = line_fragments {
                for line_fragment in line_fragments {
                    let fragment_id = output.fragments.push(
                        TreeFragment::from_horizontal_physical(box_id, *line_fragment),
                        parent,
                        parent,
                    );
                    child_parent.get_or_insert(fragment_id);
                }
            } else {
                let fragment_id = output.fragments.push(
                    TreeFragment::from_horizontal_physical(box_id, rect),
                    parent,
                    parent,
                );
                child_parent.get_or_insert(fragment_id);
            }
        }
    }
    for child in tree.children(node) {
        collect_inline_fragments(
            tree,
            boxes,
            *child,
            FragmentCursor {
                origin,
                parent: child_parent,
            },
            output,
            text_frame,
            styles,
        )?;
    }
    Ok(())
}

/// The cells of a `display: table` box, flattened to `(cell, row, column)`.
///
/// Livery lays a table out as a grid: the row-group and row nesting collapses
/// away and every cell carries an explicit grid position. That is the same
/// shape the incumbent lane uses (`genet-layout`'s `box_tree` builds the
/// identical structure), which is why a table renders at all without a table
/// algorithm in taffy.
///
/// Deferred here exactly as in the incumbent: `border-collapse`, caption
/// placement, `colgroup`, row and column spans, and real fixed or auto table
/// sizing. Tracks are implicit and auto-sized, so column widths come from
/// content rather than from the first row.
fn table_cells<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    table: D::NodeId,
) -> Vec<(D::NodeId, u16, u16)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn display_of<D>(styles: &StylePlane<D::NodeId>, id: D::NodeId) -> Option<CssDisplay>
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        styles.get(id).map(|style| style.display)
    }

    fn walk<D>(
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        container: D::NodeId,
        row: &mut u16,
        out: &mut Vec<(D::NodeId, u16, u16)>,
    ) where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        for child in dom.dom_children(container) {
            match display_of::<D>(styles, child) {
                Some(CssDisplay::TableRow) => {
                    let mut column = 0u16;
                    for cell in dom.dom_children(child) {
                        if display_of::<D>(styles, cell) == Some(CssDisplay::TableCell) {
                            out.push((cell, *row, column));
                            column += 1;
                        }
                    }
                    *row += 1;
                },
                Some(CssDisplay::TableRowGroup) => walk(dom, styles, child, row, out),
                // Captions, colgroups, and stray content are not placed in
                // the first-cut grid.
                _ => {},
            }
        }
    }

    let mut out = Vec::new();
    let mut row = 0u16;
    walk(dom, styles, table, &mut row, &mut out);
    out
}

/// Whether a table's row-group and row boxes may be flattened away.
///
/// Flattening drops those boxes from the layout tree, which is fine while
/// they only carry structure. A `position: relative` row or row group also
/// carries an offset that its cells must inherit, and with the box gone there
/// is nothing left to apply it. The incumbent lane keeps a side list of
/// "cells owed a row-relative shift" for exactly this; Livery does not
/// resolve those offsets yet, so a positioned row or group turns flattening
/// off for that table and it falls back to the previous nesting.
///
/// Measured 2026-07-26: without this guard the sixteen
/// `css-position/position-relative-table-*` files regress. Resolving the
/// shift onto the cells is the real fix and is deferred, not unknown.
fn table_is_flattenable<D>(dom: &D, styles: &StylePlane<D::NodeId>, table: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn positioned<D>(styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        styles
            .get(id)
            .is_some_and(|style| style.position != CssPosition::Static)
    }

    fn walk<D>(dom: &D, styles: &StylePlane<D::NodeId>, container: D::NodeId) -> bool
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        for child in dom.dom_children(container) {
            match styles.get(child).map(|style| style.display) {
                Some(CssDisplay::TableRow) => {
                    if positioned::<D>(styles, child) {
                        return false;
                    }
                },
                Some(CssDisplay::TableRowGroup)
                    if positioned::<D>(styles, child) || !walk(dom, styles, child) =>
                {
                    return false;
                },
                _ => {},
            }
        }
        true
    }

    walk(dom, styles, table)
}

/// Column widths for a `table-layout: fixed` table, per CSS 2.1 section
/// 17.5.2.1.
///
/// The fixed algorithm reads widths only from the first row (and from
/// `<col>`, not yet modelled here), never from content, which is what makes
/// it computable before layout. A cell's `width` is a content-box width, so
/// the column it establishes is that width plus the cell's horizontal
/// padding and border. Columns left auto share what remains of the table's
/// content width equally.
///
/// Returns `None` when the algorithm does not apply, which leaves the table
/// on auto-sized implicit tracks: no `table-layout: fixed`, no definite
/// table width, or no cells to read.
fn fixed_column_widths<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    table: D::NodeId,
    computed: &ComputedValues,
    font_size: f32,
    containing_width: Option<f32>,
) -> Option<Vec<f32>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if computed.table_layout != CssTableLayout::Fixed {
        return None;
    }
    let cells = table_cells(dom, styles, table);
    let columns = cells.iter().map(|(_, _, col)| *col + 1).max()? as usize;

    // The table's own content width: its used width less its border and
    // padding, since the column widths fill the content box.
    let table_width = resolved_explicit_size(computed.width, font_size, containing_width)?;
    let inner = match computed.box_sizing {
        CssBoxSizing::BorderBox => {
            table_width - horizontal_edges(computed, font_size, containing_width)
        },
        CssBoxSizing::ContentBox => table_width,
    };
    if !inner.is_finite() || inner <= 0.0 {
        return None;
    }

    // First-row cells set the columns; later rows are ignored by the
    // algorithm, which is the point of it.
    let mut widths: Vec<Option<f32>> = vec![None; columns];
    for (cell, row, column) in &cells {
        if *row != 0 {
            continue;
        }
        let Some(cell_style) = styles.get(*cell) else {
            continue;
        };
        let Some(width) = resolved_explicit_size(cell_style.width, font_size, Some(inner)) else {
            continue;
        };
        let border_box = match cell_style.box_sizing {
            CssBoxSizing::BorderBox => width,
            CssBoxSizing::ContentBox => {
                width + horizontal_edges(cell_style, font_size, Some(inner))
            },
        };
        widths[*column as usize] = Some(border_box.max(0.0));
    }

    let fixed: f32 = widths.iter().flatten().sum();
    let auto_columns = widths.iter().filter(|width| width.is_none()).count();
    let share = if auto_columns > 0 {
        ((inner - fixed) / auto_columns as f32).max(0.0)
    } else {
        0.0
    };
    Some(
        widths
            .into_iter()
            .map(|width| width.unwrap_or(share))
            .collect(),
    )
}

/// A box's horizontal border plus padding.
fn horizontal_edges(computed: &ComputedValues, font_size: f32, basis: Option<f32>) -> f32 {
    let basis = basis.unwrap_or(0.0);
    length_percentage_px(computed.padding_left.0, font_size, basis).max(0.0)
        + length_percentage_px(computed.padding_right.0, font_size, basis).max(0.0)
        + border_width_px(
            computed.border_left_style,
            computed.border_left_width,
            font_size,
        )
        + border_width_px(
            computed.border_right_style,
            computed.border_right_width,
            font_size,
        )
}

/// Pin a cell's taffy style to its flattened grid position.
fn place_table_cell(style: &mut Style, row: u16, column: u16) {
    style.grid_row = Line {
        start: line(row as i16 + 1),
        end: GridPlacement::Auto,
    };
    style.grid_column = Line {
        start: line(column as i16 + 1),
        end: GridPlacement::Auto,
    };
}

/// Whether every character is CSS collapsible white space.
///
/// CSS collapsible white space is exactly space, tab, line feed, carriage
/// return, and form feed (css-text-3 section 3). It is deliberately *not*
/// Rust's `char::is_whitespace`, which also matches U+00A0 no-break space
/// and the other Unicode spaces. Those generate content: `&nbsp;` is the
/// standard way a test forces a line box to exist, so trimming it away
/// silently deletes the line.
fn is_collapsible_whitespace(text: &str) -> bool {
    text.chars()
        .all(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}'))
}

fn is_atomic_inline_box<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    styles.get(id).is_some_and(|style| {
        style.display == CssDisplay::InlineBlock
            || (style.display == CssDisplay::Inline && is_replaced_element(dom, id))
    })
}

fn has_atomic_inline_ancestor<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    boxes: &GeneratedBoxTree<D::NodeId>,
    id: D::NodeId,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut ancestor = dom.parent(id);
    while let Some(candidate) = ancestor {
        if boxes
            .principal_box(candidate)
            .is_some_and(|box_id| boxes[box_id].display.outside == Some(DisplayOutside::Inline))
            && is_atomic_inline_box(dom, styles, candidate)
        {
            return true;
        }
        ancestor = dom.parent(candidate);
    }
    false
}

/// Return the topmost pointer-events-enabled element whose layout fragment
/// contains a scene point. The walk mirrors the lane's DOM paint order for the
/// bounded stacking subset: numeric z-index first, then source order.
pub fn hit_test<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    x: f32,
    y: f32,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    hit_test_with_scroll(dom, styles, fragments, &HashMap::new(), x, y)
}

/// Hit-test a retained fragment plane after applying per-element scroll
/// offsets to descendants. The ordinary [`hit_test`] path keeps the map empty;
/// retained sessions use this variant for wheel-scrolled containers.
pub(crate) fn hit_test_with_scroll<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &LiveryLayout<D::NodeId>,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    x: f32,
    y: f32,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut state = HitTestState {
        dom,
        styles,
        fragments,
        scroll_offsets,
        x,
        y,
        clips: Vec::new(),
        order: 0,
        candidates: Vec::new(),
    };
    collect_hit_candidates(&mut state, dom.document(), (0.0, 0.0));
    state
        .candidates
        .into_iter()
        .max_by_key(|candidate| (candidate.level, candidate.order))
        .map(|candidate| candidate.id)
}

struct HitCandidate<Id> {
    id: Id,
    level: i32,
    order: u64,
}

struct HitTestState<'a, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &'a LiveryLayout<D::NodeId>,
    scroll_offsets: &'a HashMap<D::NodeId, (f32, f32)>,
    x: f32,
    y: f32,
    clips: Vec<(f32, f32, f32, f32)>,
    order: u64,
    candidates: Vec<HitCandidate<D::NodeId>>,
}

fn collect_hit_candidates<D>(
    state: &mut HitTestState<'_, D>,
    id: D::NodeId,
    ancestor_scroll: (f32, f32),
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let style = state.styles.get(id);
    let fragment = state.fragments.get(id);
    let visible_fragment = fragment.map(|fragment| Fragment {
        x: fragment.x - ancestor_scroll.0,
        y: fragment.y - ancestor_scroll.1,
        ..fragment.physical_rect()
    });
    let inside_clips = state.clips.iter().all(|(left, top, right, bottom)| {
        state.x >= *left && state.x <= *right && state.y >= *top && state.y <= *bottom
    });
    if state.dom.kind(id) == NodeKind::Element
        && let (Some(style), Some(fragment)) = (style, visible_fragment)
        && style.display != CssDisplay::None
        && style.visibility == livery::values::Visibility::Visible
        && style.pointer_events == livery::values::PointerEvents::Auto
        && inside_clips
        && state.x >= fragment.x
        && state.x <= fragment.x + fragment.width
        && state.y >= fragment.y
        && state.y <= fragment.y + fragment.height
    {
        let level = match style.z_index {
            livery::values::ZIndex::Integer(level) => level,
            // A z-index still deferred at hit-test time never got an element
            // context; treat it as auto rather than guessing a stacking level.
            livery::values::ZIndex::Auto | livery::values::ZIndex::Deferred(_) => 0,
        };
        state.candidates.push(HitCandidate {
            id,
            level,
            order: state.order,
        });
    }
    state.order = state.order.saturating_add(1);

    let pushed_clip = style
        .zip(visible_fragment)
        .filter(|(style, _)| {
            style.overflow_x != CssOverflow::Visible || style.overflow_y != CssOverflow::Visible
        })
        .map(|(_, fragment)| {
            (
                fragment.x,
                fragment.y,
                fragment.x + fragment.width,
                fragment.y + fragment.height,
            )
        });
    if let Some(clip) = pushed_clip.as_ref() {
        state.clips.push(*clip);
    }
    let children = state.dom.dom_children(id).collect::<Vec<_>>();
    let next_scroll = state
        .scroll_offsets
        .get(&id)
        .copied()
        .map_or(ancestor_scroll, |offset| {
            (ancestor_scroll.0 + offset.0, ancestor_scroll.1 + offset.1)
        });
    for child in children {
        collect_hit_candidates(state, child, next_scroll);
    }
    if pushed_clip.is_some() {
        state.clips.pop();
    }
}

fn is_replaced_element<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.kind(id) == NodeKind::Element
        && dom.element_name(id).is_some_and(|name| {
            name.local.as_ref().eq_ignore_ascii_case("img")
                || name.local.as_ref().eq_ignore_ascii_case("canvas")
        })
}

fn apply_replaced_image_size<D>(
    style: &mut Style,
    dom: &D,
    id: D::NodeId,
    computed: &ComputedValues,
    image_sources: &ImageSources,
    font_size: f32,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let intrinsic = image_intrinsic_size(dom, id, image_sources)
        .filter(|(width, height)| *width > 0.0 && *height > 0.0);

    // HTML width/height attributes are presentational hints. A CSS value wins
    // even when it is percentage-based; only an auto CSS dimension accepts
    // the attribute. Legacy percentage attributes remain percentages so they
    // resolve against the eventual containing block rather than against zero.
    let width_hint = matches!(computed.width, CssSize::Auto)
        .then(|| image_attribute_size(dom, id, "width"))
        .flatten();
    let height_hint = matches!(computed.height, CssSize::Auto)
        .then(|| image_attribute_size(dom, id, "height"))
        .flatten();
    if let Some(width) = width_hint {
        style.size.width = width.dimension();
    }
    if let Some(height) = height_hint {
        style.size.height = height.dimension();
    }
    let width_specified = !matches!(computed.width, CssSize::Auto) || width_hint.is_some();
    let height_specified = !matches!(computed.height, CssSize::Auto) || height_hint.is_some();
    let width =
        definite_size(computed.width, font_size).or_else(|| width_hint.and_then(|hint| hint.px()));
    let height = definite_size(computed.height, font_size)
        .or_else(|| height_hint.and_then(|hint| hint.px()));
    if let Some((intrinsic_width, intrinsic_height)) = intrinsic
        && style.aspect_ratio.is_none()
        && !(width.is_some() && height.is_some())
    {
        style.aspect_ratio = Some(intrinsic_width / intrinsic_height);
    }
    match (width, height, width_specified, height_specified, intrinsic) {
        (Some(width), _, true, false, Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(width);
            style.size.height = Dimension::length(width * intrinsic_height / intrinsic_width);
        },
        (_, Some(height), false, true, Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(height * intrinsic_width / intrinsic_height);
            style.size.height = Dimension::length(height);
        },
        (None, None, false, false, Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(intrinsic_width);
            style.size.height = Dimension::length(intrinsic_height);
        },
        _ => {},
    }
}

#[derive(Clone, Copy)]
enum ImageAttributeSize {
    Length(f32),
    Percentage(f32),
}

impl ImageAttributeSize {
    fn dimension(self) -> Dimension {
        match self {
            Self::Length(value) => Dimension::length(value),
            Self::Percentage(value) => Dimension::percent(value),
        }
    }

    fn px(self) -> Option<f32> {
        match self {
            Self::Length(value) => Some(value),
            Self::Percentage(_) => None,
        }
    }
}

fn image_attribute_size<D>(dom: &D, id: D::NodeId, name: &str) -> Option<ImageAttributeSize>
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.attributes(id).find_map(|attribute| {
        (attribute.name.ns.as_ref().is_empty()
            && attribute.name.local.as_ref().eq_ignore_ascii_case(name))
        .then(|| {
            let value = attribute.value.trim();
            if let Some(percentage) = value.strip_suffix('%') {
                percentage
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|value| ImageAttributeSize::Percentage(value / 100.0))
            } else {
                value.parse::<f32>().ok().map(ImageAttributeSize::Length)
            }
        })
        .flatten()
        .filter(|value| match value {
            ImageAttributeSize::Length(value) | ImageAttributeSize::Percentage(value) => {
                value.is_finite() && *value > 0.0
            },
        })
    })
}

fn image_intrinsic_size<D>(
    dom: &D,
    id: D::NodeId,
    image_sources: &ImageSources,
) -> Option<(f32, f32)>
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
    let source = dom.attributes(id).find_map(|attribute| {
        (attribute.name.ns.as_ref().is_empty()
            && attribute.name.local.as_ref().eq_ignore_ascii_case("src"))
        .then_some(attribute.value)
    })?;
    let bytes = if let Ok(data_url) = data_url::DataUrl::process(source) {
        data_url.decode_to_vec().ok()?.0
    } else {
        image_sources.get(source)?.clone()
    };
    let image = image::load_from_memory(&bytes).ok()?;
    Some((image.width() as f32, image.height() as f32))
}

fn definite_size(size: CssSize, font_size: f32) -> Option<f32> {
    let CssSize::Value(value) = size else {
        return None;
    };
    match value {
        CssLengthPercentage::Length(length) => Some(absolute_length(length, font_size, 16.0)),
        CssLengthPercentage::Calc(calc) if calc.percentage == 0.0 => {
            Some(calc.px + calc.em * font_size + calc.rem * 16.0)
        },
        _ => None,
    }
}

fn collapsed_word_width(text: &str) -> usize {
    let mut maximum = 0;
    let mut current = 0;
    for character in text.chars() {
        if matches!(
            character,
            '\u{0009}' | '\u{000A}' | '\u{000C}' | '\u{000D}' | ' '
        ) {
            maximum = maximum.max(current);
            current = 0;
        } else {
            current += 1;
        }
    }
    maximum.max(current)
}

fn collapsed_text_width(text: &str) -> usize {
    let mut width = 0;
    let mut pending_space = false;
    for character in text.chars() {
        if matches!(
            character,
            '\u{0009}' | '\u{000A}' | '\u{000C}' | '\u{000D}' | ' '
        ) {
            pending_space = width != 0;
        } else {
            if pending_space {
                width += 1;
                pending_space = false;
            }
            width += 1;
        }
    }
    width
}

fn to_taffy_style(computed: &ComputedValues, font_size: f32) -> Style {
    let table = computed.display == CssDisplay::Table;
    let table_row = computed.display == CssDisplay::TableRow;
    let _ = table_row;
    let display = match computed.display {
        CssDisplay::None => Display::None,
        CssDisplay::Flex => Display::Flex,
        CssDisplay::Grid => Display::Grid,
        // A table box is laid out as a grid whose children are its
        // flattened cells; see `table_cells`.
        CssDisplay::Table => Display::Grid,
        CssDisplay::TableRow => Display::Flex,
        _ => Display::Block,
    };
    let flex_direction = if table_row {
        FlexDirection::Row
    } else {
        match computed.flex_direction {
            CssFlexDirection::Row => FlexDirection::Row,
            CssFlexDirection::RowReverse => FlexDirection::RowReverse,
            CssFlexDirection::Column => FlexDirection::Column,
            CssFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
        }
    };
    let float = match computed.float {
        CssFloat::None if table => TaffyFloat::Left,
        CssFloat::None => TaffyFloat::None,
        CssFloat::Left => TaffyFloat::Left,
        CssFloat::Right => TaffyFloat::Right,
    };
    Style {
        display,
        float,
        box_sizing: match computed.box_sizing {
            CssBoxSizing::ContentBox => BoxSizing::ContentBox,
            CssBoxSizing::BorderBox => BoxSizing::BorderBox,
        },
        overflow: Point {
            x: overflow(computed.overflow_x),
            y: overflow(computed.overflow_y),
        },
        position: match computed.position {
            CssPosition::Absolute | CssPosition::Fixed => Position::Absolute,
            _ => Position::Relative,
        },
        inset: if matches!(computed.position, CssPosition::Static) {
            Rect::auto()
        } else {
            Rect {
                left: inset(computed.left, font_size),
                right: inset(computed.right, font_size),
                top: inset(computed.top, font_size),
                bottom: inset(computed.bottom, font_size),
            }
        },
        size: Size {
            width: dimension(computed.width, font_size),
            height: dimension(computed.height, font_size),
        },
        min_size: Size {
            width: dimension(computed.min_width, font_size),
            height: dimension(computed.min_height, font_size),
        },
        max_size: Size {
            width: dimension(computed.max_width, font_size),
            height: dimension(computed.max_height, font_size),
        },
        aspect_ratio: match computed.aspect_ratio {
            AspectRatio::Auto => None,
            AspectRatio::Ratio(value) => Some(value),
        },
        size_containment: match computed.container_type {
            ContainerType::Normal => Size {
                width: false,
                height: false,
            },
            ContainerType::InlineSize if computed.writing_mode.is_vertical() => Size {
                width: false,
                height: true,
            },
            ContainerType::InlineSize => Size {
                width: true,
                height: false,
            },
            ContainerType::Size => Size {
                width: true,
                height: true,
            },
        },
        flex_direction,
        flex_wrap: match computed.flex_wrap {
            CssFlexWrap::NoWrap => FlexWrap::NoWrap,
            CssFlexWrap::Wrap => FlexWrap::Wrap,
            CssFlexWrap::WrapReverse => FlexWrap::WrapReverse,
        },
        flex_basis: dimension(computed.flex_basis, font_size),
        flex_grow: computed.flex_grow.value(),
        flex_shrink: computed.flex_shrink.value(),
        order: computed.order.value(),
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
        gap: Size {
            width: gap(computed.column_gap, font_size),
            height: gap(computed.row_gap, font_size),
        },
        align_items: Some(align_items(computed.align_items)),
        // `auto` on the self properties defers to the parent's items value,
        // which is taffy's `None`. A content-keyword size in that axis
        // additionally suppresses stretch (see `suppresses_stretch`).
        align_self: self_alignment(computed.align_self, computed.height),
        justify_items: Some(align_items(computed.justify_items)),
        justify_self: self_alignment(computed.justify_self, computed.width),
        align_content: Some(align_content(computed.align_content)),
        justify_content: Some(justify_content(computed.justify_content)),
        grid_template_columns: grid_template(&computed.grid_template_columns, font_size),
        grid_template_rows: grid_template(&computed.grid_template_rows, font_size),
        grid_auto_flow: grid_auto_flow(computed.grid_auto_flow),
        grid_column: Line {
            start: grid_placement(computed.grid_column_start),
            end: grid_placement(computed.grid_column_end),
        },
        grid_row: Line {
            start: grid_placement(computed.grid_row_start),
            end: grid_placement(computed.grid_row_end),
        },
        ..Style::default()
    }
}

fn grid_auto_flow(value: CssGridAutoFlow) -> GridAutoFlow {
    match value {
        CssGridAutoFlow::Row => GridAutoFlow::Row,
        CssGridAutoFlow::Column => GridAutoFlow::Column,
        CssGridAutoFlow::RowDense => GridAutoFlow::RowDense,
        CssGridAutoFlow::ColumnDense => GridAutoFlow::ColumnDense,
    }
}

fn grid_placement(value: CssGridPlacement) -> GridPlacement {
    match value {
        CssGridPlacement::Auto => GridPlacement::Auto,
        CssGridPlacement::Line(value) => line(value),
        CssGridPlacement::Span(value) => span(value),
    }
}

fn grid_template(value: &CssGridTemplate, em: f32) -> Vec<GridTemplateComponent<String>> {
    match value {
        CssGridTemplate::None => Vec::new(),
        CssGridTemplate::Tracks(tracks) => tracks
            .iter()
            .map(|track| match track {
                CssGridTrack::Auto => auto(),
                CssGridTrack::MinContent => min_content(),
                CssGridTrack::MaxContent => max_content(),
                CssGridTrack::Length(value) => length(value.unit.to_px(value.value, em, 16.0)),
                CssGridTrack::Percent(value) => percent(*value),
                CssGridTrack::Fr(value) => fr(*value),
            })
            .collect(),
    }
}

/// The taffy self-alignment for one axis.
///
/// `auto` normally defers to the parent's items value, which taffy spells
/// `None`. The exception is a size that suppresses stretch: css-align applies
/// `stretch` only when the item's size in that axis computes to `auto`, and
/// Livery maps the content keywords onto `Dimension::auto()` because taffy's
/// safe `Dimension` constructors cannot express them. Without this the item
/// would inherit the container's `stretch` and fill its grid area instead of
/// taking its content size. Resolving to `Start` here is the fallback
/// alignment stretch degrades to.
fn self_alignment(value: CssAlignment, size: CssSize) -> Option<AlignItems> {
    match value {
        CssAlignment::Auto if suppresses_stretch(size) => Some(align_items(CssAlignment::Start)),
        CssAlignment::Auto => None,
        value => Some(align_items(value)),
    }
}

/// Whether a size is not `auto` but reaches taffy as `auto`.
///
/// An explicit length or percentage already defeats stretch on its own, since
/// the definite size wins. Only the content keywords need saying out loud.
fn suppresses_stretch(size: CssSize) -> bool {
    matches!(
        size,
        CssSize::MinContent | CssSize::MaxContent | CssSize::FitContent(_)
    )
}

fn align_items(value: CssAlignment) -> AlignItems {
    AlignItems {
        keyword: match value {
            CssAlignment::Start => AlignItemsKeyword::Start,
            CssAlignment::End => AlignItemsKeyword::End,
            CssAlignment::FlexStart => AlignItemsKeyword::FlexStart,
            CssAlignment::FlexEnd => AlignItemsKeyword::FlexEnd,
            CssAlignment::Center => AlignItemsKeyword::Center,
            CssAlignment::Baseline => AlignItemsKeyword::Baseline,
            _ => AlignItemsKeyword::Stretch,
        },
        safety: taffy::style::AlignmentSafety::Unsafe,
    }
}

fn align_content(value: CssAlignment) -> AlignContent {
    AlignContent {
        keyword: match value {
            CssAlignment::Start => AlignContentKeyword::Start,
            CssAlignment::End => AlignContentKeyword::End,
            CssAlignment::FlexStart => AlignContentKeyword::FlexStart,
            CssAlignment::FlexEnd => AlignContentKeyword::FlexEnd,
            CssAlignment::Center => AlignContentKeyword::Center,
            CssAlignment::SpaceBetween => AlignContentKeyword::SpaceBetween,
            CssAlignment::SpaceAround => AlignContentKeyword::SpaceAround,
            CssAlignment::SpaceEvenly => AlignContentKeyword::SpaceEvenly,
            _ => AlignContentKeyword::Stretch,
        },
        safety: taffy::style::AlignmentSafety::Unsafe,
    }
}

fn justify_content(value: CssAlignment) -> JustifyContent {
    align_content(value)
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

fn dimension_with_basis(size: CssSize, em: f32, basis: Option<f32>) -> Dimension {
    match (size, basis) {
        (CssSize::Value(CssLengthPercentage::Calc(calc)), Some(basis))
            if calc.percentage != 0.0 =>
        {
            Dimension::length(absolute_length_percentage(
                CssLengthPercentage::Calc(calc),
                em,
                16.0,
                basis,
            ))
        },
        (size, _) => dimension(size, em),
    }
}

fn resolved_child_containing_size(
    computed: &ComputedValues,
    em: f32,
    containing_size: (Option<f32>, Option<f32>),
) -> (Option<f32>, Option<f32>) {
    let fills_available_width = !matches!(
        computed.display,
        CssDisplay::None | CssDisplay::Inline | CssDisplay::InlineBlock
    );
    (
        resolved_explicit_size(computed.width, em, containing_size.0).or(
            if fills_available_width {
                containing_size.0
            } else {
                None
            },
        ),
        resolved_explicit_size(computed.height, em, containing_size.1),
    )
}

fn resolved_explicit_size(size: CssSize, em: f32, basis: Option<f32>) -> Option<f32> {
    let CssSize::Value(value) = size else {
        return None;
    };
    if value.has_percentage() {
        basis.map(|basis| absolute_length_percentage(value, em, 16.0, basis))
    } else {
        Some(absolute_length_percentage(value, em, 16.0, 0.0))
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

fn gap(value: CssGap, em: f32) -> LengthPercentage {
    length_percentage(value.0, em)
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
        CssLengthPercentage::Math(math) => {
            CssLengthPercentage::Math(math).to_px(em, rem, percentage_basis)
        },
    }
}

pub(crate) fn length_percentage_px(
    value: CssLengthPercentage,
    em: f32,
    percentage_basis: f32,
) -> f32 {
    absolute_length_percentage(value, em, 16.0, percentage_basis).max(0.0)
}

pub(crate) fn signed_length_percentage_px(
    value: CssLengthPercentage,
    em: f32,
    percentage_basis: f32,
) -> f32 {
    absolute_length_percentage(value, em, 16.0, percentage_basis)
}

fn absolute_length(length: Length, em: f32, rem: f32) -> f32 {
    length.unit.to_px(length.value, em, rem)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Device, InteractionStates, StyleSet, emit_paint_list_with_text_system, resolve_styles,
    };
    use genet_static_dom::StaticDocument;
    use paint_list_api::DeviceIntSize;

    #[test]
    fn retained_inline_format_is_not_shaped_again_for_paint() {
        let dom = StaticDocument::parse(
            "<html><body><div class=\"label\"><span id=\"split\">one two three four</span></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[".label { width: 80px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (styles, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let after_layout = text.shape_count();
        let split = {
            fn find(
                dom: &StaticDocument,
                node: <StaticDocument as LayoutDom>::NodeId,
            ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
                if dom
                    .element_name(node)
                    .is_some_and(|name| name.local.as_ref() == "span")
                {
                    return Some(node);
                }
                dom.dom_children(node).find_map(|child| find(dom, child))
            }
            find(&dom, dom.document()).expect("split span")
        };

        assert!(after_layout > 0);
        assert!(
            layout.fragments_for_node(split).count() >= 2,
            "one inline box must own one fragment per wrapped line"
        );
        let _ = emit_paint_list_with_text_system(
            &dom,
            &styles,
            &layout,
            DeviceIntSize::new(320, 240),
            1,
            &mut text,
        );
        assert_eq!(
            text.shape_count(),
            after_layout,
            "paint must consume the retained inline result"
        );
    }

    #[test]
    fn split_inline_continuations_format_their_own_box_children() {
        let dom = StaticDocument::parse(
            "<html><body><div class=\"host\"><span>before<div class=\"block\">block</div>after</span></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                ".host { width: 120px; } .block { display: block; height: 20px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let split = {
            fn find(
                dom: &StaticDocument,
                node: <StaticDocument as LayoutDom>::NodeId,
            ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
                if dom
                    .element_name(node)
                    .is_some_and(|name| name.local.as_ref() == "span")
                {
                    return Some(node);
                }
                dom.dom_children(node).find_map(|child| find(dom, child))
            }
            find(&dom, dom.document()).expect("split span")
        };
        let boxes = layout.boxes().boxes_for_node(split);
        let first = layout
            .fragments()
            .fragments_for_box(boxes[0])
            .next()
            .expect("first continuation")
            .physical_rect();
        let second = layout
            .fragments()
            .fragments_for_box(boxes[1])
            .next()
            .expect("second continuation")
            .physical_rect();

        assert_eq!(boxes.len(), 2);
        assert!(
            second.y > first.y,
            "the block between continuation boxes must advance block flow"
        );
    }

    #[test]
    fn partial_inline_groups_do_not_share_one_box_intrinsic_cache_entry() {
        fn find(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom
                .element_name(node)
                .is_some_and(|name| name.local.as_ref() == "div")
            {
                return Some(node);
            }
            dom.dom_children(node).find_map(|child| find(dom, child))
        }

        let dom = StaticDocument::parse(
            "<html><body><div>before<span class=\"out\">out</span>after</div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[".out { position: absolute; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let host = find(&dom, dom.document()).expect("host");
        let host_box = boxes.principal_box(host).expect("host box");

        assert_eq!(
            intrinsic_owner_for_flow_children(&boxes, host_box, boxes[host_box].children()),
            None,
            "two partial inline groups must not alias the parent box query"
        );
    }

    #[test]
    fn ordinary_live_block_flow_uses_buckram_without_backend_dispatch() {
        fn collect_divs(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            output: &mut Vec<<StaticDocument as LayoutDom>::NodeId>,
        ) {
            if dom
                .element_name(node)
                .is_some_and(|name| name.local.as_ref() == "div")
            {
                output.push(node);
            }
            for child in dom.dom_children(node) {
                collect_divs(dom, child, output);
            }
        }

        let dom = StaticDocument::parse(
            "<html><body><div class=\"host\"><div></div><div></div></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div { margin: 0; padding: 0; border: 0; } .host > div { height: 20px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let mut divs = Vec::new();
        collect_divs(&dom, dom.document(), &mut divs);
        let first = layout.get(divs[1]).expect("first child").physical_rect();
        let second = layout.get(divs[2]).expect("second child").physical_rect();
        let algorithms = layout.block_algorithm_counts();

        assert!(
            algorithms.buckram >= 4,
            "the root, html, body, and host block contexts should use Buckram"
        );
        assert_eq!(algorithms.taffy, 0);
        assert_eq!(second.y, first.y + 20.0);
    }

    #[test]
    fn replaced_html_dimension_hints_keep_percentage_and_canvas_width() {
        fn find_by_name(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            name: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom
                .element_name(node)
                .is_some_and(|element| element.local.as_ref() == name)
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| find_by_name(dom, child, name))
        }

        let dom = StaticDocument::parse(
            "<html><body><div><img width=\"100%\" height=\"3\">\
             <canvas width=\"100\" height=\"100\"></canvas></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body { margin: 0; } div { position: relative; width: 200px; }\
                 img { position: absolute; left: 0; top: 0; } canvas { display: block; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");

        let image = find_by_name(&dom, dom.document(), "img").expect("img");
        let image = layout.get(image).expect("image fragment").physical_rect();
        assert_eq!(
            (image.width, image.height),
            (200.0, 3.0),
            "the percentage hint resolves against the positioned containing block"
        );

        let canvas = find_by_name(&dom, dom.document(), "canvas").expect("canvas");
        let canvas = layout.get(canvas).expect("canvas fragment").physical_rect();
        assert_eq!((canvas.width, canvas.height), (100.0, 100.0));
    }

    #[test]
    fn percentage_height_chain_uses_initial_containing_block_height() {
        fn find_by_name(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            name: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom
                .element_name(node)
                .is_some_and(|element| element.local.as_ref() == name)
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| find_by_name(dom, child, name))
        }

        let dom = StaticDocument::parse("<html><body><p>viewport</p></body></html>");
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, p { height: 100%; margin: 0; padding: 0; border: 0; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");

        for name in ["html", "body", "p"] {
            let node = find_by_name(&dom, dom.document(), name).expect(name);
            assert_eq!(
                layout.get(node).expect(name).physical_rect().height,
                240.0,
                "{name} should resolve 100% against a definite containing block"
            );
        }
        assert_eq!(layout.block_algorithm_counts().taffy, 0);
    }

    #[test]
    fn live_block_flow_keeps_collapsed_margin_chains_in_buckram() {
        fn collect_divs(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            output: &mut Vec<<StaticDocument as LayoutDom>::NodeId>,
        ) {
            if dom
                .element_name(node)
                .is_some_and(|name| name.local.as_ref() == "div")
            {
                output.push(node);
            }
            for child in dom.dom_children(node) {
                collect_divs(dom, child, output);
            }
        }

        let dom = StaticDocument::parse(
            "<html><body><div class=\"host\">\
             <div class=\"parent\"><div class=\"child\"></div></div>\
             <div class=\"after\"></div>\
             <div class=\"chain\"><div class=\"first\"></div><div class=\"empty\"></div>\
             <div class=\"last\"></div></div>\
             </div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, .host, .chain { margin: 0; padding: 0; border: 0; }\
                 .parent { margin: 10px 0 15px; }\
                 .child { height: 20px; margin: 30px 0 40px; }\
                 .after { height: 10px; margin: 12px 0 0; }\
                 .first { height: 10px; margin: 0 0 20px; }\
                 .empty { margin: -7px 0 12px; }\
                 .last { height: 10px; margin: -15px 0 0; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let mut divs = Vec::new();
        collect_divs(&dom, dom.document(), &mut divs);
        let parent = layout.get(divs[1]).expect("parent").physical_rect();
        let child = layout.get(divs[2]).expect("child").physical_rect();
        let after = layout.get(divs[3]).expect("after").physical_rect();
        let first = layout.get(divs[5]).expect("first").physical_rect();
        let empty = layout.get(divs[6]).expect("empty").physical_rect();
        let last = layout.get(divs[7]).expect("last").physical_rect();
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(child.y, parent.y);
        assert_eq!(after.y, parent.y + 60.0);
        assert_eq!(empty.y, first.y + 23.0);
        assert_eq!(last.y, first.y + 15.0);
        assert!(algorithms.buckram >= 6);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_bfc_places_blockified_floats_and_direct_clearance_in_buckram() {
        fn by_class(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "class"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_class(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body><div class=\"host\">\
             <span class=\"left\"></span><div class=\"right\"></div>\
             <div class=\"clear\"></div></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 .host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 .left { float: left; width: 80px; height: 40px; }\
                 .right { float: right; width: 60px; height: 70px; }\
                 .clear { clear: both; height: 10px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |class| {
            let node = by_class(&dom, dom.document(), class).expect(class);
            layout.get(node).expect(class).physical_rect()
        };

        let host = rect("host");
        let left = rect("left");
        let right = rect("right");
        let clear = rect("clear");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!((left.x, left.y), (host.x, host.y));
        assert_eq!((right.x, right.y), (host.x + 140.0, host.y));
        assert_eq!((clear.x, clear.y), (host.x, host.y + 70.0));
        assert_eq!(host.height, 80.0);
        assert!(algorithms.buckram >= 4);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_empty_clearance_keeps_its_following_margin_chain_in_buckram() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body><div id=\"host\">\
             <div id=\"float\"></div><div id=\"empty\"></div><div id=\"after\"></div>\
             </div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 #host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 #float { float: left; width: 80px; height: 40px; }\
                 #empty { clear: left; margin-top: 10px; margin-bottom: 20px; }\
                 #after { height: 10px; margin-top: 30px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let host = rect("host");
        let float = rect("float");
        let empty = rect("empty");
        let after = rect("after");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!((float.x - host.x, float.y - host.y), (0.0, 0.0));
        assert_eq!(
            (empty.y - host.y, empty.height),
            (40.0, 0.0),
            "host={host:?}, float={float:?}, empty={empty:?}, after={after:?}, algorithms={algorithms:?}"
        );
        assert_eq!((after.y - host.y, after.height), (70.0, 10.0));
        assert_eq!(host.height, 80.0);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_inline_lines_in_an_ordinary_wrapper_share_outer_float_exclusions() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body><div id=\"host\"><div id=\"float\"></div>\
             <div id=\"wrapper\"><span id=\"copy\">aa aa aa aa aa aa aa aa aa aa aa aa \
             aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa aa \
             aa aa</span></div>\
             </div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 #host { width: 200px; overflow-x: hidden; overflow-y: hidden;\
                         font-family: monospace; font-size: 10px; line-height: 20px; }\
                 #float { float: left; width: 80px; height: 40px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let host = by_id(&dom, dom.document(), "host").expect("host");
        let copy = by_id(&dom, dom.document(), "copy").expect("copy");
        let host = layout.get(host).expect("host fragment").physical_rect();
        let algorithms = layout.block_algorithm_counts();
        let mut lines = layout
            .fragments_for_node(copy)
            .map(|fragment| fragment.physical_rect())
            .collect::<Vec<_>>();
        lines.sort_by(|left, right| left.y.total_cmp(&right.y));

        assert!(
            lines.len() >= 4,
            "fixture must produce several line fragments"
        );
        assert!(
            (lines[0].x - (host.x + 80.0)).abs() <= 0.5,
            "host={host:?}, lines={lines:?}, algorithms={algorithms:?}"
        );
        assert!(
            (lines[1].x - (host.x + 80.0)).abs() <= 0.5,
            "host={host:?}, lines={lines:?}, algorithms={algorithms:?}"
        );
        assert!(
            lines
                .iter()
                .filter(|line| line.y >= host.y + 40.0)
                .all(|line| (line.x - host.x).abs() <= 0.5),
            "lines below the float must use the full content column"
        );
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_nested_float_state_crosses_ordinary_wrappers_but_stops_at_bfcs() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body>\
             <div id=\"shared\" class=\"host\"><div id=\"wrapper\"><div class=\"float\"></div></div>\
             <div id=\"shared-clear\" class=\"clear\"></div></div>\
             <div id=\"isolated\" class=\"host\"><div id=\"boundary\"><div class=\"float\"></div></div>\
             <div id=\"isolated-clear\" class=\"clear\"></div></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 .host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 .float { float: left; width: 80px; height: 40px; }\
                 .clear { clear: left; height: 10px; }\
                 #boundary { display: flow-root; height: 0; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let shared = rect("shared");
        let wrapper = rect("wrapper");
        let shared_clear = rect("shared-clear");
        let isolated = rect("isolated");
        let boundary = rect("boundary");
        let isolated_clear = rect("isolated-clear");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(wrapper.height, 0.0);
        assert_eq!(shared_clear.y - shared.y, 40.0);
        assert_eq!(shared.height, 50.0);
        assert_eq!(boundary.height, 0.0);
        assert_eq!(isolated_clear.y, isolated.y);
        assert_eq!(isolated.height, 10.0);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_block_bfcs_narrow_beside_a_float_or_move_below_it() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body><div id=\"host\"><div id=\"float\"></div>\
             <div id=\"adjacent\"></div><div id=\"lowered\"></div>\
             </div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 #host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 #float { float: left; width: 80px; height: 40px; }\
                 #adjacent { height: 20px; overflow-x: hidden; overflow-y: hidden; }\
                 #lowered { width: 150px; height: 20px;\
                            overflow-x: hidden; overflow-y: hidden; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let host = rect("host");
        let adjacent = rect("adjacent");
        let lowered = rect("lowered");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(
            (adjacent.x, adjacent.y, adjacent.width, adjacent.height),
            (host.x + 80.0, host.y, 120.0, 20.0)
        );
        assert_eq!(
            (lowered.x, lowered.y, lowered.width, lowered.height),
            (host.x, host.y + 40.0, 150.0, 20.0)
        );
        assert_eq!(host.height, 60.0);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_bfc_inline_margins_can_force_float_avoidance_in_both_directions() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body>\
             <div id=\"ltr\" class=\"host\"><div class=\"right-float\"></div>\
             <div id=\"ltr-bfc\" class=\"bfc\"></div></div>\
             <div id=\"rtl\" class=\"host\"><div class=\"left-float\"></div>\
             <div id=\"rtl-bfc\" class=\"bfc\"></div></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 .host { width: 100px; overflow-x: hidden; overflow-y: hidden; }\
                 .right-float { float: right; width: 50px; height: 40px; }\
                 .left-float { float: left; width: 50px; height: 40px; }\
                 .bfc { display: flow-root; height: 60px; }\
                 #ltr-bfc { margin-left: 51px; }\
                 #rtl { direction: rtl; } #rtl-bfc { margin-right: 51px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let ltr = rect("ltr");
        let ltr_bfc = rect("ltr-bfc");
        let rtl = rect("rtl");
        let rtl_bfc = rect("rtl-bfc");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(
            (
                ltr_bfc.x - ltr.x,
                ltr_bfc.y - ltr.y,
                ltr_bfc.width,
                ltr_bfc.height,
            ),
            (51.0, 40.0, 49.0, 60.0),
            "ltr={ltr:?}, ltr_bfc={ltr_bfc:?}, rtl={rtl:?}, rtl_bfc={rtl_bfc:?}, algorithms={algorithms:?}"
        );
        assert_eq!(
            (
                rtl_bfc.x - rtl.x,
                rtl_bfc.y - rtl.y,
                rtl_bfc.width,
                rtl_bfc.height,
            ),
            (0.0, 40.0, 49.0, 60.0)
        );
        assert_eq!((ltr.height, rtl.height), (100.0, 100.0));
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_flex_and_grid_bfcs_use_buckram_float_placement() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body><div id=\"host\"><div id=\"float\"></div>\
             <div id=\"flex\"><div id=\"flex-child\"></div></div>\
             <div id=\"grid\"><div id=\"grid-child\"></div></div>\
             </div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 #host { width: 100px; overflow-x: hidden; overflow-y: hidden; }\
                 #float { float: left; width: 40px; height: 40px; }\
                 #flex { display: flex; height: 20px; }\
                 #flex-child { width: 20px; height: 10px; }\
                 #grid { display: grid; grid-template-columns: 20px; width: 70px; height: 20px; }\
                 #grid-child { width: 20px; height: 10px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let host = rect("host");
        let flex = rect("flex");
        let flex_child = rect("flex-child");
        let grid = rect("grid");
        let grid_child = rect("grid-child");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(
            (
                flex.x - host.x,
                flex.y - host.y,
                flex.width,
                flex.height,
                flex_child.x - flex.x,
                flex_child.y - flex.y,
            ),
            (40.0, 0.0, 60.0, 20.0, 0.0, 0.0),
            "host={host:?}, flex={flex:?}, flex_child={flex_child:?}, grid={grid:?}, grid_child={grid_child:?}, algorithms={algorithms:?}"
        );
        assert_eq!(
            (
                grid.x - host.x,
                grid.y - host.y,
                grid.width,
                grid.height,
                grid_child.x - grid.x,
                grid_child.y - grid.y,
            ),
            (0.0, 40.0, 70.0, 20.0, 0.0, 0.0)
        );
        assert_eq!(host.height, 60.0);
        assert!(algorithms.buckram > 0);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_auto_float_width_clamps_retained_inline_intrinsics() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body>\
             <div id=\"narrow\" class=\"host\"><span id=\"narrow-float\" class=\"float\">\
             aaaa aaaa aaaa aaaa</span><div class=\"clear\"></div></div>\
             <div id=\"wide\" class=\"host\"><span id=\"wide-float\" class=\"float\">\
             aaaa aaaa aaaa aaaa</span><div class=\"clear\"></div></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 .host { overflow-x: hidden; overflow-y: hidden; }\
                 #narrow { width: 80px; } #wide { width: 200px; }\
                 .float { float: left; font-family: monospace; font-size: 10px;\
                          line-height: 20px; }\
                 .clear { clear: both; height: 1px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let narrow_host = rect("narrow");
        let narrow_float = rect("narrow-float");
        let wide_host = rect("wide");
        let wide_float = rect("wide-float");
        let algorithms = layout.block_algorithm_counts();

        assert!((narrow_float.width - narrow_host.width).abs() <= 0.5);
        assert!(
            wide_float.width > narrow_float.width + 10.0
                && wide_float.width < wide_host.width - 10.0,
            "narrow={narrow_float:?}, wide={wide_float:?}"
        );
        assert!(narrow_float.height > wide_float.height);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn live_multi_child_float_and_atomic_inline_use_intrinsic_subtrees() {
        fn by_id(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            expected: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.attributes(node).any(|attribute| {
                attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref() == "id"
                    && attribute.value == expected
            }) {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| by_id(dom, child, expected))
        }

        let dom = StaticDocument::parse(
            "<html><body>\
             <div id=\"narrow\" class=\"host\"><div id=\"narrow-float\" class=\"float\">\
             <div>aaaa aaaa aaaa aaaa</div><div>aaaa aaaa aaaa aaaa</div></div>\
             <div class=\"clear\"></div></div>\
             <div id=\"wide\" class=\"host\"><div id=\"wide-float\" class=\"float\">\
             <div>aaaa aaaa aaaa aaaa</div><div>aaaa aaaa aaaa aaaa</div></div>\
             <div class=\"clear\"></div></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 .host { overflow-x: hidden; overflow-y: hidden; }\
                 #narrow { width: 80px; } #wide { width: 200px; }\
                 .float { float: left; font-family: monospace; font-size: 10px;\
                          line-height: 20px; }\
                 .clear { clear: both; height: 1px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        let (_, layout) = layout_with_text_system(
            &dom,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout");
        let rect = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout.get(node).expect(id).physical_rect()
        };

        let narrow_host = rect("narrow");
        let narrow_float = rect("narrow-float");
        let wide_host = rect("wide");
        let wide_float = rect("wide-float");

        assert!((narrow_float.width - narrow_host.width).abs() <= 0.5);
        assert!(
            wide_float.width > narrow_float.width + 10.0
                && wide_float.width < wide_host.width - 10.0,
            "narrow={narrow_float:?}, wide={wide_float:?}"
        );
        assert!(narrow_float.height > wide_float.height);
        assert_eq!(layout.block_algorithm_counts().taffy, 0);

        fn atomic_inline_width(viewport_width: f32) -> f32 {
            let dom = StaticDocument::parse(
                "<html><body><span id=\"atomic\">aaaa aaaa aaaa aaaa</span></body></html>",
            );
            let styles = resolve_styles(
                &dom,
                &StyleSet::cambium(&["html, body, span { margin: 0; padding: 0; border: 0; }\
                     span { display: inline-block; font-family: monospace; font-size: 10px;\
                            line-height: 20px; }"]),
                &Device::screen(viewport_width, 240.0),
                &InteractionStates::default(),
            );
            let mut text = TextSystem::new();
            let (_, layout) = layout_with_text_system(
                &dom,
                &styles,
                viewport_width,
                240.0,
                ViewportSizes::uniform(viewport_width, 240.0),
                &mut text,
                &HashMap::new(),
            )
            .expect("atomic inline layout");
            let atomic = by_id(&dom, dom.document(), "atomic").expect("atomic node");
            layout
                .get(atomic)
                .expect("atomic fragment")
                .physical_rect()
                .width
        }

        assert_eq!(atomic_inline_width(30.0), 30.0);
        assert_eq!(atomic_inline_width(80.0), 80.0);
        assert_eq!(atomic_inline_width(200.0), 114.0);
    }
}
