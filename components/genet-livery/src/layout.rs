use std::{collections::HashMap, error::Error, fmt, hash::Hash};

use buckram::{
    AlgorithmAvailableSpace, AlgorithmKind, AlgorithmNodeId, AlgorithmSize, AlgorithmTree,
    Baselines, BlockBoxSizing, BlockDimensions, BlockPosition as BuckramBlockPosition,
    BlockSizeValue, BlockStyle, BoxId, BoxOrigin, ClearSide, CssBox, DisplayInside, DisplayOutside,
    FloatContextProvenance, FloatLineConstraints, FloatSide, FlowAxes, FlowLength, FlowLengthAuto,
    FormattingContextKind, Fragment as TreeFragment, FragmentDraftTree, FragmentId, FragmentTree,
    InternalTableRole, IntrinsicSizeCache, IntrinsicSizeKind, IntrinsicSizeQuery, IntrinsicSizes,
    LayoutResult, LogicalAxis, LogicalRect, PhysicalRect, PhysicalSides, PhysicalSize,
    PositioningScheme, TableCell, TableCellInput, TableCellLayoutInput, TableCellLayoutOutput,
    TableCellLayoutPass, TableFragmentRole, TableFragments, TableGrid, TableGridInputs,
    TableRowLayoutError, TableRowSpan, TableTrackInput,
};
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
use livery::{
    ComputedValues,
    media::{Device, ViewportSizes},
    stylesheet::ContainerSnapshot,
    values::{
        Alignment as CssAlignment, AspectRatio, BorderStyle, BorderWidth,
        BoxSizing as CssBoxSizing, CaptionSide, Clear as CssClear, ContainerType,
        Display as CssDisplay, FlexDirection as CssFlexDirection, FlexWrap as CssFlexWrap,
        Float as CssFloat, FontSize, Gap as CssGap, GridAutoFlow as CssGridAutoFlow,
        GridPlacement as CssGridPlacement, GridTemplate as CssGridTemplate,
        GridTrack as CssGridTrack, Inset, Length, LengthPercentage as CssLengthPercentage,
        LineHeight, Margin, Overflow as CssOverflow, Position as CssPosition,
        RelativeLengthEnvironment, Size as CssSize, VerticalAlign, WhiteSpaceCollapse,
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
    table_block::{
        CellBlockInput, CellFormatter, buckram_table_block, cell_content_block_size,
        commit_table_block, table_block_inputs, verify_table_block,
    },
    table_shadow::{
        LIVE_ROOT_FONT_SIZE, PendingTable, TableShadowLedger, buckram_table_columns,
        verify_assigned_columns,
    },
    table_wrapper::{grid_style, wrapper_style},
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
    // Accumulated K4c5a shadow ledgers from each atomic root's BuildState,
    // which are otherwise dropped exactly as table_bridge_count still is.
    table_shadow: TableShadowLedger,
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
    table_bridges: TableBridgeCounts,
    table_shadow: TableShadowLedger,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockAlgorithmCounts {
    pub buckram: usize,
    pub taffy: usize,
}

/// Live tables still routed through Livery's temporary Grid/Flex bridge.
/// K4c5 replaces this count with Buckram table dispatch and removes the
/// compatibility route.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TableBridgeCounts {
    pub grids: usize,
}

impl<Id> LiveryLayout<Id>
where
    Id: Copy + Eq + Hash,
{
    fn new(
        buckram: LayoutResult<Id>,
        text_frame: Option<TextFrame<Id>>,
        block_algorithms: BlockAlgorithmCounts,
        table_bridges: TableBridgeCounts,
        table_shadow: TableShadowLedger,
    ) -> Self {
        Self {
            buckram,
            text_frame,
            block_algorithms,
            table_bridges,
            table_shadow,
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

    /// The node's principal box's fragment: a table element's grid box, which
    /// owns background, borders, and used `width`/`height` under CSS 2.1
    /// section 17.4. Rectangle queries and paint-effect anchors use
    /// [`Self::get`], whose first box is the outermost - the wrapper.
    pub fn principal_fragment(&self, node: Id) -> Option<&TreeFragment> {
        self.buckram.principal_fragment(node)
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

    pub fn table_bridge_counts(&self) -> TableBridgeCounts {
        self.table_bridges
    }

    /// K4c5a's shadow comparison of Buckram's fixed sizing against the live
    /// path. K4c5b may only make Buckram authoritative once this is silent.
    pub fn table_shadow_ledger(&self) -> &TableShadowLedger {
        &self.table_shadow
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
    table_bridge_count: usize,
    table_shadow: TableShadowLedger,
    pending_tables: Vec<PendingTable<D::NodeId>>,
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

/// Compare one table's Buckram result against its painted fragments, in both
/// axes.
///
/// Buckram's rectangles are relative to the table grid's own origin, so the
/// block-axis comparison subtracts the grid's painted position. Without that
/// the table's place in the page would be reported as a table-layout
/// disagreement on every cell.
fn verify_one_table<Id>(
    pending: &PendingTable<Id>,
    live_rect_of: &impl Fn(BoxId) -> Option<Fragment>,
    ledger: &mut TableShadowLedger,
) {
    if let Some(assigned) = pending.assigned.as_ref().map(|inline| &inline.column_sizes) {
        let live = pending
            .grid
            .columns
            .iter()
            .enumerate()
            .map(|(index, _)| {
                pending
                    .grid
                    .cells
                    .iter()
                    .find(|cell| cell.column == index && cell.column_span == 1)
                    .and_then(|cell| live_rect_of(cell.source))
                    .map(|rect| rect.width)
            })
            .collect::<Vec<_>>();
        verify_assigned_columns(pending.table, assigned, &live, ledger);
    }
    let (Some(block), Some(grid)) = (pending.block.as_ref(), live_rect_of(pending.table)) else {
        return;
    };
    verify_table_block(
        pending.table,
        block,
        |box_id| live_rect_of(box_id).map(|rect| (rect.y - grid.y, rect.height)),
        &mut ledger.block,
    );
}

/// Format one table cell for Buckram's block pipeline.
///
/// The cell is laid out at exactly the content inline size K4c assigned, with
/// its own inline and block constraints neutralized: Buckram applies those
/// itself, and a floor applied here would come back as measured content. The
/// cell's specified block size reaches Buckram as a row constraint, never as
/// a taller content box, which is why the measurement pass leaves the height
/// automatic.
fn format_table_cell<Context, Source>(
    tree: &mut AlgorithmTree<Style, Context, Source>,
    node: AlgorithmNodeId,
    request: TableCellLayoutInput,
    cell: &CellBlockInput,
    mut measure: impl FnMut(&mut Context, InlineMeasureGeometry<'_>) -> (f32, f32),
) -> TableCellLayoutOutput {
    let offsets = cell.style.offsets;
    let style = tree.style_mut(node);
    let saved = (style.size, style.min_size, style.max_size, style.box_sizing);
    style.box_sizing = BoxSizing::ContentBox;
    style.size.width = Dimension::length(request.content_inline_size);
    style.min_size.width = Dimension::auto();
    style.max_size.width = Dimension::auto();
    style.min_size.height = Dimension::auto();
    style.max_size.height = Dimension::auto();
    style.size.height = match request.pass {
        TableCellLayoutPass::Measure => Dimension::auto(),
        // The percentage pass supplies a used border-box block size; the
        // cell's content box is that minus the offsets Buckram already knows.
        TableCellLayoutPass::ResolvePercentages { cell_block_size } => {
            Dimension::length(cell_content_block_size(cell_block_size, offsets))
        },
    };
    tree.compute_layout_with_measure(
        node,
        AlgorithmSize::new(
            AlgorithmAvailableSpace::Definite(request.content_inline_size),
            AlgorithmAvailableSpace::MaxContent,
        ),
        |known, available, _, context, _| {
            let Some(context) = context else {
                return AlgorithmSize::new(0.0, 0.0);
            };
            let width = match available.width {
                AlgorithmAvailableSpace::Definite(width) => width,
                AlgorithmAvailableSpace::MinContent => 0.01,
                AlgorithmAvailableSpace::MaxContent => f32::INFINITY,
            };
            let (measured_width, measured_height) = measure(
                context,
                InlineMeasureGeometry {
                    width: known.width.unwrap_or(width),
                    line_constraints: None,
                },
            );
            AlgorithmSize::new(
                known.width.unwrap_or(measured_width),
                known.height.unwrap_or(measured_height),
            )
        },
    );
    let border_box = tree.layout(node).height;
    let baselines = tree.baselines(node);
    let style = tree.style_mut(node);
    (style.size, style.min_size, style.max_size, style.box_sizing) = saved;
    TableCellLayoutOutput {
        content_block_size: cell_content_block_size(border_box, offsets),
        // CSS 2.1 section 10.7 leaves min-height and max-height undefined on
        // a table cell, and the K4d4c matrix measured both engines ignoring
        // them outright, so a cell carries no border-box floor of its own.
        border_box_min_block_size: 0.0,
        // Live descendants stay in the backend tree, which fragment
        // collection already walks. K4d6a's drafts exist for adapters with no
        // such tree, and a zero rectangle unions into the cell's own without
        // changing it.
        baselines,
        overflow: LogicalRect::default(),
        fragments: FragmentDraftTree::default(),
    }
}

/// Feed retained IFC line baselines into Buckram before fragment collection.
/// Parent block, flex, and grid contexts consume these declared outputs
/// through `AlgorithmTree::propagate_declared_baselines`; no post-layout
/// traversal of backend children is involved.
fn populate_inline_baselines(tree: &mut AlgorithmTree<Style, InlineMeasure, Vec<BoxId>>) {
    let direct = tree
        .node_ids()
        .filter_map(|node| {
            let width = tree.layout(node).width;
            tree.context(node)
                .and_then(|context| context.layout_for_width(width))
                .and_then(|layout| layout.baselines())
                .and_then(|(first, last)| Baselines::new(Some(first), Some(last)))
                .map(|baselines| (node, baselines))
        })
        .collect::<Vec<_>>();
    for (node, baselines) in direct {
        tree.set_baselines(node, baselines);
    }
    tree.propagate_declared_baselines();
}

struct InlineBuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    boxes: &'a GeneratedBoxTree<D::NodeId>,
    atomic: &'a AtomicLayoutPlane,
    tree: AlgorithmTree<Style, InlineMeasure, Vec<BoxId>>,
    image_sources: &'a ImageSources,
    table_bridge_count: usize,
    table_shadow: TableShadowLedger,
    pending_tables: Vec<PendingTable<D::NodeId>>,
    /// The grid and cell nodes for the table `build_children` just processed,
    /// consumed by `build_box` when it creates the table's algorithm node.
    pending_table_handoff: Option<(TableGrid, Vec<Option<AlgorithmNodeId>>)>,
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
    // K4e4: used `width` and `height` are properties of the principal box.
    // For a table element that is the grid, whose border box excludes the
    // captions the wrapper contains.
    let Some(fragment) = fragments.principal_fragment(node) else {
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
        table_bridge_count: 0,
        table_shadow: TableShadowLedger::default(),
        pending_tables: Vec::new(),
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

    state.apply_buckram_table_layout();
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
    let table_bridges = TableBridgeCounts {
        grids: state.table_bridge_count,
    };

    let tables = state.table_fragment_plane();
    let mut fragments = FragmentTree::default();
    let mut output = FragmentOutput {
        fragments: &mut fragments,
    };
    collect_fragments(
        &state.tree,
        &boxes,
        root,
        FragmentCursor {
            origin: Point { x: 0.0, y: 0.0 },
            containing: Fragment {
                x: 0.0,
                y: 0.0,
                width: viewport_width,
                height: viewport_height,
            },
            parent: None,
        },
        &tables,
        &mut output,
    )?;
    state.verify_table_layout(|box_id| {
        fragments
            .fragments_for_box(box_id)
            .next()
            .map(|fragment| Fragment {
                x: fragment.x,
                y: fragment.y,
                width: fragment.width,
                height: fragment.height,
            })
    });
    let table_shadow = std::mem::take(&mut state.table_shadow);
    drop(state);
    Ok(LiveryLayout::new(
        LayoutResult::new(boxes.into_tree(), fragments),
        None,
        BlockAlgorithmCounts {
            buckram: buckram_blocks,
            taffy: taffy_blocks,
        },
        table_bridges,
        table_shadow,
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
            if has_atomic_inline_ancestor(dom, styles, boxes, node) {
                return None;
            }
            // K4e4: an inline-table's principal box is the grid, but its atom
            // is the wrapper above it - the box that carries the element's
            // margins and contains its captions.
            if css_box.display.internal_table == Some(InternalTableRole::Grid)
                && let Some(wrapper) = css_box.parent().filter(|parent| {
                    boxes[*parent].display.internal_table == Some(InternalTableRole::Wrapper)
                })
            {
                return Some(wrapper);
            }
            Some(box_id)
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
            table_shadow: TableShadowLedger::default(),
            table_bridge_count: 0,
            pending_tables: Vec::new(),
        };
        let built = state.build_box(
            box_id,
            None,
            16.0,
            (Some(viewport_width), Some(viewport_height)),
        )?;
        // Harvest before any continue below: the shadow already ran inside
        // build_box, and both skip paths would otherwise drop its ledger.
        plane
            .table_shadow
            .merge(std::mem::take(&mut state.table_shadow));
        let Some(atomic_root) = built else {
            // No layout will run for a root that built nothing, but noted
            // tables still record their deferrals.
            state.apply_buckram_table_layout();
            plane
                .table_shadow
                .merge(std::mem::take(&mut state.table_shadow));
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
        state.apply_buckram_table_layout();
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
        // Widths are stable across the origin shift below, so verification
        // can read them either side of it.
        state.verify_table_layout(|needle| {
            fragments
                .iter()
                .find(|(candidate, _)| *candidate == needle)
                .map(|(_, rect)| *rect)
        });
        plane
            .table_shadow
            .merge(std::mem::take(&mut state.table_shadow));
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
                TreeFragment::from_horizontal_physical(*box_id, rect)
                    .with_baselines(Baselines::synthesized_from_block_end(rect.height)),
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
        table_bridge_count: 0,
        table_shadow: TableShadowLedger::default(),
        pending_tables: Vec::new(),
        pending_table_handoff: None,
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

    state.apply_buckram_table_layout(text);
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
    populate_inline_baselines(&mut state.tree);
    let (buckram_blocks, taffy_blocks) = state.tree.block_algorithm_counts();
    let table_bridges = TableBridgeCounts {
        grids: state.table_bridge_count,
    };

    let tables = state.table_fragment_plane();
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
            containing: Fragment {
                x: 0.0,
                y: 0.0,
                width: viewport_width,
                height: viewport_height,
            },
            parent: None,
        },
        &tables,
        &mut output,
        &mut text_frame,
        styles,
    )?;
    state.verify_table_layout(|box_id| {
        fragments
            .fragments_for_box(box_id)
            .next()
            .map(|fragment| Fragment {
                x: fragment.x,
                y: fragment.y,
                width: fragment.width,
                height: fragment.height,
            })
    });
    // Tables on the inline route record into the state's own ledger; tables
    // inside atomic subtrees accumulated into the plane's. Both survive.
    let mut table_shadow = std::mem::take(&mut state.table_shadow);
    table_shadow.merge(atomic.table_shadow.clone());
    drop(state);
    merge_atomic_subtrees(atomic, &boxes, &mut fragments);
    Ok(LiveryLayout::new(
        LayoutResult::new(boxes.into_tree(), fragments),
        Some(text_frame),
        BlockAlgorithmCounts {
            buckram: buckram_blocks,
            taffy: taffy_blocks,
        },
        table_bridges,
        table_shadow,
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
        containing_size: (Option<f32>, Option<f32>),
    ) -> Result<Option<AlgorithmNodeId>, LayoutError> {
        match self.boxes[box_id].origin {
            BoxOrigin::Element(node) => {
                let computed = self.styles.get(node).cloned().unwrap_or_default();
                if matches!(
                    computed.display,
                    CssDisplay::Table | CssDisplay::InlineTable
                ) {
                    self.table_bridge_count += 1;
                }
                // K4e1: the wrapper above this grid took the properties
                // CSS 2.1 section 17.4 assigns to it; the grid sees them unset.
                let computed =
                    if self.boxes[box_id].display.internal_table == Some(InternalTableRole::Grid) {
                        grid_style(&computed, containing_size)
                    } else {
                        computed
                    };
                debug_assert!(
                    self.pending_table_handoff.is_none(),
                    "a table handoff must be consumed by its own build_box call"
                );
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let child_containing_size =
                    resolved_child_containing_size(&computed, font_size, containing_size);
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
                let children = self.build_children(
                    box_id,
                    &inline_container_style,
                    font_size,
                    child_containing_size,
                )?;
                let table_handoff = self.pending_table_handoff.take();
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
                let dom_node = node;
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    taffy_style,
                    &children,
                    vec![box_id],
                );
                if let Some((grid, cell_nodes)) = table_handoff {
                    self.pending_tables.push(PendingTable {
                        table: box_id,
                        node: dom_node,
                        taffy_table: node,
                        wrapper: None,
                        captions: Vec::new(),
                        grid,
                        cell_nodes,
                        font_size,
                        containing_width: containing_size.0,
                        containing_height: containing_size.1,
                        assigned: None,
                        block: None,
                    });
                }
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                if block_style.float != FloatSide::None
                    && self.boxes[box_id].float_context == FloatContextProvenance::Inline
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
                if let Some(element) = (self.boxes[box_id].display.internal_table
                    == Some(InternalTableRole::Wrapper))
                .then(|| wrapped_table_element(self.boxes, box_id))
                .flatten()
                {
                    // K4e1: the wrapper is the box that participates in flow.
                    // Its children keep the *table's* inherited context, so
                    // they are built against the parent's font size and
                    // containing block, not the wrapper's.
                    let table = self.styles.get(element).cloned().unwrap_or_default();
                    let computed = wrapper_style(&table);
                    let font_size = font_size_px(&computed.font_size, parent_font_size);
                    let mut caption_nodes = Vec::new();
                    let mut children = Vec::new();
                    for child in wrapper_children_in_caption_order(self.boxes, self.styles, box_id)
                    {
                        let Some(child_node) =
                            self.build_box(child, inherited, parent_font_size, containing_size)?
                        else {
                            continue;
                        };
                        if self.boxes[child].display.internal_table
                            == Some(InternalTableRole::Caption)
                        {
                            let caption = self
                                .boxes
                                .origin_node(child)
                                .and_then(|node| self.styles.get(node))
                                .cloned()
                                .unwrap_or_default();
                            let em = font_size_px(&caption.font_size, font_size);
                            caption_nodes.push((
                                child_node,
                                caption_horizontal_margins(&caption, em, containing_size.0),
                            ));
                        }
                        children.push(child_node);
                    }
                    let mut taffy_style = to_taffy_style(&computed, font_size);
                    if wrapper_needs_float_fallback(self.boxes, box_id, &taffy_style) {
                        taffy_style.float = TaffyFloat::Left;
                    }
                    if let Some(width) = wrapper_width_from_grid(&to_taffy_style(
                        &grid_style(&table, containing_size),
                        font_size,
                    )) {
                        taffy_style.size.width = width;
                    }
                    let block_style = to_block_style(self.boxes, box_id, &computed, font_size);
                    let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                    let node = self.tree.new_with_children_and_block_style(
                        kind,
                        block_style,
                        taffy_style,
                        &children,
                        vec![box_id],
                    );
                    if let Some(pending) = self
                        .pending_tables
                        .iter_mut()
                        .find(|pending| pending.node == element)
                    {
                        pending.wrapper = Some(node);
                        pending.captions = caption_nodes;
                    }
                    return Ok(Some(node));
                }
                let computed = inherited.cloned().unwrap_or_default();
                let children =
                    self.build_children(box_id, &computed, parent_font_size, containing_size)?;
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

    /// Measure one cell's border-box intrinsic pair through the inline
    /// measure contract. Width sizing is neutralized for the query, because
    /// Buckram applies those constraints itself, and restored afterwards.
    fn measure_cell_intrinsics(
        &mut self,
        text: &mut TextSystem,
        cell_node: AlgorithmNodeId,
    ) -> Option<IntrinsicSizes> {
        let (dom, styles, boxes, atomic) = (self.dom, self.styles, self.boxes, self.atomic);
        let style = self.tree.style_mut(cell_node);
        let saved = (style.size.width, style.min_size.width, style.max_size.width);
        style.size.width = Dimension::auto();
        style.min_size.width = Dimension::auto();
        style.max_size.width = Dimension::auto();
        let mut measure = |available| {
            self.tree.compute_layout_with_measure(
                cell_node,
                AlgorithmSize::new(available, AlgorithmAvailableSpace::MaxContent),
                |known, available, _, context, _| {
                    let Some(context) = context else {
                        return AlgorithmSize::new(0.0, 0.0);
                    };
                    let width = match available.width {
                        AlgorithmAvailableSpace::Definite(width) => width,
                        // A nearly-zero line breaks at every opportunity; an
                        // infinite one suppresses wrapping, as in the main
                        // measure closure.
                        AlgorithmAvailableSpace::MinContent => 0.01,
                        AlgorithmAvailableSpace::MaxContent => f32::INFINITY,
                    };
                    let (measured_width, measured_height) = measure_inline_context(
                        text,
                        dom,
                        styles,
                        boxes,
                        atomic,
                        context,
                        InlineMeasureGeometry {
                            width: known.width.unwrap_or(width),
                            line_constraints: None,
                        },
                    );
                    AlgorithmSize::new(
                        known.width.unwrap_or(measured_width),
                        known.height.unwrap_or(measured_height),
                    )
                },
            );
            self.tree.layout(cell_node).width
        };
        let min = measure(AlgorithmAvailableSpace::MinContent);
        let max = measure(AlgorithmAvailableSpace::MaxContent);
        let style = self.tree.style_mut(cell_node);
        (style.size.width, style.min_size.width, style.max_size.width) = saved;
        IntrinsicSizes::new(min, max.max(min))
    }

    /// The floor a caption puts under the table's inline size.
    ///
    /// Its own min-content width plus its horizontal margins, which is what
    /// C5 and C6 of the K4e1 interop matrix pin. Unlike a cell measurement
    /// this does *not* neutralize the caption's own `width`: C7 shows a
    /// specified caption width participating like any other box, so a
    /// `width: 300px` caption puts a floor of 300 under the table. Several
    /// captions each put their own floor down and the widest one wins.
    fn measure_caption_min(
        &mut self,
        text: &mut TextSystem,
        captions: &[(AlgorithmNodeId, f32)],
    ) -> Option<f32> {
        let (dom, styles, boxes, atomic) = (self.dom, self.styles, self.boxes, self.atomic);
        captions
            .iter()
            .map(|(caption, margins)| {
                self.tree.compute_layout_with_measure(
                    *caption,
                    AlgorithmSize::new(
                        AlgorithmAvailableSpace::MinContent,
                        AlgorithmAvailableSpace::MaxContent,
                    ),
                    |known, available, _, context, _| {
                        let Some(context) = context else {
                            return AlgorithmSize::new(0.0, 0.0);
                        };
                        let width = match available.width {
                            AlgorithmAvailableSpace::Definite(width) => width,
                            AlgorithmAvailableSpace::MinContent => 0.01,
                            AlgorithmAvailableSpace::MaxContent => f32::INFINITY,
                        };
                        let (measured_width, measured_height) = measure_inline_context(
                            text,
                            dom,
                            styles,
                            boxes,
                            atomic,
                            context,
                            InlineMeasureGeometry {
                                width: known.width.unwrap_or(width),
                                line_constraints: None,
                            },
                        );
                        AlgorithmSize::new(
                            known.width.unwrap_or(measured_width),
                            known.height.unwrap_or(measured_height),
                        )
                    },
                );
                self.tree.layout(*caption).width + margins
            })
            .reduce(f32::max)
            .filter(|minimum| minimum.is_finite() && *minimum >= 0.0)
    }

    /// K4c5b and K4d6b: compute Buckram's columns for every noted table and
    /// pin them as explicit grid tracks, then lay out the block axis through
    /// the pipeline Buckram owns. Runs before the main layout pass; the
    /// formatting queries only scribble on scratch layout state the main pass
    /// recomputes.
    fn apply_buckram_table_layout(&mut self, text: &mut TextSystem) {
        let mut pendings = std::mem::take(&mut self.pending_tables);
        for pending in &mut pendings {
            let Some(computed) = self.styles.get(pending.node).cloned() else {
                continue;
            };
            let intrinsics = pending
                .cell_nodes
                .clone()
                .into_iter()
                .map(|cell_node| {
                    cell_node.and_then(|node| self.measure_cell_intrinsics(text, node))
                })
                .collect::<Vec<_>>();
            let caption_min = self.measure_caption_min(text, &pending.captions.clone());
            let columns = buckram_table_columns(
                self.dom,
                self.boxes,
                self.styles,
                &pending.grid,
                pending.table,
                pending.node,
                &computed,
                pending.font_size,
                pending.containing_width,
                caption_min,
                &intrinsics,
                &mut self.table_shadow,
            );
            if let Some(inline) = &columns {
                self.tree
                    .style_mut(pending.taffy_table)
                    .grid_template_columns =
                    inline.column_sizes.iter().copied().map(length).collect();
            }
            pending.assigned = columns;
            self.size_wrapper_from_grid(pending);
        }
        self.apply_buckram_table_rows(text, &mut pendings);
        self.pending_tables = pendings;
    }

    /// Give the wrapper the grid's border-edge width, which is CSS Tables 3
    /// section 2.2.1: "the width of the table wrapper box is the border-edge
    /// width of the table grid box inside it."
    ///
    /// Buckram's table inline sizing has just produced that width, so the rule
    /// is an assignment rather than a measurement, and an `auto` table width is
    /// no harder than a specified one - the shrink-wrapping already happened,
    /// inside the table algorithm that owns it.
    ///
    /// A table Buckram deferred has no such width. Its wrapper falls back to
    /// the `float: left` shrink-to-fit that stood in for this rule before
    /// K4e2, whose domain is now exactly the deferral set.
    fn size_wrapper_from_grid(&mut self, pending: &PendingTable<D::NodeId>) {
        let (Some(wrapper), Some(inline)) = (pending.wrapper, pending.assigned.as_ref()) else {
            return;
        };
        // The fallback float was applied when the tree was built, before this
        // width existed. Retire it here rather than leaving both in play - but
        // only where it was this route that put it there, never where the
        // author wrote `float` on the table and K4e1 migrated it.
        let authored_float = self
            .styles
            .get(pending.node)
            .is_some_and(|computed| computed.float != CssFloat::None);
        let style = self.tree.style_mut(wrapper);
        style.size.width = Dimension::length(inline.used_grid_inline_size);
        if !authored_float {
            style.float = TaffyFloat::None;
        }
    }

    /// Run Buckram's block pipeline for every table whose columns it assigned.
    ///
    /// Split from the inline pass so the shared borrows the formatter needs
    /// begin only after column assignment has released them.
    fn apply_buckram_table_rows(
        &mut self,
        text: &mut TextSystem,
        pendings: &mut [PendingTable<D::NodeId>],
    ) {
        let mut ledger = std::mem::take(&mut self.table_shadow.block);
        let Self {
            tree,
            dom,
            styles,
            boxes,
            atomic,
            ..
        } = self;
        for pending in pendings {
            let Some(inline) = pending.assigned.as_ref() else {
                continue;
            };
            let Some(computed) = styles.get(pending.node) else {
                continue;
            };
            let Some(inputs) = table_block_inputs(
                boxes,
                styles,
                &pending.grid,
                pending.table,
                computed,
                pending.font_size,
                pending.containing_height,
                &mut ledger,
            ) else {
                continue;
            };
            let mut formatter = CellFormatter(|request: TableCellLayoutInput| {
                let index = pending
                    .grid
                    .cells
                    .iter()
                    .position(|cell| cell.source == request.box_id)
                    .ok_or(TableRowLayoutError::InvalidCellOutput {
                        box_id: request.box_id,
                    })?;
                let node =
                    pending.cell_nodes[index].ok_or(TableRowLayoutError::InvalidCellOutput {
                        box_id: request.box_id,
                    })?;
                Ok(format_table_cell(
                    tree,
                    node,
                    request,
                    &inputs.cells[index],
                    |context, geometry| {
                        measure_inline_context(text, *dom, styles, boxes, atomic, context, geometry)
                    },
                ))
            });
            pending.block = buckram_table_block(
                &pending.grid,
                pending.table,
                inline,
                &inputs,
                pending.containing_height,
                &mut formatter,
                &mut ledger,
            );
            if let Some(block) = &pending.block {
                commit_table_block(tree, pending.taffy_table, block, inline, |box_id| {
                    pending
                        .grid
                        .cells
                        .iter()
                        .position(|cell| cell.source == box_id)
                        .and_then(|index| pending.cell_nodes[index])
                });
            }
        }
        self.table_shadow.block = ledger;
    }

    /// The structural fragments Buckram emitted for every table it laid out,
    /// keyed by the grid box that owns them. Fragment collection commits them
    /// where the walk reaches that grid.
    fn table_fragment_plane(&self) -> TableFragmentPlane {
        self.pending_tables
            .iter()
            .filter_map(|pending| {
                pending
                    .block
                    .as_ref()
                    .map(|block| (pending.table, block.fragments.clone()))
            })
            .collect()
    }

    /// Assert the painted fragments honored every assigned column vector, and
    /// record how far the painted cells sit from Buckram's block rectangles.
    fn verify_table_layout(&mut self, live_rect_of: impl Fn(BoxId) -> Option<Fragment>) {
        let pendings = std::mem::take(&mut self.pending_tables);
        for pending in pendings {
            verify_one_table(&pending, &live_rect_of, &mut self.table_shadow);
        }
    }

    fn build_children(
        &mut self,
        parent: BoxId,
        parent_style: &ComputedValues,
        parent_font_size: f32,
        containing_size: (Option<f32>, Option<f32>),
    ) -> Result<Vec<AlgorithmNodeId>, LayoutError> {
        // A `display: table` box takes its flattened cells directly, matching
        // the precomputed atomic subtree.
        if matches!(
            parent_style.display,
            CssDisplay::Table | CssDisplay::InlineTable
        ) && self
            .boxes
            .origin_node(parent)
            .is_some_and(|node| table_is_flattenable(self.dom, self.styles, node))
        {
            let table = build_table_grid(self.boxes, self.dom, parent);
            let mut cell_nodes = Vec::with_capacity(table.cells.len());
            let mut children = Vec::with_capacity(table.cells.len());
            for cell in &table.cells {
                let built = self.build_box(
                    cell.source,
                    Some(parent_style),
                    parent_font_size,
                    containing_size,
                )?;
                cell_nodes.push(built);
                let Some(node) = built else {
                    continue;
                };
                place_table_cell(self.tree.style_mut(node), cell);
                children.push(node);
            }
            // K4c5b: hand the grid to build_box, which creates the table's
            // algorithm node and notes the table for Buckram column
            // assignment before the main layout pass.
            self.pending_table_handoff = Some((table, cell_nodes));
            return Ok(children);
        }
        self.build_flow_children(
            parent,
            self.boxes[parent].children().to_vec(),
            parent_style,
            parent_font_size,
            containing_size,
        )
    }

    fn build_flow_children(
        &mut self,
        parent: BoxId,
        child_ids: Vec<BoxId>,
        parent_style: &ComputedValues,
        parent_font_size: f32,
        containing_size: (Option<f32>, Option<f32>),
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
            if let Some(node) =
                self.build_box(child, Some(parent_style), parent_font_size, containing_size)?
            {
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
        // The inline formatter owns the distinction between wrapped and
        // no-wrap lines. Both still need the current float band to choose a
        // line origin and, when possible, the next wider band.
        self.tree.enable_float_line_constraints(node);
        Ok(node)
    }
}

impl<D> BuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    /// One intrinsic inline query through the same measure contract the main
    /// layout uses. Only sound once painted fragments have been collected,
    /// because it recomputes the subtree's scratch layout.
    fn measure_intrinsic_width(
        &mut self,
        node: AlgorithmNodeId,
        available: AlgorithmAvailableSpace,
    ) -> f32 {
        self.tree.compute_layout_with_measure(
            node,
            AlgorithmSize::new(available, AlgorithmAvailableSpace::MaxContent),
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
        self.tree.layout(node).width
    }

    /// Measure one cell's border-box intrinsic pair through the live measure
    /// contract. The cell's own width sizing is neutralized for the query,
    /// because Buckram applies those constraints itself, and restored
    /// afterwards so the main layout pass sees the real style.
    fn measure_cell_intrinsics(&mut self, cell_node: AlgorithmNodeId) -> Option<IntrinsicSizes> {
        let style = self.tree.style_mut(cell_node);
        let saved = (style.size.width, style.min_size.width, style.max_size.width);
        style.size.width = Dimension::auto();
        style.min_size.width = Dimension::auto();
        style.max_size.width = Dimension::auto();
        let min = self.measure_intrinsic_width(cell_node, AlgorithmAvailableSpace::MinContent);
        let max = self.measure_intrinsic_width(cell_node, AlgorithmAvailableSpace::MaxContent);
        let style = self.tree.style_mut(cell_node);
        (style.size.width, style.min_size.width, style.max_size.width) = saved;
        IntrinsicSizes::new(min, max.max(min))
    }

    /// The floor a caption puts under the table's inline size.
    ///
    /// Its own min-content width plus its horizontal margins, which is what
    /// C5 and C6 of the K4e1 interop matrix pin. Unlike a cell measurement
    /// this does *not* neutralize the caption's own `width`: C7 shows a
    /// specified caption width participating like any other box, so a
    /// `width: 300px` caption puts a floor of 300 under the table. Several
    /// captions each put their own floor down and the widest one wins.
    fn measure_caption_min(&mut self, captions: &[(AlgorithmNodeId, f32)]) -> Option<f32> {
        captions
            .iter()
            .map(|(caption, margins)| {
                self.measure_intrinsic_width(*caption, AlgorithmAvailableSpace::MinContent)
                    + margins
            })
            .reduce(f32::max)
            .filter(|minimum| minimum.is_finite() && *minimum >= 0.0)
    }

    /// K4c5b and K4d6b: compute Buckram's columns for every noted table and
    /// pin them as explicit grid tracks, then lay out the block axis. Runs
    /// after the tree is built and before the main layout pass; the queries
    /// only scribble on scratch layout state that the main pass recomputes.
    fn apply_buckram_table_layout(&mut self) {
        let mut pendings = std::mem::take(&mut self.pending_tables);
        for pending in &mut pendings {
            let Some(computed) = self.styles.get(pending.node).cloned() else {
                continue;
            };
            let intrinsics = pending
                .cell_nodes
                .clone()
                .into_iter()
                .map(|cell_node| cell_node.and_then(|node| self.measure_cell_intrinsics(node)))
                .collect::<Vec<_>>();
            let caption_min = self.measure_caption_min(&pending.captions.clone());
            let columns = buckram_table_columns(
                self.dom,
                self.boxes,
                self.styles,
                &pending.grid,
                pending.table,
                pending.node,
                &computed,
                pending.font_size,
                pending.containing_width,
                caption_min,
                &intrinsics,
                &mut self.table_shadow,
            );
            if let Some(inline) = &columns {
                self.tree
                    .style_mut(pending.taffy_table)
                    .grid_template_columns =
                    inline.column_sizes.iter().copied().map(length).collect();
            }
            pending.assigned = columns;
            self.size_wrapper_from_grid(pending);
        }
        self.apply_buckram_table_rows(&mut pendings);
        self.pending_tables = pendings;
    }

    /// Give the wrapper the grid's border-edge width, which is CSS Tables 3
    /// section 2.2.1: "the width of the table wrapper box is the border-edge
    /// width of the table grid box inside it."
    ///
    /// Buckram's table inline sizing has just produced that width, so the rule
    /// is an assignment rather than a measurement, and an `auto` table width is
    /// no harder than a specified one - the shrink-wrapping already happened,
    /// inside the table algorithm that owns it.
    ///
    /// A table Buckram deferred has no such width. Its wrapper falls back to
    /// the `float: left` shrink-to-fit that stood in for this rule before
    /// K4e2, whose domain is now exactly the deferral set.
    fn size_wrapper_from_grid(&mut self, pending: &PendingTable<D::NodeId>) {
        let (Some(wrapper), Some(inline)) = (pending.wrapper, pending.assigned.as_ref()) else {
            return;
        };
        // The fallback float was applied when the tree was built, before this
        // width existed. Retire it here rather than leaving both in play - but
        // only where it was this route that put it there, never where the
        // author wrote `float` on the table and K4e1 migrated it.
        let authored_float = self
            .styles
            .get(pending.node)
            .is_some_and(|computed| computed.float != CssFloat::None);
        let style = self.tree.style_mut(wrapper);
        style.size.width = Dimension::length(inline.used_grid_inline_size);
        if !authored_float {
            style.float = TaffyFloat::None;
        }
    }

    /// Run Buckram's block pipeline for every table whose columns it assigned.
    fn apply_buckram_table_rows(&mut self, pendings: &mut [PendingTable<D::NodeId>]) {
        let mut ledger = std::mem::take(&mut self.table_shadow.block);
        let Self {
            tree,
            styles,
            boxes,
            ..
        } = self;
        for pending in pendings {
            let Some(inline) = pending.assigned.as_ref() else {
                continue;
            };
            let Some(computed) = styles.get(pending.node) else {
                continue;
            };
            let Some(inputs) = table_block_inputs(
                boxes,
                styles,
                &pending.grid,
                pending.table,
                computed,
                pending.font_size,
                pending.containing_height,
                &mut ledger,
            ) else {
                continue;
            };
            let mut formatter = CellFormatter(|request: TableCellLayoutInput| {
                let index = pending
                    .grid
                    .cells
                    .iter()
                    .position(|cell| cell.source == request.box_id)
                    .ok_or(TableRowLayoutError::InvalidCellOutput {
                        box_id: request.box_id,
                    })?;
                let node =
                    pending.cell_nodes[index].ok_or(TableRowLayoutError::InvalidCellOutput {
                        box_id: request.box_id,
                    })?;
                Ok(format_table_cell(
                    tree,
                    node,
                    request,
                    &inputs.cells[index],
                    |context: &mut TextMeasure, geometry| {
                        (
                            context.max_width.min(geometry.width.max(0.0)),
                            context.height,
                        )
                    },
                ))
            });
            pending.block = buckram_table_block(
                &pending.grid,
                pending.table,
                inline,
                &inputs,
                pending.containing_height,
                &mut formatter,
                &mut ledger,
            );
            if let Some(block) = &pending.block {
                commit_table_block(tree, pending.taffy_table, block, inline, |box_id| {
                    pending
                        .grid
                        .cells
                        .iter()
                        .position(|cell| cell.source == box_id)
                        .and_then(|index| pending.cell_nodes[index])
                });
            }
        }
        self.table_shadow.block = ledger;
    }

    /// The structural fragments Buckram emitted for every table it laid out,
    /// keyed by the grid box that owns them. Fragment collection commits them
    /// where the walk reaches that grid.
    fn table_fragment_plane(&self) -> TableFragmentPlane {
        self.pending_tables
            .iter()
            .filter_map(|pending| {
                pending
                    .block
                    .as_ref()
                    .map(|block| (pending.table, block.fragments.clone()))
            })
            .collect()
    }

    /// Assert the painted fragments honored every assigned column vector, and
    /// record how far the painted cells sit from Buckram's block rectangles.
    /// Runs after fragment collection.
    fn verify_table_layout(&mut self, live_rect_of: impl Fn(BoxId) -> Option<Fragment>) {
        let pendings = std::mem::take(&mut self.pending_tables);
        for pending in pendings {
            verify_one_table(&pending, &live_rect_of, &mut self.table_shadow);
        }
    }

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
                if matches!(
                    computed.display,
                    CssDisplay::Table | CssDisplay::InlineTable
                ) {
                    self.table_bridge_count += 1;
                }
                // K4e1: the wrapper above this grid took the properties
                // CSS 2.1 section 17.4 assigns to it; the grid sees them unset.
                let computed =
                    if self.boxes[box_id].display.internal_table == Some(InternalTableRole::Grid) {
                        grid_style(&computed, containing_size)
                    } else {
                        computed
                    };
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
                let table = (matches!(
                    computed.display,
                    CssDisplay::Table | CssDisplay::InlineTable
                ) && table_is_flattenable(self.dom, self.styles, node))
                .then(|| build_table_grid(self.boxes, self.dom, box_id));
                let mut table_cell_nodes = Vec::new();
                let children = if let Some(table) = table.as_ref() {
                    let mut children = Vec::with_capacity(table.cells.len());
                    for cell in &table.cells {
                        let built = self.build_box(
                            cell.source,
                            Some(&computed),
                            font_size,
                            child_containing_size,
                        )?;
                        table_cell_nodes.push(built);
                        let Some(taffy_node) = built else {
                            continue;
                        };
                        place_table_cell(self.tree.style_mut(taffy_node), cell);
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
                let dom_node = node;
                let node = self.tree.new_with_children_and_block_style(
                    kind,
                    block_style,
                    taffy_style,
                    &children,
                    Some(box_id),
                );
                // K4c5b: Buckram owns this table's columns. They are computed
                // before the main layout pass, once the whole tree exists and
                // intrinsic queries can run, and pinned as explicit tracks.
                if let Some(grid) = table {
                    self.pending_tables.push(PendingTable {
                        table: box_id,
                        node: dom_node,
                        taffy_table: node,
                        wrapper: None,
                        captions: Vec::new(),
                        grid,
                        cell_nodes: std::mem::take(&mut table_cell_nodes),
                        font_size,
                        containing_width: containing_size.0,
                        containing_height: containing_size.1,
                        assigned: None,
                        block: None,
                    });
                }
                if supports_nested_float_state(&self.boxes[box_id], block_style, kind) {
                    self.tree.enable_nested_float_state(node);
                }
                if block_style.float != FloatSide::None
                    && self.boxes[box_id].float_context == FloatContextProvenance::Inline
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
                if let Some(element) = (self.boxes[box_id].display.internal_table
                    == Some(InternalTableRole::Wrapper))
                .then(|| wrapped_table_element(self.boxes, box_id))
                .flatten()
                {
                    // See InlineBuildState's corresponding K4e1 wrapper.
                    let table = self.styles.get(element).cloned().unwrap_or_default();
                    let computed = wrapper_style(&table);
                    let font_size = font_size_px(&computed.font_size, parent_font_size);
                    let mut caption_nodes = Vec::new();
                    let mut children = Vec::new();
                    for child in wrapper_children_in_caption_order(self.boxes, self.styles, box_id)
                    {
                        let Some(child_node) =
                            self.build_box(child, inherited, parent_font_size, containing_size)?
                        else {
                            continue;
                        };
                        if self.boxes[child].display.internal_table
                            == Some(InternalTableRole::Caption)
                        {
                            let caption = self
                                .boxes
                                .origin_node(child)
                                .and_then(|node| self.styles.get(node))
                                .cloned()
                                .unwrap_or_default();
                            let em = font_size_px(&caption.font_size, font_size);
                            caption_nodes.push((
                                child_node,
                                caption_horizontal_margins(&caption, em, containing_size.0),
                            ));
                        }
                        children.push(child_node);
                    }
                    let mut taffy_style = to_taffy_style(&computed, font_size);
                    if wrapper_needs_float_fallback(self.boxes, box_id, &taffy_style) {
                        taffy_style.float = TaffyFloat::Left;
                    }
                    if let Some(width) = wrapper_width_from_grid(&to_taffy_style(
                        &grid_style(&table, containing_size),
                        font_size,
                    )) {
                        taffy_style.size.width = width;
                    }
                    let block_style = to_block_style(self.boxes, box_id, &computed, font_size);
                    let kind = algorithm_kind(&self.boxes[box_id], children.is_empty());
                    let node = self.tree.new_with_children_and_block_style(
                        kind,
                        block_style,
                        taffy_style,
                        &children,
                        Some(box_id),
                    );
                    if let Some(pending) = self
                        .pending_tables
                        .iter_mut()
                        .find(|pending| pending.node == element)
                    {
                        pending.wrapper = Some(node);
                        pending.captions = caption_nodes;
                    }
                    return Ok(Some(node));
                }
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

fn fragment_baselines<Id, Context, Source>(
    tree: &AlgorithmTree<Style, Context, Source>,
    boxes: &GeneratedBoxTree<Id>,
    node: AlgorithmNodeId,
    box_id: BoxId,
    rect: Fragment,
) -> Baselines
where
    Id: Copy + Eq + Hash,
{
    let css_box = &boxes[box_id];
    if css_box.display.outside == Some(DisplayOutside::Inline)
        && css_box.display.inside == Some(DisplayInside::FlowRoot)
    {
        // The admitted atomic lane currently has no line-baseline provider of
        // its own. Its modeled fallback is therefore its block-end edge,
        // rather than a value inferred from the parent's line rectangle.
        Baselines::synthesized_from_block_end(rect.height)
    } else {
        tree.baselines(node)
    }
}

#[derive(Clone, Copy)]
struct FragmentCursor {
    origin: Point<f32>,
    containing: Fragment,
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
    // K4e4 deleted K4a's wrapper/grid exclusion here: an inline-table's
    // wrapper is inline-level and rides the atomic-inline lane. The grid
    // never reaches this test, because a wrapper's children are built
    // directly rather than through flow-children grouping.
    css_box.display.outside == Some(DisplayOutside::Inline)
        && css_box.float == FloatSide::None
        && matches!(
            css_box.positioning,
            PositioningScheme::Static | PositioningScheme::Relative | PositioningScheme::Sticky
        )
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
            | CssDisplay::InlineTable
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

/// The element a table wrapper box splits its computed values with.
///
/// A wrapper generated by fixup around stray table parts wraps an *anonymous*
/// grid and owns no element of its own, so nothing migrates onto it and it
/// stays an ordinary anonymous block. The wrapper's grid is its last child;
/// captions are the earlier ones.
fn wrapped_table_element<Id>(boxes: &GeneratedBoxTree<Id>, wrapper: BoxId) -> Option<Id>
where
    Id: Copy + Eq + Hash,
{
    let grid = boxes[wrapper]
        .children()
        .iter()
        .copied()
        .find(|child| boxes[*child].display.internal_table == Some(InternalTableRole::Grid))?;
    legacy_origin_node(boxes, grid)
}

/// The wrapper's width under CSS Tables 3 section 2.2.1: "the width of the
/// table wrapper box is the border-edge width of the table grid box inside
/// it."
///
/// A definite table width makes that computable before layout, which is what
/// stops a wrapper from stretching when it is a flex or grid item - it is
/// never `auto` in the sense stretching means, because the grid decides it.
/// K4e2 resolves the `auto` case from Buckram's own table sizing instead of
/// guessing at it, but that runs after the tree is built, and a flex item's
/// style has to be right before it.
fn wrapper_width_from_grid(grid: &Style) -> Option<Dimension> {
    let width = grid.size.width.into_option()?;
    if grid.box_sizing == BoxSizing::BorderBox {
        return Some(Dimension::length(width));
    }
    let outside = [
        grid.padding.left,
        grid.padding.right,
        grid.border.left,
        grid.border.right,
    ]
    .into_iter()
    .map(|edge| Dimension::from(edge).into_option())
    .sum::<Option<f32>>()?;
    Some(Dimension::length(width + outside))
}

/// The wrapper's children in the order they are laid out.
///
/// CSS 2.1 section 17.4.1 puts a caption above or below the table grid inside
/// the wrapper's margins, according to `caption-side`. Buckram's box tree
/// keeps every caption before the grid, which is the order the source implies;
/// this is the order the page shows. Within a side, source order is kept, so
/// two top captions stack in the order they were written.
fn wrapper_children_in_caption_order<Id>(
    boxes: &GeneratedBoxTree<Id>,
    styles: &StylePlane<Id>,
    wrapper: BoxId,
) -> Vec<BoxId>
where
    Id: Copy + Eq + Hash,
{
    let below = |child: BoxId| {
        boxes[child].display.internal_table == Some(InternalTableRole::Caption)
            && boxes
                .origin_node(child)
                .and_then(|node| styles.get(node))
                .is_some_and(|computed| computed.caption_side == CaptionSide::Bottom)
    };
    let children = boxes[wrapper].children();
    children
        .iter()
        .copied()
        .filter(|child| !below(*child))
        .chain(children.iter().copied().filter(|child| below(*child)))
        .collect()
}

/// The horizontal margins a caption carries into its contribution.
///
/// C5 of the K4e1 interop matrix pins that both engines include them: a
/// 176-wide caption with `margin-left: 30px` puts a floor of 206 under the
/// table, not 176.
fn caption_horizontal_margins(computed: &ComputedValues, em: f32, basis: Option<f32>) -> f32 {
    let resolve = |margin: Margin| match margin {
        // An auto margin on a caption resolves against a width the table does
        // not have yet, and neither engine lets it widen the table.
        Margin::Auto => 0.0,
        Margin::Value(value) => {
            absolute_length_percentage(value, em, LIVE_ROOT_FONT_SIZE, basis.unwrap_or(0.0))
        },
    };
    resolve(computed.margin_left) + resolve(computed.margin_right)
}

/// Whether a wrapper needs the `float: left` shrink-to-fit compatibility route.
///
/// Only a block-level in-flow wrapper does. The wrapper of an inline-table is
/// an atomic inline, an absolutely positioned wrapper is shrink-to-fit under
/// CSS 2.1 section 10.3.7, and a flex or grid item is sized by its container -
/// all three already shrink-wrap, and floating them instead would take them
/// out of the formatting context they belong to.
fn wrapper_needs_float_fallback<Id>(
    boxes: &GeneratedBoxTree<Id>,
    wrapper: BoxId,
    style: &Style,
) -> bool
where
    Id: Copy + Eq + Hash,
{
    let laid_out_as_an_item = boxes[wrapper].parent().is_some_and(|parent| {
        matches!(
            boxes[parent].formatting_context,
            Some(FormattingContextKind::Flex | FormattingContextKind::Grid)
        )
    });
    boxes[wrapper].display.outside == Some(DisplayOutside::Block)
        && !laid_out_as_an_item
        && style.position != Position::Absolute
        && style.float == TaffyFloat::None
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

/// Record flow-relative fragment geometry without changing the physical
/// rectangle consumed by the retained text and paint lanes.
fn fragment_for_box<Id>(
    boxes: &GeneratedBoxTree<Id>,
    box_id: BoxId,
    physical: Fragment,
    relative: Fragment,
    containing: Fragment,
) -> TreeFragment
where
    Id: Copy + Eq + Hash,
{
    let flow = boxes[box_id].flow;
    if flow.is_horizontal() {
        TreeFragment::from_horizontal_physical(box_id, physical)
    } else {
        let logical = flow.logical_rect(
            relative,
            PhysicalSize {
                width: containing.width,
                height: containing.height,
            },
        );
        TreeFragment::from_physical_with_logical(box_id, physical, logical, flow)
    }
}

/// The structural fragments Buckram emitted for each live table, keyed by the
/// grid box that owns them.
type TableFragmentPlane = HashMap<BoxId, TableFragments>;

/// Commit the row groups, rows, column groups, and columns Buckram emitted.
///
/// Cells are deliberately left out: the ordinary walk already produces them
/// from the algorithm tree, with their contents, baselines, and text. What the
/// walk cannot produce is a box with no algorithm node, and that is every one
/// of these. The bridge flattened rows and groups away before the backend saw
/// them, so a `<tr>` background has never had a fragment to paint into.
///
/// These are pushed before the walk descends into the cells, so each cell's
/// structural-parent lookup finds its own row rather than falling back to the
/// grid. Buckram guarantees parents precede children, so one forward pass
/// resolves every parent.
///
/// Rectangles are logical and the live path is horizontal LTR throughout, so
/// inline maps to x and block to y.
fn commit_table_structure(
    emitted: &TableFragments,
    grid_origin: Point<f32>,
    grid_fragment: FragmentId,
    output: &mut FragmentOutput<'_>,
) {
    let mut ids: Vec<Option<FragmentId>> = vec![None; emitted.fragments().len()];
    for (index, fragment) in emitted.fragments().iter().enumerate() {
        match fragment.role {
            // The walk already pushed the grid's own fragment; record it so
            // children can hang from it.
            TableFragmentRole::Grid => {
                ids[index] = Some(grid_fragment);
                continue;
            },
            TableFragmentRole::Cell => continue,
            _ => {},
        }
        // A track created implicitly by placement has no CSS box, so there is
        // no identity to attribute a fragment to.
        let Some(box_id) = fragment.box_id else {
            continue;
        };
        let parent = fragment
            .parent
            .and_then(|at| ids.get(at).copied().flatten())
            .unwrap_or(grid_fragment);
        let rect = Fragment {
            x: grid_origin.x + fragment.rect.inline_start,
            y: grid_origin.y + fragment.rect.block_start,
            width: fragment.rect.inline_size,
            height: fragment.rect.block_size,
        };
        ids[index] = Some(output.fragments.push(
            TreeFragment::from_horizontal_physical(box_id, rect),
            Some(parent),
            Some(parent),
        ));
    }
}

fn collect_fragments<Id>(
    tree: &AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
    boxes: &GeneratedBoxTree<Id>,
    node: AlgorithmNodeId,
    cursor: FragmentCursor,
    tables: &TableFragmentPlane,
    output: &mut FragmentOutput<'_>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node);
    let origin = Point {
        x: cursor.origin.x + computed.x,
        y: cursor.origin.y + computed.y,
    };
    let rect = Fragment {
        x: origin.x,
        y: origin.y,
        width: computed.width,
        height: computed.height,
    };
    let mut child_parent = cursor.parent;
    {
        let source = *tree.source(node);
        let origin_node = match source {
            Some(box_id) => {
                let structural_parent = boxes[box_id].parent().and_then(|parent_box| {
                    output
                        .fragments
                        .fragment_ids_for_box(parent_box)
                        .last()
                        .copied()
                });
                let parent = structural_parent.or(cursor.parent);
                let flow = boxes[box_id].flow;
                let fragment = if flow.is_horizontal() {
                    TreeFragment::from_horizontal_physical(box_id, rect)
                } else {
                    let logical_rect = flow.logical_rect(
                        PhysicalRect {
                            x: computed.x,
                            y: computed.y,
                            width: computed.width,
                            height: computed.height,
                        },
                        PhysicalSize {
                            width: cursor.containing.width,
                            height: cursor.containing.height,
                        },
                    );
                    TreeFragment::from_physical_with_logical(box_id, rect, logical_rect, flow)
                };
                let id = output.fragments.push(
                    fragment.with_baselines(fragment_baselines(tree, boxes, node, box_id, rect)),
                    parent,
                    parent,
                );
                child_parent = Some(id);
                if let Some(emitted) = tables.get(&box_id) {
                    commit_table_structure(emitted, origin, id, output);
                }
                legacy_origin_node(boxes, box_id)
            },
            None => None,
        };
        let _ = origin_node;
    }
    for child in tree.children(node) {
        collect_fragments(
            tree,
            boxes,
            *child,
            FragmentCursor {
                origin,
                containing: rect,
                parent: child_parent,
            },
            tables,
            output,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "one recursive walk; six of the eight are context invariant across               the whole walk and the other two are the per-node cursor"
)]
fn collect_inline_fragments<Id>(
    tree: &AlgorithmTree<Style, InlineMeasure, Vec<BoxId>>,
    boxes: &GeneratedBoxTree<Id>,
    node: AlgorithmNodeId,
    cursor: FragmentCursor,
    tables: &TableFragmentPlane,
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
    let relative_rect = Fragment {
        x: computed.x,
        y: computed.y,
        width: computed.width,
        height: computed.height,
    };
    let rect = Fragment {
        x: origin.x,
        y: origin.y,
        width: computed.width,
        height: computed.height,
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
                    let relative_line = Fragment {
                        x: line_fragment.x - cursor.origin.x,
                        y: line_fragment.y - cursor.origin.y,
                        width: line_fragment.width,
                        height: line_fragment.height,
                    };
                    let fragment_id = output.fragments.push(
                        fragment_for_box(
                            boxes,
                            box_id,
                            *line_fragment,
                            relative_line,
                            cursor.containing,
                        )
                        .with_baselines(fragment_baselines(
                            tree,
                            boxes,
                            node,
                            box_id,
                            *line_fragment,
                        )),
                        parent,
                        parent,
                    );
                    child_parent.get_or_insert(fragment_id);
                }
            } else {
                let fragment_id = output.fragments.push(
                    fragment_for_box(boxes, box_id, rect, relative_rect, cursor.containing)
                        .with_baselines(fragment_baselines(tree, boxes, node, box_id, rect)),
                    parent,
                    parent,
                );
                if let Some(emitted) = tables.get(&box_id) {
                    commit_table_structure(emitted, origin, fragment_id, output);
                }
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
                containing: rect,
                parent: child_parent,
            },
            tables,
            output,
            text_frame,
            styles,
        )?;
    }
    Ok(())
}

/// Normalize HTML table attributes at the Livery boundary, then ask Buckram
/// to derive one topology from the generated CSS boxes. CSS-display tables
/// receive the same model with the default one-by-one spans.
fn build_table_grid<D>(boxes: &GeneratedBoxTree<D::NodeId>, dom: &D, grid: BoxId) -> TableGrid
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn visit<D>(
        boxes: &GeneratedBoxTree<D::NodeId>,
        dom: &D,
        box_id: BoxId,
        inputs: &mut TableGridInputs,
    ) where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        if let Some(node) = boxes.origin_node(box_id) {
            match boxes[box_id].display.internal_table {
                Some(InternalTableRole::Cell) if html_element(dom, node, &["td", "th"]) => {
                    inputs.set_cell(box_id, html_cell_input(dom, node));
                },
                Some(InternalTableRole::Column) if html_element(dom, node, &["col"]) => {
                    inputs.set_column(
                        box_id,
                        TableTrackInput {
                            span: html_limited_span(html_attribute(dom, node, "span"), 1_000),
                        },
                    );
                },
                Some(InternalTableRole::ColumnGroup) if html_element(dom, node, &["colgroup"]) => {
                    inputs.set_column_group(
                        box_id,
                        TableTrackInput {
                            span: html_limited_span(html_attribute(dom, node, "span"), 1_000),
                        },
                    );
                },
                _ => {},
            }
        }
        for child in boxes[box_id].children() {
            visit(boxes, dom, *child, inputs);
        }
    }

    let mut inputs = TableGridInputs::default();
    visit(boxes, dom, grid, &mut inputs);
    TableGrid::from_box_tree(&**boxes, grid, &inputs)
}

fn html_element<D>(dom: &D, node: D::NodeId, names: &[&str]) -> bool
where
    D: LayoutDom,
{
    dom.element_name(node).is_some_and(|name| {
        name.ns.as_ref() == "http://www.w3.org/1999/xhtml"
            && names
                .iter()
                .any(|candidate| name.local.as_ref().eq_ignore_ascii_case(candidate))
    })
}

fn html_attribute<'dom, D>(dom: &'dom D, node: D::NodeId, local: &str) -> Option<&'dom str>
where
    D: LayoutDom,
{
    dom.attribute(node, &Namespace::from(""), &LocalName::from(local))
}

fn html_limited_span(value: Option<&str>, maximum: usize) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(maximum))
        .unwrap_or(1)
}

fn html_cell_input<D>(dom: &D, node: D::NodeId) -> TableCellInput
where
    D: LayoutDom,
{
    let row_span = match html_attribute(dom, node, "rowspan")
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        Some(0) => TableRowSpan::ToEndOfGroup,
        Some(value) => TableRowSpan::Count(value.min(65_534)),
        None => TableRowSpan::Count(1),
    };
    TableCellInput {
        column_span: html_limited_span(html_attribute(dom, node, "colspan"), 1_000),
        row_span,
    }
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
                Some(
                    CssDisplay::TableRowGroup
                    | CssDisplay::TableHeaderGroup
                    | CssDisplay::TableFooterGroup,
                ) if positioned::<D>(styles, child) || !walk(dom, styles, child) => {
                    return false;
                },
                _ => {},
            }
        }
        true
    }

    walk(dom, styles, table)
}

/// Pin a cell's temporary bridge style to its TableGrid start slot. Buckram
/// retains the spans; K4d replaces this Grid bridge before span layout runs.
fn place_table_cell(style: &mut Style, cell: &TableCell) {
    style.grid_row = Line {
        start: line(cell.row as i16 + 1),
        end: GridPlacement::Auto,
    };
    style.grid_column = Line {
        start: line(cell.column as i16 + 1),
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
            // K4e4: an inline-table is an atomic inline like an inline-block;
            // its wrapper occupies line space as a unit.
            || style.display == CssDisplay::InlineTable
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
    // K4e4: the hit target is the node's outermost box - a table element's
    // wrapper - so the caption area belongs to the table when nothing deeper
    // claims it, and the caption element wins inside its own rectangle by
    // paint order.
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
    let table_row = computed.display == CssDisplay::TableRow;
    let _ = table_row;
    let display = match computed.display {
        CssDisplay::None => Display::None,
        CssDisplay::Flex => Display::Flex,
        CssDisplay::Grid => Display::Grid,
        // A table box is laid out as a grid whose children are its
        // TableGrid starts; K4d replaces this compatibility bridge. An
        // inline-table's grid is the same box - inline-ness lives on the
        // wrapper (K4e4).
        CssDisplay::Table | CssDisplay::InlineTable => Display::Grid,
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

    fn node_by_id(
        dom: &StaticDocument,
        node: <StaticDocument as LayoutDom>::NodeId,
        id: &str,
    ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
        if dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(id) {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|child| node_by_id(dom, child, id))
    }

    #[test]
    fn html_table_spans_are_normalized_before_buckram_receives_them() {
        let dom = StaticDocument::parse(
            "<table id=table><tbody><tr><td id=first colspan=9001 rowspan=0></td></tr><tr><td id=second></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = node_by_id(&dom, dom.document(), "table").expect("table");
        let grid = boxes.principal_box(table).expect("table grid");
        let model = build_table_grid(&boxes, &dom, grid);

        assert_eq!(model.cells[0].column_span, 1_000);
        assert_eq!(model.cells[0].row_span, 2);
        assert_eq!(model.cells[1].column, 1_000);
    }

    #[test]
    fn css_display_tables_do_not_consume_html_span_attributes() {
        let dom = StaticDocument::parse(
            "<div id=table><div id=row><div id=cell colspan=9 rowspan=0></div></div></div>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "#table { display: table; } #row { display: table-row; } #cell { display: table-cell; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = node_by_id(&dom, dom.document(), "table").expect("table");
        let grid = boxes.principal_box(table).expect("table grid");
        let model = build_table_grid(&boxes, &dom, grid);

        assert_eq!(model.cells[0].column_span, 1);
        assert_eq!(model.cells[0].row_span, 1);
    }

    #[test]
    fn live_table_bridge_count_reports_each_grid_route_once() {
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td>one</td><td>two</td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );

        let layout = layout(&dom, &styles, 320.0, 240.0).expect("layout");
        assert_eq!(
            layout.table_bridge_counts(),
            TableBridgeCounts { grids: 1 },
            "the table's Grid/Flex compatibility route is counted once at its grid"
        );
    }

    fn fixed_table_ledger(spacing: &str) -> crate::table_shadow::TableShadowLedger {
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td id=first>one</td><td>two</td><td>three</td></tr></tbody></table>",
        );
        let css = format!(
            "table {{ display: table; table-layout: fixed; width: 300px; border-spacing: {spacing}; }} tbody {{ display: table-row-group; }} tr {{ display: table-row; }} td {{ display: table-cell; }} #first {{ width: 120px; }}"
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[&css]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        layout(&dom, &styles, 320.0, 240.0)
            .expect("layout")
            .table_shadow_ledger()
            .clone()
    }

    fn assert_assigned_and_honored(ledger: &crate::table_shadow::TableShadowLedger) {
        assert_eq!(
            ledger.assigned, 1,
            "Buckram did not size the table: {ledger:?}"
        );
        assert_eq!(
            ledger.verified, 1,
            "the assignment was not verified: {ledger:?}"
        );
        assert!(
            ledger.is_silent(),
            "the bridge did not honor Buckram's tracks: {ledger:?}"
        );
        assert_eq!(ledger.honored, 1, "{ledger:?}");
    }

    /// K4c5b: Buckram owns the fixed algorithm and the painted fragments
    /// honor its columns exactly.
    #[test]
    fn k4c5b_fixed_table_columns_are_buckram_owned() {
        assert_assigned_and_honored(&fixed_table_ledger("0"));
    }

    /// The first K4c5a divergence, resolved by authority. The deleted live
    /// helper omitted `border-spacing` from CSS 2.1 17.5.2.1's distribution
    /// and painted 89px columns; Buckram's 85px columns now paint, and the
    /// fragment verification proves it.
    #[test]
    fn k4c5b_fixed_border_spacing_distribution_is_painted() {
        assert_assigned_and_honored(&fixed_table_ledger("2px"));
    }

    /// K4c5b on the production text path: a fixed table routed through
    /// `InlineBuildState` receives Buckram columns before the main pass. This
    /// route previously had no fixed sizing at all.
    #[test]
    fn k4c5b_text_path_fixed_tables_are_buckram_owned() {
        let dom = StaticDocument::parse(
            "<p>before the table</p><table><tbody><tr><td id=first>one</td><td>two</td><td>three</td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; table-layout: fixed; width: 300px; border-spacing: 0; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; } #first { width: 120px; }",
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
        assert_assigned_and_honored(layout.table_shadow_ledger());
    }

    /// A fixed table inside an inline-block builds under an atomic subtree's
    /// own `BuildState`; its assignment and verification survive into
    /// `LiveryLayout` through the accumulated plane ledger.
    ///
    /// Span-based markup because a `<div>` start tag inside `<p>` closes the
    /// paragraph at the HTML parser, before box generation runs.
    #[test]
    fn k4c5b_tables_inside_atomic_inline_subtrees_are_buckram_owned() {
        let dom = StaticDocument::parse(
            "<p>before <span id=atom><span class=t><span class=tb><span class=row><span class=cell id=first>one</span><span class=cell>two</span><span class=cell>three</span></span></span></span></span> after</p>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "#atom { display: inline-block; } .t { display: table; table-layout: fixed; width: 300px; border-spacing: 0; } .tb { display: table-row-group; } .row { display: table-row; } .cell { display: table-cell; } #first { width: 120px; }",
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
        let ledger = layout.table_shadow_ledger();
        assert!(
            ledger.assigned >= 1,
            "the atomic subtree's table was not sized by Buckram: {ledger:?}"
        );
        assert!(ledger.is_silent(), "{ledger:?}");
    }

    /// K4d6b: a `height` on a `<tr>` reaches the painted rows.
    ///
    /// It is a row minimum under CSS 2.1 section 17.5.3. The Grid bridge
    /// flattened rows away before the backend saw them, so that declaration
    /// reached no track and the table painted content-height rows: 18 and 19
    /// for these two. Buckram computes 40 and 60, and now writes them.
    ///
    /// The painted rectangles are asserted directly rather than through the
    /// ledger. A ledger that agreed with itself would prove nothing.
    #[test]
    fn k4d6b_row_heights_reach_the_painted_rows() {
        let dom = StaticDocument::parse(
            "<table><tbody><tr id=a><td>one</td><td>two</td></tr><tr id=b><td>three</td><td>four</td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; table-layout: fixed; width: 200px; border-spacing: 0; }                  tbody { display: table-row-group; } tr { display: table-row; }                  td { display: table-cell; padding: 0; } #a { height: 40px; } #b { height: 60px; }",
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
        let ledger = layout.table_shadow_ledger();
        assert_eq!(
            ledger.block.laid_out, 1,
            "Buckram did not lay out the table's block axis: {:?}",
            ledger.block
        );
        assert_eq!(
            ledger.block.agreed, 1,
            "the painted cells must now be the ones Buckram wrote: {:?}",
            ledger.block.divergences
        );

        fn cell_rect(
            dom: &StaticDocument,
            layout: &LiveryLayout<<StaticDocument as LayoutDom>::NodeId>,
            index: usize,
        ) -> PhysicalRect {
            fn cells(
                dom: &StaticDocument,
                node: <StaticDocument as LayoutDom>::NodeId,
                found: &mut Vec<<StaticDocument as LayoutDom>::NodeId>,
            ) {
                if dom
                    .element_name(node)
                    .is_some_and(|name| name.local.as_ref() == "td")
                {
                    found.push(node);
                }
                for child in dom.dom_children(node) {
                    cells(dom, child, found);
                }
            }
            let mut found = Vec::new();
            cells(dom, dom.document(), &mut found);
            let box_id = layout.boxes().boxes_for_node(found[index])[0];
            layout
                .fragments()
                .fragments_for_box(box_id)
                .next()
                .expect("cell fragment")
                .physical_rect()
        }
        // The first cell of each row, in document order.
        let first = cell_rect(&dom, &layout, 0);
        let second = cell_rect(&dom, &layout, 2);
        assert!(
            (first.height - 40.0).abs() < 0.5,
            "the first row's cell must be its row's 40px, not its content              height: {first:?}"
        );
        assert!(
            (second.height - 60.0).abs() < 0.5,
            "the second row's cell must be its row's 60px: {second:?}"
        );
        assert!(
            (second.y - first.y - 40.0).abs() < 0.5,
            "the second row must start just below a 40px first row:              {first:?} {second:?}"
        );
    }

    /// Lay out one document and return every table-role box's rectangle.
    fn table_boxes(html: &str, css: &str) -> Vec<(InternalTableRole, PhysicalRect)> {
        let dom = StaticDocument::parse(html);
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }", css]),
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
        layout
            .boxes()
            .iter()
            .filter_map(|(box_id, css_box)| {
                Some((
                    css_box.display.internal_table?,
                    layout
                        .fragments()
                        .fragments_for_box(box_id)
                        .next()?
                        .physical_rect(),
                ))
            })
            .collect()
    }

    /// Every rectangle laid out for one table role, in tree order.
    fn table_role_rects(html: &str, css: &str, role: InternalTableRole) -> Vec<PhysicalRect> {
        table_boxes(html, css)
            .into_iter()
            .filter(|(each, _)| *each == role)
            .map(|(_, rect)| rect)
            .collect()
    }

    /// The table wrapper box and the table grid box, in that order.
    fn table_wrapper_and_grid(html: &str, css: &str) -> (PhysicalRect, PhysicalRect) {
        let one = |role| {
            *table_role_rects(html, css, role)
                .first()
                .unwrap_or_else(|| panic!("no fragment for {role:?}"))
        };
        (
            one(InternalTableRole::Wrapper),
            one(InternalTableRole::Grid),
        )
    }

    /// K4e1: the wrapper and the grid are two boxes that split one element.
    ///
    /// CSS 2.1 section 17.4 uses `margin-*` on the wrapper and leaves `width`,
    /// `border`, and `padding` on the grid. Both are observable at once: the
    /// margin has to move the wrapper, and the grid's own border box has to
    /// still contain the border and padding the wrapper does not have. CSS
    /// Tables 3 section 2.2.1 then makes the two the same width, because the
    /// wrapper's width *is* the grid's border-edge width.
    #[test]
    fn k4e1_the_wrapper_takes_the_margin_and_the_grid_keeps_its_own_box() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host><table><tr><td></td></tr></table></div>",
            "#host { width: 300px; }\
             table { display: table; box-sizing: content-box; width: 100px;\
                     margin-left: 20px; border: 5px solid; padding: 3px;\
                     border-spacing: 0; }\
             tr { display: table-row; } td { display: table-cell; padding: 0; }",
        );

        // The margin is the wrapper's, so both boxes start past it.
        assert!((wrapper.x - 20.0).abs() < 0.5, "wrapper: {wrapper:?}");
        assert!((grid.x - 20.0).abs() < 0.5, "grid: {grid:?}");
        // The grid's border edge is 100 of content plus its own padding and
        // border, and the wrapper is exactly that wide - no wider, which is
        // what would happen if it had kept the border and padding too.
        assert!((grid.width - 116.0).abs() < 0.5, "grid: {grid:?}");
        assert!(
            (wrapper.width - grid.width).abs() < 0.5,
            "the wrapper is the grid's border-edge width: {wrapper:?} {grid:?}"
        );
    }

    /// K4e1: `position` is the wrapper's, and a percentage size skips it.
    ///
    /// CSS 2.1 section 17.4 again: "Percentages on 'width' and 'height' on the
    /// table are relative to the table wrapper box's containing block, not the
    /// table wrapper box itself." Without that rule the grid's `50%` resolves
    /// against a wrapper that is itself waiting on the grid, and the pair
    /// collapses to zero - which is what `absolute-tables-012` measures.
    #[test]
    fn k4e1_a_percentage_table_skips_the_wrapper_it_would_otherwise_wait_on() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host><table></table></div>",
            "#host { position: relative; width: 200px; }\
             table { display: table; position: absolute; width: 50%; height: 100px;\
                     table-layout: fixed; border-spacing: 0; }",
        );

        for (name, rect) in [("wrapper", wrapper), ("grid", grid)] {
            assert!((rect.width - 100.0).abs() < 0.5, "{name}: {rect:?}");
            assert!((rect.height - 100.0).abs() < 0.5, "{name}: {rect:?}");
        }
    }

    /// K4e1: a table flex item is the wrapper, and it does not stretch.
    ///
    /// Inserting the wrapper makes it, not the grid, the flex item. CSS
    /// Tables 3 section 2.2.1 keeps its width the grid's, so a column flex
    /// container's default `align-items: stretch` has nothing to stretch -
    /// the width is not `auto`. `table-as-item-cell-percentage-002` fails the
    /// moment the wrapper widens to the container instead.
    #[test]
    fn k4e1_a_table_flex_item_is_the_wrapper_and_keeps_the_grids_width() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host><table><tr><td></td></tr></table></div>",
            "#host { display: flex; flex-direction: column; width: 300px; }\
             table { display: table; width: 100px; height: 100px; border-spacing: 0; }\
             tr { display: table-row; } td { display: table-cell; padding: 0; }",
        );

        assert!((wrapper.width - 100.0).abs() < 0.5, "wrapper: {wrapper:?}");
        assert!((grid.width - 100.0).abs() < 0.5, "grid: {grid:?}");
    }

    /// K4e2: an `auto`-width wrapper measures the grid rather than filling.
    ///
    /// This is the half of CSS Tables 3 section 2.2.1 that cannot be computed
    /// before layout. An ordinary block with `width: auto` would take all 300
    /// of the container; the wrapper takes the 80 its two columns come to,
    /// through Buckram's intrinsic shrink-to-fit lane rather than through the
    /// `float: left` that used to stand in for it.
    #[test]
    fn k4e2_an_auto_width_wrapper_measures_the_grid_instead_of_filling() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host><table><tr><td></td><td></td></tr></table></div>",
            "#host { width: 300px; }\
             table { display: table; border-spacing: 0; }\
             tr { display: table-row; }\
             td { display: table-cell; padding: 0; width: 40px; height: 10px; }",
        );

        assert!((grid.width - 80.0).abs() < 0.5, "grid: {grid:?}");
        assert!((wrapper.width - 80.0).abs() < 0.5, "wrapper: {wrapper:?}");
    }

    /// K4e2: auto margins centre a table, which a float cannot do.
    ///
    /// The margins are the wrapper's under CSS 2.1 section 17.4, and a float
    /// resolves an `auto` margin to zero. Once the wrapper is an in-flow
    /// shrink-to-fit block on K3's equations, `margin: 0 auto` on a table
    /// centres it the way it does on any other block.
    #[test]
    fn k4e2_auto_margins_centre_a_table() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host><table><tr><td></td></tr></table></div>",
            "#host { width: 300px; }\
             table { display: table; border-spacing: 0; width: 100px;\
                     margin-left: auto; margin-right: auto; }\
             tr { display: table-row; }\
             td { display: table-cell; padding: 0; width: 100px; height: 10px; }",
        );

        assert!((wrapper.width - 100.0).abs() < 0.5, "wrapper: {wrapper:?}");
        assert!((wrapper.x - 100.0).abs() < 0.5, "wrapper: {wrapper:?}");
        assert!((grid.x - 100.0).abs() < 0.5, "grid: {grid:?}");
    }

    /// K4e3: a captioned table stops deferring.
    ///
    /// The point of the gate. `CaptionMinContribution::PendingK4e` was a named
    /// gap rather than a defect, and the 2026-08-03 census counted it firing
    /// 17 times; a measured caption closes it, and Buckram sizes the table
    /// instead of declining it.
    #[test]
    fn k4e3_a_captioned_table_no_longer_defers() {
        let dom = StaticDocument::parse(
            "<div id=host><table><caption>a caption</caption>\
             <tr><td>one</td><td>two</td></tr></table></div>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["#host { width: 400px; } \
                 table { display: table; border-spacing: 0; } \
                 caption { display: table-caption; margin: 0; padding: 0; } \
                 tr { display: table-row; } td { display: table-cell; padding: 0; }"]),
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

        let ledger = layout.table_shadow_ledger();
        assert_eq!(
            ledger.deferral_count(buckram::TableDeferral::CaptionMinPendingK4e),
            0,
            "a measured caption must not defer: {ledger:?}"
        );
        assert_assigned_and_honored(ledger);
    }

    /// K4f: `visibility: collapse` removes a row's space, not just its ink.
    ///
    /// CSS 2.1 section 17.5.5 reduces the table's height by exactly what the
    /// collapsed row occupied and leaves the other rows the heights they were
    /// given. Three 20px rows come to 60; collapsing the middle one leaves 40,
    /// with the third row moved up into the gap rather than a hole left where
    /// the second used to be.
    #[test]
    fn k4f_a_collapsed_row_gives_its_space_back_to_the_table() {
        let html = "<div id=host><table><tr id=a><td></td></tr>\
                    <tr id=b><td></td></tr><tr id=c><td></td></tr></table></div>";
        let visible = "#host { width: 300px; }\
                       table { display: table; border-spacing: 0; }\
                       tr { display: table-row; }\
                       td { display: table-cell; padding: 0; width: 40px; height: 20px; }";
        let collapsed = format!("{visible} #b {{ visibility: collapse; }}");

        let (_, before) = table_wrapper_and_grid(html, visible);
        let (_, after) = table_wrapper_and_grid(html, &collapsed);
        let rows = table_role_rects(html, &collapsed, InternalTableRole::Row);

        assert!((before.height - 60.0).abs() < 0.5, "before: {before:?}");
        assert!((after.height - 40.0).abs() < 0.5, "after: {after:?}");
        assert!(
            (rows[1].height - 0.0).abs() < 0.5,
            "collapsed: {:?}",
            rows[1]
        );
        assert!(
            (rows[2].y - rows[0].y - 20.0).abs() < 0.5,
            "the third row closes the gap: {:?} {:?}",
            rows[0],
            rows[2]
        );
    }

    /// K4f: a collapsed column gives its width back the same way.
    ///
    /// Applied through the column group here, which section 17.5.5 also
    /// allows: a collapsed `<colgroup>` collapses every column in its range.
    #[test]
    fn k4f_a_collapsed_column_group_gives_its_width_back() {
        let html = "<div id=host><table><colgroup id=g><col></colgroup>\
                    <colgroup><col></colgroup>\
                    <tr><td></td><td></td></tr></table></div>";
        let visible = "#host { width: 300px; }\
                       table { display: table; border-spacing: 0; }\
                       colgroup { display: table-column-group; }\
                       col { display: table-column; }\
                       tr { display: table-row; }\
                       td { display: table-cell; padding: 0; width: 40px; height: 20px; }";
        let collapsed = format!("{visible} #g {{ visibility: collapse; }}");

        let (_, before) = table_wrapper_and_grid(html, visible);
        let (wrapper, after) = table_wrapper_and_grid(html, &collapsed);

        assert!((before.width - 80.0).abs() < 0.5, "before: {before:?}");
        assert!((after.width - 40.0).abs() < 0.5, "after: {after:?}");
        // K4e2's rule still holds through the collapse.
        assert!((wrapper.width - after.width).abs() < 0.5, "{wrapper:?}");
    }

    /// K4e4: used `width` and `height` answer from the grid, not the wrapper.
    ///
    /// The `height` property stayed on the grid under CSS 2.1 section 17.4,
    /// so `getComputedStyle(table).height` reports the grid's border box - the
    /// 40px of rows, not the 70px wrapper that also contains the caption.
    #[test]
    fn k4e4_used_height_of_a_captioned_table_is_the_grids() {
        let dom =
            StaticDocument::parse("<table><caption>above</caption><tr><td>one</td></tr></table>");
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; table-layout: fixed; width: 200px; border-spacing: 0; } \
                 caption { display: table-caption; height: 30px; margin: 0; padding: 0; } \
                 tr { display: table-row; height: 40px; } \
                 td { display: table-cell; padding: 0; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let table = {
            fn find(
                dom: &StaticDocument,
                node: <StaticDocument as LayoutDom>::NodeId,
            ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
                if dom
                    .element_name(node)
                    .is_some_and(|name| name.local.as_ref() == "table")
                {
                    return Some(node);
                }
                dom.dom_children(node)
                    .into_iter()
                    .find_map(|child| find(dom, child))
            }
            find(&dom, dom.document()).expect("the table exists")
        };
        let used = used_value_context(&dom, &styles, 320.0, 240.0, table)
            .expect("layout")
            .expect("the table has a fragment");

        assert!((used.border_box.0 - 200.0).abs() < 0.5, "{used:?}");
        assert!(
            (used.border_box.1 - 40.0).abs() < 0.5,
            "the used height is the grid's, without the caption: {used:?}"
        );
    }

    /// K4e4: an inline-table occupies line space as an atom.
    ///
    /// Deleting K4a's wrapper/grid exclusion from `box_is_inline` lets the
    /// wrapper join the inline group, and the atomic-inline lane lays its
    /// subtree out separately - the same route an inline-block rides. The
    /// receipt is placement: the table sits to the right of the text that
    /// precedes it in the same line, at the grid's own width, instead of
    /// dropping to a line of its own as the block it used to be built as.
    #[test]
    fn k4e4_an_inline_table_sits_in_the_text_line() {
        let (wrapper, grid) = table_wrapper_and_grid(
            "<div id=host>before<span class=t><span class=r>\
             <span class=c>cell</span></span></span></div>",
            "#host { width: 300px; font-family: monospace; font-size: 10px;\
                     line-height: 20px; }\
             .t { display: inline-table; border-spacing: 0; }\
             .r { display: table-row; }\
             .c { display: table-cell; padding: 0; width: 50px; height: 12px; }",
        );

        assert!((grid.width - 50.0).abs() < 0.5, "grid: {grid:?}");
        assert!(
            (wrapper.width - grid.width).abs() < 0.5,
            "{wrapper:?} {grid:?}"
        );
        // In the line, after the word, not below it.
        assert!(
            wrapper.x > 30.0,
            "the atom must sit after 'before': {wrapper:?}"
        );
        assert!(
            wrapper.y < 20.0,
            "the atom must sit in the first line: {wrapper:?}"
        );
    }

    /// K4e3: a caption wider than the table widens the *grid*.
    ///
    /// The two engines break CSS Tables 3 section 2.2.1 apart here, measured
    /// in the K4e1 interop matrix: Chrome grows the grid and its columns to
    /// the caption, Firefox leaves the grid at its own content width and lets
    /// only the wrapper be caption-wide. Section 2.2.1 says the wrapper's
    /// width *is* the grid's border-edge width, which Firefox's answer
    /// contradicts and Chrome's keeps, so this keeps the rule.
    ///
    /// C7 of that matrix is the sharpest case to assert, because a specified
    /// caption width fixes the expected number without depending on font
    /// metrics: the caption's 300 reaches the single column.
    #[test]
    fn k4e3_a_caption_widens_the_grid_and_its_columns() {
        let html = "<div id=host><table><caption>x</caption>\
                    <tr><td></td></tr></table></div>";
        let css = "#host { width: 400px; }\
                   table { display: table; border-spacing: 0; }\
                   caption { display: table-caption; width: 300px; margin: 0; padding: 0; }\
                   tr { display: table-row; }\
                   td { display: table-cell; padding: 0; height: 10px; }";
        let (wrapper, grid) = table_wrapper_and_grid(html, css);
        let cells = table_role_rects(html, css, InternalTableRole::Cell);

        assert!((grid.width - 300.0).abs() < 0.5, "grid: {grid:?}");
        assert!((cells[0].width - 300.0).abs() < 0.5, "cell: {:?}", cells[0]);
        // Section 2.2.1 still holds: the wrapper is the grid's width.
        assert!(
            (wrapper.width - grid.width).abs() < 0.5,
            "{wrapper:?} {grid:?}"
        );
    }

    /// K4e3: a caption's own margins are part of the floor it puts down.
    ///
    /// C5 of the interop matrix, where both engines agree: a 176-wide caption
    /// with `margin-left: 30px` contributes 206. Asserted with a specified
    /// width so the number does not depend on font metrics.
    #[test]
    fn k4e3_a_captions_margins_count_toward_what_it_contributes() {
        let html = "<div id=host><table><caption>x</caption>\
                    <tr><td></td></tr></table></div>";
        let css = "#host { width: 400px; }\
                   table { display: table; border-spacing: 0; }\
                   caption { display: table-caption; width: 200px; margin-left: 30px;\
                             padding: 0; }\
                   tr { display: table-row; }\
                   td { display: table-cell; padding: 0; height: 10px; }";
        let (_, grid) = table_wrapper_and_grid(html, css);

        assert!((grid.width - 230.0).abs() < 0.5, "grid: {grid:?}");
    }

    /// K4e3: `caption-side` decides which side of the grid a caption lands on.
    ///
    /// CSS 2.1 section 17.4.1 lays a caption above or below the grid inside
    /// the wrapper's margins. Buckram's box tree keeps every caption before
    /// the grid, so a bottom caption has to be reordered on the way into
    /// layout - and C4 of the matrix pins that the side does not change what
    /// the caption contributes to sizing.
    #[test]
    fn k4e3_caption_side_moves_the_caption_without_changing_the_table() {
        let html = "<div id=host><table><caption>x</caption>\
                    <tr><td></td></tr></table></div>";
        let above = "#host { width: 400px; }\
                     table { display: table; border-spacing: 0; }\
                     caption { display: table-caption; width: 300px; height: 20px;\
                               margin: 0; padding: 0; }\
                     tr { display: table-row; }\
                     td { display: table-cell; padding: 0; height: 10px; }";
        let below = format!("{above} caption {{ caption-side: bottom; }}");

        let (top_wrapper, top_grid) = table_wrapper_and_grid(html, above);
        let top_caption = table_role_rects(html, above, InternalTableRole::Caption)[0];
        let (bottom_wrapper, bottom_grid) = table_wrapper_and_grid(html, &below);
        let bottom_caption = table_role_rects(html, &below, InternalTableRole::Caption)[0];

        assert!(
            top_caption.y < top_grid.y,
            "a top caption sits above the grid: {top_caption:?} {top_grid:?}"
        );
        assert!(
            bottom_caption.y > bottom_grid.y,
            "a bottom caption sits below the grid: {bottom_caption:?} {bottom_grid:?}"
        );
        // The side is placement only; the sizing it forces is the same.
        assert!((top_grid.width - bottom_grid.width).abs() < 0.5);
        assert!((top_wrapper.height - bottom_wrapper.height).abs() < 0.5);
    }

    /// K4d6: a table row now has a fragment of its own.
    ///
    /// The Grid bridge flattened rows, row groups, and columns away before
    /// the backend saw them, so none of them had any box to paint into: a
    /// `<tr>` background could not render at all. Buckram emits the whole
    /// structural subtree from the track model, so each one gets its exact
    /// rectangle whether or not a cell happens to cover it.
    #[test]
    fn k4d6_rows_groups_and_columns_have_their_own_fragments() {
        let dom = StaticDocument::parse(
            "<table><colgroup><col></colgroup><tbody><tr id=a><td>one</td></tr><tr id=b><td>two</td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; table-layout: fixed; width: 200px; border-spacing: 0; }                  tbody { display: table-row-group; } tr { display: table-row; }                  td { display: table-cell; padding: 0; } #a { height: 40px; } #b { height: 60px; }",
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

        let rect_of = |local: &str, nth: usize| {
            fn walk(
                dom: &StaticDocument,
                node: <StaticDocument as LayoutDom>::NodeId,
                local: &str,
                out: &mut Vec<<StaticDocument as LayoutDom>::NodeId>,
            ) {
                if dom
                    .element_name(node)
                    .is_some_and(|n| n.local.as_ref() == local)
                {
                    out.push(node);
                }
                for child in dom.dom_children(node) {
                    walk(dom, child, local, out);
                }
            }
            let mut found = Vec::new();
            walk(&dom, dom.document(), local, &mut found);
            // A table element generates a wrapper and a grid; only one of
            // them carries the fragment.
            layout
                .boxes()
                .boxes_for_node(found[nth])
                .iter()
                .find_map(|box_id| layout.fragments().fragments_for_box(*box_id).next())
                .unwrap_or_else(|| panic!("no fragment for {local}[{nth}]"))
                .physical_rect()
        };

        let table = rect_of("table", 0);
        let first = rect_of("tr", 0);
        let second = rect_of("tr", 1);
        let group = rect_of("tbody", 0);
        let column = rect_of("col", 0);

        // Each row spans the grid and holds exactly its own track.
        assert!((first.height - 40.0).abs() < 0.5, "first row: {first:?}");
        assert!((second.height - 60.0).abs() < 0.5, "second row: {second:?}");
        assert!(
            (second.y - first.y - 40.0).abs() < 0.5,
            "rows must tile: {first:?} {second:?}"
        );
        assert!((first.width - 200.0).abs() < 0.5, "{first:?}");

        // A group's rectangle is the exact union of its track range, not a
        // box reconstructed from the cells inside it.
        assert!((group.y - first.y).abs() < 0.5, "{group:?}");
        assert!((group.height - 100.0).abs() < 0.5, "{group:?}");

        // A column runs the table's whole block extent.
        assert!((column.height - table.height).abs() < 0.5, "{column:?}");
        assert!((column.width - 200.0).abs() < 0.5, "{column:?}");
    }

    /// K4d4c: `min-height` and `max-height` do not reach a table cell.
    ///
    /// CSS 2.1 section 10.7 leaves their effect on table cells, rows, and row
    /// groups undefined, and Chrome 150 and Firefox 153 both ignore them
    /// outright in all eight measured cases. So a cell carrying them is
    /// ordinary work rather than a deferral, and a 100px child keeps its
    /// 100px row against a `max-height: 20px` cell.
    #[test]
    fn k4d4c_cell_min_and_max_height_are_ignored() {
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td><div class=tall></div></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; table-layout: fixed; width: 100px; border-spacing: 0; }                  tbody { display: table-row-group; } tr { display: table-row; }                  td { display: table-cell; padding: 0; height: 20px; max-height: 20px;                  min-height: 5px; } .tall { height: 100px; }",
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
        let ledger = layout.table_shadow_ledger();
        assert_eq!(
            ledger.block.laid_out, 1,
            "a cell max-height must not defer the table: {:?}",
            ledger.block
        );
        assert!(
            ledger.block.skipped.is_empty(),
            "{:?}",
            ledger.block.skipped
        );
        assert_eq!(
            ledger.block.agreed, 1,
            "the painted cell must be the one Buckram wrote: {:?}",
            ledger.block.divergences
        );
    }

    fn automatic_table_ledger(table_css: &str) -> crate::table_shadow::TableShadowLedger {
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td>one</td><td>one</td><td>one</td></tr></tbody></table>",
        );
        let css = format!(
            "table {{ display: table; border-spacing: 0; {table_css} }} tbody {{ display: table-row-group; }} tr {{ display: table-row; }} td {{ display: table-cell; padding: 0; }}"
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[&css]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        layout(&dom, &styles, 320.0, 240.0)
            .expect("layout")
            .table_shadow_ledger()
            .clone()
    }

    /// K4c5b: a shrink-to-fit automatic table is sized by Buckram from cell
    /// intrinsics measured through the live machinery before the main pass.
    #[test]
    fn k4c5b_automatic_shrink_to_fit_is_buckram_owned() {
        assert_assigned_and_honored(&automatic_table_ledger(""));
    }

    /// The second K4c5a divergence, resolved by authority: an automatic table
    /// explicitly wider than its max-content distributes the extra space over
    /// its columns (CSS 2.1 17.5.2.2). Taffy inference left the tracks at
    /// max-content; Buckram's 100px columns now paint, verified against the
    /// fragments.
    #[test]
    fn k4c5b_automatic_explicit_width_is_distributed_and_painted() {
        assert_assigned_and_honored(&automatic_table_ledger("width: 300px;"));
    }

    /// K4c5b on the production text path: automatic tables there previously
    /// could not even be shadowed; they are now sized by Buckram.
    #[test]
    fn k4c5b_text_path_automatic_tables_are_buckram_owned() {
        let dom = StaticDocument::parse(
            "<p>before</p><table><tbody><tr><td>one</td><td>two</td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; border-spacing: 0; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; padding: 0; }",
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
        let ledger = layout.table_shadow_ledger();
        assert!(
            ledger.assigned >= 1,
            "the text path's automatic table was not sized by Buckram: {ledger:?}"
        );
        assert!(ledger.is_silent(), "{ledger:?}");
    }

    /// Regression for the K4a/K4b-window crash in WPT
    /// `css/CSS2/css21-errata/s-11-1-1b-005.html`, run byte-exact from the
    /// in-repo corpus: the root element styled `display: table` with a bare
    /// `table-cell` `<body>` whose `margin-top: -15px` places its baseline
    /// above the parent's block-start edge. Baseline propagation asserted
    /// offsets were non-negative and panicked on the legitimate negative one.
    #[test]
    fn a_root_element_table_with_a_negative_margin_cell_does_not_panic() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/wpt/tests/css/CSS2/css21-errata/s-11-1-1b-005.html"
        ))
        .expect("in-repo WPT corpus file");
        let style_start = source.find("<style>").expect("style open") + "<style>".len();
        let style_end = source.find("</style>").expect("style close");
        let css = source[style_start..style_end].to_owned();
        let dom = StaticDocument::parse(&source);
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[&css]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let mut text = TextSystem::new();
        layout_with_text_system(
            &dom,
            &styles,
            800.0,
            600.0,
            ViewportSizes::uniform(800.0, 600.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("layout must not panic");
    }

    /// K4d1 adapter fixture: a `TableCellFormatter` over the live algorithm
    /// tree formats block and flex cell contents at the exact inline size the
    /// table algorithm supplies. The scratch tree contains only cell
    /// subtrees, so the table and its row structurally record no backend
    /// call, while the flex cell dispatches through its own algorithm; the
    /// old bridge's table-as-Grid node does not exist here at all.
    #[test]
    fn k4d1_cell_formatter_formats_contents_at_exact_inline_sizes() {
        use buckram::{
            FragmentDraft, FragmentDraftTree, TableCellFormatter, TableCellLayoutInput,
            TableCellLayoutOutput, TableRowLayoutError,
        };

        struct TreeFormatter<'a> {
            tree: &'a mut AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
            nodes: HashMap<BoxId, AlgorithmNodeId>,
            formatted: Vec<(BoxId, f32)>,
        }

        impl TableCellFormatter for TreeFormatter<'_> {
            fn format_cell(
                &mut self,
                input: TableCellLayoutInput,
            ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
                let node = *self.nodes.get(&input.box_id).ok_or(
                    TableRowLayoutError::InvalidCellOutput {
                        box_id: input.box_id,
                    },
                )?;
                self.tree.compute_layout_with_measure(
                    node,
                    AlgorithmSize::new(
                        AlgorithmAvailableSpace::Definite(input.content_inline_size),
                        AlgorithmAvailableSpace::MaxContent,
                    ),
                    |known, _, _, context, _| {
                        let Some(context) = context else {
                            return AlgorithmSize::new(0.0, 0.0);
                        };
                        AlgorithmSize::new(
                            known.width.unwrap_or(context.max_width),
                            known.height.unwrap_or(context.height),
                        )
                    },
                );
                let layout = self.tree.layout(node);
                self.formatted
                    .push((input.box_id, input.content_inline_size));
                let mut fragments = FragmentDraftTree::default();
                fragments.push(FragmentDraft {
                    box_id: input.box_id,
                    logical_rect: buckram::LogicalRect::default(),
                    overflow: buckram::LogicalRect::default(),
                    parent: None,
                });
                Ok(TableCellLayoutOutput {
                    content_block_size: layout.height,
                    border_box_min_block_size: layout.height,
                    // K4d5 owns real cell baselines; the contract placeholder
                    // synthesizes from the block end.
                    baselines: buckram::Baselines::synthesized_from_block_end(layout.height),
                    overflow: buckram::LogicalRect::default(),
                    fragments,
                })
            }
        }

        // A real grid from a two-cell table document; the algorithm tree gets
        // one block cell and one flex cell, and nothing else.
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td id=blocky>x</td><td id=flexy><i>a</i><i>b</i></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; border-spacing: 0; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; padding: 0; } #flexy { display: table-cell; } #flexy i { display: block; height: 7px; }",
            ]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = boxes
            .iter()
            .find_map(|(box_id, css_box)| {
                (css_box.display.internal_table == Some(buckram::InternalTableRole::Grid))
                    .then_some(box_id)
            })
            .expect("table grid box");
        let grid = build_table_grid(&boxes, &dom, table);
        assert_eq!(grid.cells.len(), 2);

        let mut tree: AlgorithmTree<Style, TextMeasure, Option<BoxId>> = AlgorithmTree::new();
        let mut nodes = HashMap::new();
        for cell in &grid.cells {
            let kind = if nodes.is_empty() {
                AlgorithmKind::Block
            } else {
                AlgorithmKind::Flex
            };
            let children = if kind == AlgorithmKind::Flex {
                vec![tree.new_with_children(
                    AlgorithmKind::Block,
                    Style {
                        size: Size {
                            width: Dimension::auto(),
                            height: Dimension::length(7.0),
                        },
                        ..Style::default()
                    },
                    &[],
                    None,
                )]
            } else {
                Vec::new()
            };
            let node = tree.new_with_children(kind, Style::default(), &children, None);
            nodes.insert(cell.source, node);
        }
        let inline = {
            let sizing = buckram::TableInlineSizingInput {
                grid: &grid,
                available_inline_size: Some(150.0),
                table_constraints: buckram::TableInlineConstraints::default(),
                border_metrics: buckram::TableInlineBorderMetrics::Separated(
                    buckram::TableSeparatedBorderMetrics::default(),
                ),
                caption_min: buckram::CaptionMinContribution::NoCaption,
                track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
            };
            buckram::TableInlineSizingResult::new(
                &sizing,
                buckram::IntrinsicSizes::new(150.0, 150.0).expect("intrinsic pair"),
                150.0,
                150.0,
                vec![90.0, 60.0],
            )
            .expect("reconciled inline result")
        };
        let input = buckram::TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: buckram::TableBlockConstraint::Auto,
            table_box_sizing: buckram::TableBoxSizing::BorderBox,
            border_metrics: buckram::TableBlockBorderMetrics::Separated(
                buckram::TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
        };
        let mut formatter = TreeFormatter {
            tree: &mut tree,
            nodes,
            formatted: Vec::new(),
        };
        let outputs = buckram::format_table_cells(&input, 0.0, |_, _| 0.0, &mut formatter)
            .expect("formatted cells");

        assert_eq!(outputs.len(), 2);
        // Exact K4c column sizes reached the formatter.
        let widths = formatter
            .formatted
            .iter()
            .map(|(_, width)| *width)
            .collect::<Vec<_>>();
        assert_eq!(widths, vec![90.0, 60.0]);
        // The tree holds only cell subtrees: no node represents the table or
        // the row, so neither can have recorded a backend call.
        assert_eq!(tree.node_ids().count(), 3);
        assert!(
            tree.node_ids()
                .all(|id| tree.kind(id) != AlgorithmKind::Grid),
            "no table-as-Grid node may exist in the K4d dispatch shape"
        );
    }

    /// K4d2 adapter fixture: real cell contents are formatted at exact K4c
    /// inline sizes with an indefinite first-pass block size, and the row
    /// minima that follow come from those measured contents. The taller row
    /// is taller because its content is, not because anything was assumed.
    #[test]
    fn k4d2_row_minima_follow_from_formatted_cell_contents() {
        use buckram::{
            FragmentDraftTree, TableCellBlockStyle, TableCellFormatter, TableCellLayoutInput,
            TableCellLayoutOutput, TableCellLayoutPass, TableRowLayoutError,
        };

        struct TreeFormatter<'a> {
            tree: &'a mut AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
            nodes: HashMap<BoxId, AlgorithmNodeId>,
            requests: Vec<TableCellLayoutInput>,
        }

        impl TableCellFormatter for TreeFormatter<'_> {
            fn format_cell(
                &mut self,
                input: TableCellLayoutInput,
            ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
                self.requests.push(input);
                let node = *self.nodes.get(&input.box_id).ok_or(
                    TableRowLayoutError::InvalidCellOutput {
                        box_id: input.box_id,
                    },
                )?;
                self.tree.compute_layout_with_measure(
                    node,
                    AlgorithmSize::new(
                        AlgorithmAvailableSpace::Definite(input.content_inline_size),
                        // The first pass is deliberately indefinite in the
                        // block axis: a cell height must not stretch its
                        // content formatting context.
                        AlgorithmAvailableSpace::MaxContent,
                    ),
                    |known, _, _, context, _| {
                        let Some(context) = context else {
                            return AlgorithmSize::new(0.0, 0.0);
                        };
                        AlgorithmSize::new(
                            known.width.unwrap_or(context.max_width),
                            known.height.unwrap_or(context.height),
                        )
                    },
                );
                let layout = self.tree.layout(node);
                Ok(TableCellLayoutOutput {
                    content_block_size: layout.height,
                    border_box_min_block_size: 0.0,
                    baselines: buckram::Baselines::synthesized_from_block_end(layout.height),
                    overflow: buckram::LogicalRect::default(),
                    fragments: FragmentDraftTree::default(),
                })
            }
        }

        // Row 0's cell is three stacked 9px blocks; row 1's is one.
        let dom = StaticDocument::parse(
            "<table><tbody><tr><td id=tall><i></i><i></i><i></i></td></tr><tr><td id=short><i></i></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; border-spacing: 0; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; padding: 0; } i { display: block; height: 9px; }",
            ]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = boxes
            .iter()
            .find_map(|(box_id, css_box)| {
                (css_box.display.internal_table == Some(buckram::InternalTableRole::Grid))
                    .then_some(box_id)
            })
            .expect("table grid box");
        let grid = build_table_grid(&boxes, &dom, table);
        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.cells.len(), 2);

        let mut tree: AlgorithmTree<Style, TextMeasure, Option<BoxId>> = AlgorithmTree::new();
        let mut nodes = HashMap::new();
        for (index, cell) in grid.cells.iter().enumerate() {
            let blocks = (0..if index == 0 { 3 } else { 1 })
                .map(|_| {
                    tree.new_with_children_and_block_style(
                        AlgorithmKind::Block,
                        BlockStyle {
                            size: BlockDimensions::new(
                                BlockSizeValue::Auto,
                                BlockSizeValue::Length(FlowLength::px(9.0)),
                            ),
                            ..BlockStyle::default()
                        },
                        Style {
                            size: Size {
                                width: Dimension::auto(),
                                height: Dimension::length(9.0),
                            },
                            ..Style::default()
                        },
                        &[],
                        None,
                    )
                })
                .collect::<Vec<_>>();
            nodes.insert(
                cell.source,
                tree.new_with_children_and_block_style(
                    AlgorithmKind::Block,
                    BlockStyle::default(),
                    Style::default(),
                    &blocks,
                    None,
                ),
            );
        }

        let inline = {
            let sizing = buckram::TableInlineSizingInput {
                grid: &grid,
                available_inline_size: Some(80.0),
                table_constraints: buckram::TableInlineConstraints::default(),
                border_metrics: buckram::TableInlineBorderMetrics::Separated(
                    buckram::TableSeparatedBorderMetrics::default(),
                ),
                caption_min: buckram::CaptionMinContribution::NoCaption,
                track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
            };
            buckram::TableInlineSizingResult::new(
                &sizing,
                buckram::IntrinsicSizes::new(80.0, 80.0).expect("intrinsic pair"),
                80.0,
                80.0,
                vec![80.0],
            )
            .expect("reconciled inline result")
        };
        let input = buckram::TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: buckram::TableBlockConstraint::Auto,
            table_box_sizing: buckram::TableBoxSizing::BorderBox,
            border_metrics: buckram::TableBlockBorderMetrics::Separated(
                buckram::TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
        };
        let mut formatter = TreeFormatter {
            tree: &mut tree,
            nodes,
            requests: Vec::new(),
        };
        let outputs = buckram::format_table_cells(&input, 0.0, |_, _| 0.0, &mut formatter)
            .expect("formatted cells");

        // Every first-pass request carried the exact column size and an
        // indefinite block size.
        assert!(formatter.requests.iter().all(|request| {
            request.content_inline_size == 80.0
                && request.available_block_size.is_none()
                && request.percentage_basis.is_none()
                && request.pass == TableCellLayoutPass::Measure
        }));

        let rows = buckram::measure_single_span_rows(
            &input,
            &[TableCellBlockStyle::default(); 2],
            &outputs,
            &[buckram::TableBlockConstraint::Auto; 2],
        )
        .expect("row measures");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].min_block_size, 27.0);
        assert_eq!(rows[1].min_block_size, 9.0);
        assert!(rows.iter().all(|row| !row.constrained && row.row.is_some()));
    }

    /// K4d5 adapter fixture: block and flex cell contents return their
    /// baselines through the formatter's own output, and the table algorithm
    /// aligns from those values alone. Nothing walks a backend descendant to
    /// rediscover a baseline, and no physical coordinate is stored as one.
    #[test]
    fn k4d5_cell_contents_return_baselines_directly() {
        use buckram::{
            FragmentDraftTree, TableCellAlignment, TableCellBlockStyle, TableCellFormatter,
            TableCellLayoutInput, TableCellLayoutOutput, TableRowLayoutError,
        };

        struct BaselineFormatter<'a> {
            tree: &'a mut AlgorithmTree<Style, TextMeasure, Option<BoxId>>,
            nodes: HashMap<BoxId, AlgorithmNodeId>,
        }

        impl TableCellFormatter for BaselineFormatter<'_> {
            fn format_cell(
                &mut self,
                input: TableCellLayoutInput,
            ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
                let node = *self.nodes.get(&input.box_id).ok_or(
                    TableRowLayoutError::InvalidCellOutput {
                        box_id: input.box_id,
                    },
                )?;
                self.tree.compute_layout_with_measure(
                    node,
                    AlgorithmSize::new(
                        AlgorithmAvailableSpace::Definite(input.content_inline_size),
                        AlgorithmAvailableSpace::MaxContent,
                    ),
                    |known, _, _, context, _| {
                        let Some(context) = context else {
                            return AlgorithmSize::new(0.0, 0.0);
                        };
                        AlgorithmSize::new(
                            known.width.unwrap_or(context.max_width),
                            known.height.unwrap_or(context.height),
                        )
                    },
                );
                self.tree.propagate_baselines();
                let layout = self.tree.layout(node);
                // The formatting context hands its baselines back directly.
                let baselines = self.tree.baselines(node);
                Ok(TableCellLayoutOutput {
                    content_block_size: layout.height,
                    border_box_min_block_size: 0.0,
                    baselines,
                    overflow: buckram::LogicalRect::default(),
                    fragments: FragmentDraftTree::default(),
                })
            }
        }

        let dom = StaticDocument::parse(
            "<table><tbody><tr><td id=a><i></i></td><td id=b><i></i><i></i></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; border-spacing: 0; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; padding: 0; } i { display: block; height: 12px; }",
            ]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = boxes
            .iter()
            .find_map(|(box_id, css_box)| {
                (css_box.display.internal_table == Some(buckram::InternalTableRole::Grid))
                    .then_some(box_id)
            })
            .expect("table grid box");
        let grid = build_table_grid(&boxes, &dom, table);

        let mut tree: AlgorithmTree<Style, TextMeasure, Option<BoxId>> = AlgorithmTree::new();
        let mut nodes = HashMap::new();
        for (index, cell) in grid.cells.iter().enumerate() {
            // A block container's first baseline is its first child's, so the
            // cells differ in their first block rather than their count.
            let height = if index == 0 { 20.0 } else { 12.0 };
            let blocks = vec![tree.new_with_children_and_block_style(
                AlgorithmKind::Block,
                BlockStyle {
                    size: BlockDimensions::new(
                        BlockSizeValue::Auto,
                        BlockSizeValue::Length(FlowLength::px(height)),
                    ),
                    ..BlockStyle::default()
                },
                Style {
                    size: Size {
                        width: Dimension::auto(),
                        height: Dimension::length(height),
                    },
                    ..Style::default()
                },
                &[],
                None,
            )];
            nodes.insert(
                cell.source,
                tree.new_with_children_and_block_style(
                    AlgorithmKind::Block,
                    BlockStyle::default(),
                    Style::default(),
                    &blocks,
                    None,
                ),
            );
        }

        let inline = {
            let sizing = buckram::TableInlineSizingInput {
                grid: &grid,
                available_inline_size: Some(80.0),
                table_constraints: buckram::TableInlineConstraints::default(),
                border_metrics: buckram::TableInlineBorderMetrics::Separated(
                    buckram::TableSeparatedBorderMetrics::default(),
                ),
                caption_min: buckram::CaptionMinContribution::NoCaption,
                track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
            };
            buckram::TableInlineSizingResult::new(
                &sizing,
                buckram::IntrinsicSizes::new(80.0, 80.0).expect("intrinsic pair"),
                80.0,
                80.0,
                vec![40.0, 40.0],
            )
            .expect("reconciled inline result")
        };
        let input = buckram::TableBlockSizingInput {
            grid: &grid,
            inline: &inline,
            table_constraint: buckram::TableBlockConstraint::Auto,
            table_box_sizing: buckram::TableBoxSizing::BorderBox,
            border_metrics: buckram::TableBlockBorderMetrics::Separated(
                buckram::TableSeparatedBlockMetrics::default(),
            ),
            available_block_size: None,
            track_visibility: buckram::TableTrackVisibility::all_visible(&grid),
        };
        let mut formatter = BaselineFormatter {
            tree: &mut tree,
            nodes,
        };
        let outputs = buckram::format_table_cells(&input, 0.0, |_, _| 0.0, &mut formatter)
            .expect("formatted cells");
        // Each cell reported a real baseline from its own formatting context.
        assert!(
            outputs
                .iter()
                .all(|(_, output)| output.baselines.first.is_some())
        );

        let styles = vec![TableCellBlockStyle::default(); grid.cells.len()];
        let mut measures = buckram::measure_single_span_rows(
            &input,
            &styles,
            &outputs,
            &[buckram::TableBlockConstraint::Auto],
        )
        .expect("measures");
        buckram::apply_baseline_row_minima(&input, &styles, &outputs, &mut measures)
            .expect("baseline minima");
        let sizing =
            buckram::size_table_rows(&input, &measures, &styles, &outputs).expect("sizing");
        let alignment =
            buckram::align_table_cells(&input, &sizing, &styles, &outputs, 0.0).expect("alignment");

        // Cell a's baseline is 20 and cell b's is 12, so the row takes 20 and
        // shifts b down by 8 to meet it.
        assert!(alignment.rows[0].from_aligned_cell);
        assert!(
            (alignment.rows[0].baseline - 20.0).abs() < 0.05,
            "{alignment:?}"
        );
        assert!(
            (alignment.cells[0].content_block_offset).abs() < 0.05,
            "{alignment:?}"
        );
        assert!(
            (alignment.cells[1].content_block_offset - 8.0).abs() < 0.05,
            "{alignment:?}"
        );
        assert_eq!(
            alignment.baselines.first,
            Some(alignment.rows[0].baseline),
            "the table's first baseline is its first row's"
        );
        // Alignment never touches K4c's columns.
        assert_eq!(inline.column_sizes, vec![40.0, 40.0]);
        assert!(
            alignment
                .cells
                .iter()
                .all(|cell| (cell.rect.inline_size - 40.0).abs() < 0.05)
        );
        assert_eq!(
            buckram::TableCellAlignment::default(),
            TableCellAlignment::Baseline
        );
    }

    #[test]
    fn html_column_and_column_group_spans_are_bounded_at_the_adapter() {
        let dom = StaticDocument::parse(
            "<table id=table><colgroup span=9001></colgroup><col span=9001><tbody><tr><td></td></tr></tbody></table>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "table { display: table; } colgroup { display: table-column-group; } col { display: table-column; } tbody { display: table-row-group; } tr { display: table-row; } td { display: table-cell; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let table = node_by_id(&dom, dom.document(), "table").expect("table");
        let grid = boxes.principal_box(table).expect("table grid");
        let model = build_table_grid(&boxes, &dom, grid);

        assert_eq!(model.column_groups[0].span, 1_000);
        assert_eq!(model.columns.len(), 2_000);
    }

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
    fn live_orthogonal_normal_flow_preserves_logical_fragment_geometry_and_baseline() {
        use buckram::{Direction, WritingMode};

        let document = StaticDocument::parse(
            "<html><body><div class=\"vertical\"><div>orthogonal text</div></div></body></html>",
        );
        let styles = resolve_styles(
            &document,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; } \
                 .vertical { writing-mode: vertical-rl; height: 100px; }"]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let vertical = document
            .first_with_class(document.document(), "vertical")
            .expect("vertical host");
        assert!(
            styles
                .get(vertical)
                .is_some_and(|style| style.writing_mode.is_vertical()),
            "the cascade must retain vertical-rl for the principal box"
        );
        let mut text = TextSystem::new();
        let (resolved, layout) = layout_with_text_system(
            &document,
            &styles,
            320.0,
            240.0,
            ViewportSizes::uniform(320.0, 240.0),
            &mut text,
            &HashMap::new(),
        )
        .expect("orthogonal layout");

        assert!(
            resolved
                .get(vertical)
                .is_some_and(|style| style.writing_mode.is_vertical()),
            "relative-unit resolution must preserve vertical-rl"
        );

        let vertical_box = layout
            .boxes()
            .principal_box(vertical)
            .expect("vertical principal box");
        assert_eq!(
            layout.boxes()[vertical_box].flow,
            FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr),
            "the generated principal box must preserve the computed flow"
        );
        let fragment = layout
            .fragments()
            .fragments_for_box(vertical_box)
            .next()
            .expect("vertical fragment");
        let physical = fragment.physical_rect();
        assert_eq!(
            fragment.flow(),
            FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr)
        );
        assert_eq!(fragment.logical_rect.inline_size, physical.height);
        assert_eq!(fragment.logical_rect.block_size, physical.width);
        assert!(
            fragment.baselines.first.is_some() && fragment.baselines.last.is_some(),
            "the host must retain its modeled BFC baseline output"
        );
        assert!(
            fragment
                .containing_fragment()
                .and_then(|id| layout.fragments().get(id))
                .is_some(),
            "the orthogonal host must retain its containing fragment"
        );
        assert!(layout.block_algorithm_counts().buckram >= 4);
    }

    #[test]
    fn contained_root_keeps_body_writing_mode_local_to_body_content() {
        fn first_text(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.kind(node) == NodeKind::Text
                && dom.text(node).is_some_and(|text| !text.is_empty())
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| first_text(dom, child))
        }

        fn text_fragment_x(
            document: &StaticDocument,
            css: &str,
        ) -> (
            f32,
            Fragment,
            FlowAxes,
            Option<FlowAxes>,
            (CssSize, CssSize),
        ) {
            let styles = resolve_styles(
                document,
                &StyleSet::cambium(&[css]),
                &Device::screen(800.0, 600.0),
                &InteractionStates::default(),
            );
            let mut text = TextSystem::new();
            let (_, layout) = layout_with_text_system(
                document,
                &styles,
                800.0,
                600.0,
                ViewportSizes::uniform(800.0, 600.0),
                &mut text,
                &HashMap::new(),
            )
            .expect("writing-mode layout");
            let source = first_text(document, document.document()).expect("text node");
            let text_x = layout
                .fragments_for_node(source)
                .next()
                .expect("text fragment")
                .physical_rect()
                .x;
            let parent = document.parent(source).expect("text parent");
            let parent = layout.get(parent).expect("parent fragment").physical_rect();
            let parent_box = layout
                .boxes()
                .principal_box(document.parent(source).expect("text parent"))
                .expect("parent box");
            let flow = layout.boxes()[parent_box].flow;
            let containing_flow = layout.boxes()[parent_box]
                .parent()
                .map(|containing| layout.boxes()[containing].flow);
            let style = styles
                .get(document.parent(source).expect("text parent"))
                .expect("parent style");
            (
                text_x,
                parent,
                flow,
                containing_flow,
                (style.width, style.height),
            )
        }

        let target = StaticDocument::parse(
            "<html><body>This text should run vertically on the left side</body></html>",
        );
        let reference = StaticDocument::parse(
            "<html><body><div>This text should run vertically on the left side</div></body></html>",
        );
        let target_x = text_fragment_x(
            &target,
            "html { contain: paint; } body { writing-mode: vertical-rl; }",
        );
        let reference_x = text_fragment_x(&reference, "div { writing-mode: vertical-rl; }");

        assert_eq!(
            target_x.0, reference_x.0,
            "target={target_x:?} reference={reference_x:?}"
        );
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
    fn live_nowrap_nested_inline_content_uses_float_bands_in_both_directions() {
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
             <div id=\"ltr\" class=\"host\"><div class=\"float\"></div>\
             <span id=\"ltr-copy\"><span><span>aa aa aa aa</span></span></span></div>\
             <div id=\"rtl\" class=\"host\"><div class=\"float\"></div>\
             <span id=\"rtl-copy\"><span><span>aa aa aa aa</span></span></span></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 .host { width: 200px; overflow-x: hidden; overflow-y: hidden;\
                         white-space: nowrap; font-family: monospace; font-size: 10px;\
                         line-height: 20px; }\
                 .float { float: left; width: 80px; height: 40px; }\
                 #rtl { direction: rtl; }",
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
        let copy_lines = |id| {
            let node = by_id(&dom, dom.document(), id).expect(id);
            layout
                .fragments_for_node(node)
                .map(|fragment| fragment.physical_rect())
                .collect::<Vec<_>>()
        };

        let ltr = rect("ltr");
        let rtl = rect("rtl");
        let ltr_lines = copy_lines("ltr-copy");
        let rtl_lines = copy_lines("rtl-copy");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(ltr_lines.len(), 1, "nowrap must remain one line");
        assert_eq!(rtl_lines.len(), 1, "nowrap must remain one line");
        assert_eq!((ltr_lines[0].x, ltr_lines[0].y), (ltr.x + 80.0, ltr.y));
        assert!(
            rtl_lines[0].x >= rtl.x + 80.0 - 0.5
                && rtl_lines[0].x + rtl_lines[0].width <= rtl.x + rtl.width + 0.5
                && (rtl_lines[0].y - rtl.y).abs() <= 0.5,
            "rtl host={rtl:?}, lines={rtl_lines:?}"
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
    fn live_generated_block_roots_translate_nested_float_state() {
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
            "<html><body><div id=\"host\"><div id=\"outer\"><div id=\"middle\">\
             <div id=\"float\"></div></div></div><div id=\"clear\"></div></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 #host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 #outer { margin-top: 10px; }\
                 #middle { margin-top: 20px; padding-top: 5px; border-top: 3px solid; }\
                 #float { float: left; width: 80px; height: 40px; }\
                 #clear { clear: left; height: 10px; }"]),
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
        let outer = rect("outer");
        let middle = rect("middle");
        let float = rect("float");
        let clear = rect("clear");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!((outer.y - host.y, middle.y - host.y), (20.0, 20.0));
        assert_eq!(float.y - host.y, 28.0);
        assert_eq!(clear.y - host.y, 68.0);
        assert_eq!(host.height, 78.0);
        assert_eq!(algorithms.taffy, 0);
    }

    #[test]
    fn livery_box_tree_preserves_split_inline_float_provenance() {
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
            "<html><body><div id=\"host\"><div id=\"wrapper\"><span id=\"split\">before\
             <span id=\"inline-float\"></span><span id=\"block\"></span>after</span></div>\
             <div id=\"clear\"></div></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 #host { width: 200px; overflow-x: hidden; overflow-y: hidden; }\
                 #inline-float { float: left; width: 80px; height: 40px; }\
                 #block { display: block; height: 0; }\
                 #clear { clear: left; height: 10px; }",
            ]),
            &Device::screen(320.0, 240.0),
            &InteractionStates::default(),
        );
        let split = by_id(&dom, dom.document(), "split").expect("split");
        let inline_float = by_id(&dom, dom.document(), "inline-float").expect("inline float");
        let boxes = GeneratedBoxTree::from_dom(&dom, &styles);
        let float_box = boxes.principal_box(inline_float).expect("float box");

        assert_eq!(boxes.boxes_for_node(split).len(), 2);
        assert_eq!(
            boxes[float_box].float_context,
            FloatContextProvenance::Inline
        );
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
    fn live_bfc_auto_margins_fit_or_move_below_floats_in_both_directions() {
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
             <div id=\"lowered\" class=\"host\"><div class=\"right-float\"></div>\
             <div id=\"lowered-bfc\" class=\"bfc\"></div></div>\
             </body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["html, body, div { margin: 0; padding: 0; border: 0; }\
                 .host { width: 100px; overflow-x: hidden; overflow-y: hidden; }\
                 .right-float { float: right; width: 50px; height: 40px; }\
                 .left-float { float: left; width: 50px; height: 40px; }\
                 .bfc { display: flow-root; width: 30px; height: 20px; }\
                 #ltr-bfc { margin-left: auto; }\
                 #rtl { direction: rtl; } #rtl-bfc { margin-right: auto; }\
                 #lowered-bfc { width: 60px; margin-left: auto; margin-right: 10px; }"]),
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
        let lowered = rect("lowered");
        let lowered_bfc = rect("lowered-bfc");
        let algorithms = layout.block_algorithm_counts();

        assert_eq!(
            (
                ltr_bfc.x - ltr.x,
                ltr_bfc.y - ltr.y,
                ltr_bfc.width,
                ltr_bfc.height,
            ),
            (20.0, 0.0, 30.0, 20.0),
            "ltr={ltr:?}, ltr_bfc={ltr_bfc:?}, rtl={rtl:?}, rtl_bfc={rtl_bfc:?}, lowered={lowered:?}, lowered_bfc={lowered_bfc:?}, algorithms={algorithms:?}"
        );
        assert_eq!(
            (
                rtl_bfc.x - rtl.x,
                rtl_bfc.y - rtl.y,
                rtl_bfc.width,
                rtl_bfc.height,
            ),
            (50.0, 0.0, 30.0, 20.0)
        );
        assert_eq!(
            (
                lowered_bfc.x - lowered.x,
                lowered_bfc.y - lowered.y,
                lowered_bfc.width,
                lowered_bfc.height,
            ),
            (30.0, 40.0, 60.0, 20.0)
        );
        assert_eq!((ltr.height, rtl.height, lowered.height), (40.0, 40.0, 60.0));
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

    #[test]
    fn live_bfc_fragments_expose_text_flex_grid_and_atomic_baselines() {
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
            "<html><body><div id=\"host\"><span id=\"text\">text</span>\
             <span id=\"atomic\"></span><div id=\"flex\"><span id=\"flex-text\">flex</span></div>\
             <div id=\"grid\"><span id=\"grid-text\">grid</span></div></div></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&[
                "html, body, div, span { margin: 0; padding: 0; border: 0; }\
                 #host { width: 160px; font-family: monospace; font-size: 10px; line-height: 20px; }\
                 #atomic { display: inline-block; width: 20px; height: 12px; }\
                 #flex { display: flex; width: 80px; }\
                 #grid { display: grid; width: 80px; grid-template-columns: 1fr; }",
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
        let fragment = |id| {
            layout
                .get(by_id(&dom, dom.document(), id).expect(id))
                .expect(id)
        };
        let host = fragment("host");
        let text = fragment("text");
        let atomic = fragment("atomic");
        let flex = fragment("flex");
        let grid = fragment("grid");

        for (name, fragment) in [
            ("host", host),
            ("text", text),
            ("atomic", atomic),
            ("flex", flex),
            ("grid", grid),
        ] {
            assert!(
                fragment.baselines.first.is_some() && fragment.baselines.last.is_some(),
                "{name} must expose modeled first and last baselines"
            );
        }
        assert_eq!(
            atomic.baselines,
            Baselines::synthesized_from_block_end(atomic.physical_rect().height),
            "an admitted atomic context keeps its own block-end fallback"
        );
        assert!(
            host.baselines.first.expect("host first baseline") < host.logical_rect.block_size,
            "the independent host keeps its IFC first baseline instead of its block-end fallback"
        );
        assert!(
            host.baselines.first.expect("host first baseline")
                >= text.baselines.first.expect("text baseline"),
            "the host baseline must retain the text IFC contribution"
        );
        assert_eq!(
            flex.baselines,
            Baselines::synthesized_from_block_end(flex.physical_rect().height),
            "the admitted flex BFC returns its own empty-line fallback"
        );
        assert_eq!(
            grid.baselines,
            Baselines::synthesized_from_block_end(grid.physical_rect().height),
            "the admitted grid BFC returns its own empty-line fallback"
        );
        assert_eq!(
            host.baselines.last,
            grid.baselines
                .last
                .map(|baseline| { grid.physical_rect().y - host.physical_rect().y + baseline }),
            "the independent host consumes its admitted grid BFC output"
        );
    }
}
