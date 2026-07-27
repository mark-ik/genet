//! Caller-owned scratch tree for Taffy's low-level layout algorithms.
//!
//! Taffy supplies block, flex, and grid algorithms. Buckram owns the tree,
//! source identity, contexts, and returned placements. The public surface in
//! this module deliberately uses Buckram types rather than Taffy types.

use std::{marker::PhantomData, slice};

use taffy::{
    BlockContext, Cache, CacheTree, Layout, LayoutBlockContainer, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RoundTree, RunMode,
    SizingMode, Style, TraversePartialTree, TraverseTree, compute_block_layout,
    compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_hidden_layout,
    compute_leaf_layout, compute_root_layout, round_layout,
};

use crate::{
    BlockBoxSizing, BlockContainingBlock, BlockDeferral, BlockFormattingContext, BlockMarginState,
    BlockSizeValue, BlockStyle, ClearSide, CollapsedMargin, FloatLineConstraints, FloatSide,
    FlowAxes, LogicalSides, PhysicalSides, PhysicalSize, solve_float_inline_size,
    solve_in_flow_inline_size,
};

/// Formatting role selected by Buckram before entering a backend algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlgorithmKind {
    Hidden,
    Leaf,
    Block,
    Flex,
    Grid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAlgorithm {
    Buckram,
    Taffy,
}

/// Stable identity within one scratch algorithm tree.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AlgorithmNodeId(u32);

impl AlgorithmNodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    fn from_taffy(id: NodeId) -> Self {
        Self(
            usize::from(id)
                .try_into()
                .expect("a Taffy node id exceeded u32::MAX"),
        )
    }

    fn into_taffy(self) -> NodeId {
        NodeId::from(self.index())
    }
}

/// Width and height pair at the algorithm boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AlgorithmSize<T> {
    pub width: T,
    pub height: T,
}

impl<T> AlgorithmSize<T> {
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }
}

/// Available-space constraint without exposing the backend enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlgorithmAvailableSpace {
    Definite(f32),
    MinContent,
    MaxContent,
}

/// Parent-relative placement returned by the algorithm backend.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AlgorithmLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

mod sealed {
    pub trait AlgorithmStyle {
        fn as_taffy_style(&self) -> &taffy::Style;
    }

    impl AlgorithmStyle for taffy::Style {
        fn as_taffy_style(&self) -> &taffy::Style {
            self
        }
    }
}

/// A style accepted by the private Taffy algorithm adapter.
///
/// This sealed trait keeps Taffy out of Buckram's method signatures while the
/// K1 caller still constructs the backend style privately. K3 replaces that
/// caller-side lowering with Buckram-owned logical and intrinsic inputs.
pub trait AlgorithmStyle: sealed::AlgorithmStyle {}

impl AlgorithmStyle for Style {}

struct AlgorithmNode<S, Context, Source> {
    kind: AlgorithmKind,
    block_style: BlockStyle,
    block_algorithm: Option<BlockAlgorithm>,
    block_margins: Option<BlockMarginState>,
    float_line_constraints_enabled: bool,
    float_avoidance_enabled: bool,
    style: S,
    context: Option<Context>,
    source: Source,
    parent: Option<AlgorithmNodeId>,
    children: Vec<AlgorithmNodeId>,
    cache: Cache,
    unrounded_layout: Layout,
    final_layout: Layout,
}

/// A caller-owned arena used only while running layout algorithms.
///
/// Source identity lives on each node, so callers need no parallel
/// `NodeId -> source` map. The style parameter is generic and the public
/// methods expose only Buckram identifiers and geometry.
pub struct AlgorithmTree<S, Context, Source> {
    nodes: Vec<AlgorithmNode<S, Context, Source>>,
}

impl<S, Context, Source> Default for AlgorithmTree<S, Context, Source> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, Context, Source> AlgorithmTree<S, Context, Source> {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn new_with_children(
        &mut self,
        kind: AlgorithmKind,
        style: S,
        children: &[AlgorithmNodeId],
        source: Source,
    ) -> AlgorithmNodeId {
        self.new_with_children_and_block_style(kind, BlockStyle::default(), style, children, source)
    }

    pub fn new_with_children_and_block_style(
        &mut self,
        kind: AlgorithmKind,
        block_style: BlockStyle,
        style: S,
        children: &[AlgorithmNodeId],
        source: Source,
    ) -> AlgorithmNodeId {
        self.push(kind, block_style, style, children, None, source)
    }

    pub fn new_leaf_with_context(
        &mut self,
        style: S,
        context: Context,
        source: Source,
    ) -> AlgorithmNodeId {
        self.new_leaf_with_context_and_block_style(BlockStyle::default(), style, context, source)
    }

    pub fn new_leaf_with_context_and_block_style(
        &mut self,
        block_style: BlockStyle,
        style: S,
        context: Context,
        source: Source,
    ) -> AlgorithmNodeId {
        self.push(
            AlgorithmKind::Leaf,
            block_style,
            style,
            &[],
            Some(context),
            source,
        )
    }

    fn push(
        &mut self,
        kind: AlgorithmKind,
        block_style: BlockStyle,
        style: S,
        children: &[AlgorithmNodeId],
        context: Option<Context>,
        source: Source,
    ) -> AlgorithmNodeId {
        let id = AlgorithmNodeId(
            self.nodes
                .len()
                .try_into()
                .expect("an algorithm tree exceeded u32::MAX nodes"),
        );
        self.nodes.push(AlgorithmNode {
            kind,
            block_style,
            block_algorithm: None,
            block_margins: None,
            float_line_constraints_enabled: false,
            float_avoidance_enabled: false,
            style,
            context,
            source,
            parent: None,
            children: children.to_vec(),
            cache: Cache::new(),
            unrounded_layout: Layout::new(),
            final_layout: Layout::new(),
        });
        for child in children {
            let previous = self.nodes[child.index()].parent.replace(id);
            assert!(
                previous.is_none(),
                "an algorithm scratch node cannot have two parents"
            );
        }
        id
    }

    pub fn source(&self, id: AlgorithmNodeId) -> &Source {
        &self.nodes[id.index()].source
    }

    pub fn kind(&self, id: AlgorithmNodeId) -> AlgorithmKind {
        self.nodes[id.index()].kind
    }

    pub fn style(&self, id: AlgorithmNodeId) -> &S {
        &self.nodes[id.index()].style
    }

    pub fn block_style(&self, id: AlgorithmNodeId) -> BlockStyle {
        self.nodes[id.index()].block_style
    }

    pub fn block_algorithm(&self, id: AlgorithmNodeId) -> Option<BlockAlgorithm> {
        self.nodes[id.index()].block_algorithm
    }

    pub fn block_margins(&self, id: AlgorithmNodeId) -> Option<BlockMarginState> {
        self.nodes[id.index()].block_margins
    }

    pub fn block_algorithm_counts(&self) -> (usize, usize) {
        self.nodes.iter().fold((0, 0), |(buckram, taffy), node| {
            match node.block_algorithm {
                Some(BlockAlgorithm::Buckram) => (buckram + 1, taffy),
                Some(BlockAlgorithm::Taffy) => (buckram, taffy + 1),
                None => (buckram, taffy),
            }
        })
    }

    pub fn style_mut(&mut self, id: AlgorithmNodeId) -> &mut S {
        &mut self.nodes[id.index()].style
    }

    /// Admit this direct measured leaf to Buckram's float-aware line lane.
    ///
    /// Measured contexts opt in explicitly because a generic callback is not
    /// necessarily an inline formatter and may ignore float constraints.
    pub fn enable_float_line_constraints(&mut self, id: AlgorithmNodeId) {
        assert!(
            self.nodes[id.index()].context.is_some(),
            "only a measured leaf can consume float line constraints"
        );
        self.nodes[id.index()].float_line_constraints_enabled = true;
    }

    /// Admit this block-level independent formatting context to Buckram's
    /// float-avoidance policy.
    ///
    /// The caller opts in because a generic flex/grid backend or atomic inline
    /// box can establish a BFC while still requiring sizing and baseline work
    /// outside this lane.
    pub fn enable_float_avoidance(&mut self, id: AlgorithmNodeId) {
        let node = &mut self.nodes[id.index()];
        assert!(
            matches!(node.kind, AlgorithmKind::Leaf | AlgorithmKind::Block),
            "the first float-avoidance lane accepts block and leaf algorithms"
        );
        assert!(
            node.block_style.establishes_bfc
                && node.block_style.float == FloatSide::None
                && node.block_style.has_zero_inline_margins(),
            "float avoidance requires an in-flow BFC with zero inline margins"
        );
        node.float_avoidance_enabled = true;
    }

    pub fn children(&self, id: AlgorithmNodeId) -> &[AlgorithmNodeId] {
        &self.nodes[id.index()].children
    }

    pub fn context(&self, id: AlgorithmNodeId) -> Option<&Context> {
        self.nodes[id.index()].context.as_ref()
    }

    pub fn layout(&self, id: AlgorithmNodeId) -> AlgorithmLayout {
        let layout = self.nodes[id.index()].final_layout;
        AlgorithmLayout {
            x: layout.location.x,
            y: layout.location.y,
            width: layout.size.width,
            height: layout.size.height,
        }
    }
}

impl<S, Context, Source> AlgorithmTree<S, Context, Source>
where
    S: AlgorithmStyle,
{
    pub fn compute_layout_with_measure<Measure>(
        &mut self,
        root: AlgorithmNodeId,
        available: AlgorithmSize<AlgorithmAvailableSpace>,
        measure: Measure,
    ) where
        Measure: FnMut(
            AlgorithmSize<Option<f32>>,
            AlgorithmSize<AlgorithmAvailableSpace>,
            AlgorithmNodeId,
            Option<&mut Context>,
            Option<&FloatLineConstraints>,
        ) -> AlgorithmSize<f32>,
    {
        let available = taffy::Size {
            width: to_taffy_available(available.width),
            height: to_taffy_available(available.height),
        };
        let mut run = AlgorithmRun {
            tree: self,
            measure,
            line_constraints: None,
            marker: PhantomData,
        };
        compute_root_layout(&mut run, root.into_taffy(), available);
        round_layout(&mut run, root.into_taffy());
    }
}

fn to_taffy_available(value: AlgorithmAvailableSpace) -> taffy::AvailableSpace {
    match value {
        AlgorithmAvailableSpace::Definite(value) => taffy::AvailableSpace::Definite(value),
        AlgorithmAvailableSpace::MinContent => taffy::AvailableSpace::MinContent,
        AlgorithmAvailableSpace::MaxContent => taffy::AvailableSpace::MaxContent,
    }
}

fn from_taffy_available(value: taffy::AvailableSpace) -> AlgorithmAvailableSpace {
    match value {
        taffy::AvailableSpace::Definite(value) => AlgorithmAvailableSpace::Definite(value),
        taffy::AvailableSpace::MinContent => AlgorithmAvailableSpace::MinContent,
        taffy::AvailableSpace::MaxContent => AlgorithmAvailableSpace::MaxContent,
    }
}

#[derive(Clone, Copy)]
struct PhysicalOptionalSize {
    width: Option<f32>,
    height: Option<f32>,
}

impl PhysicalOptionalSize {
    fn from_taffy(size: taffy::Size<Option<f32>>) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }

    fn from_available(size: taffy::Size<taffy::AvailableSpace>) -> Self {
        Self {
            width: available_option(size.width),
            height: available_option(size.height),
        }
    }
}

fn available_option(value: taffy::AvailableSpace) -> Option<f32> {
    match value {
        taffy::AvailableSpace::Definite(value) => Some(value),
        taffy::AvailableSpace::MinContent | taffy::AvailableSpace::MaxContent => None,
    }
}

fn logical_optional_size(axes: FlowAxes, physical: PhysicalOptionalSize) -> LogicalOptionalSize {
    if axes.is_horizontal() {
        LogicalOptionalSize {
            inline: physical.width,
            block: physical.height,
        }
    } else {
        LogicalOptionalSize {
            inline: physical.height,
            block: physical.width,
        }
    }
}

struct LogicalOptionalSize {
    inline: Option<f32>,
    #[allow(dead_code)]
    block: Option<f32>,
}

fn resolve_outer_dimension(
    preferred: BlockSizeValue,
    minimum: BlockSizeValue,
    maximum: BlockSizeValue,
    containing_size: Option<f32>,
    padding_border: f32,
    box_sizing: BlockBoxSizing,
) -> Option<f32> {
    preferred
        .resolve_definite(containing_size)
        .map(|preferred| specified_outer_size(preferred, padding_border, box_sizing))
        .map(|preferred| {
            clamp_outer_dimension(
                preferred,
                minimum,
                maximum,
                containing_size,
                padding_border,
                box_sizing,
            )
        })
}

fn clamp_outer_dimension(
    value: f32,
    minimum: BlockSizeValue,
    maximum: BlockSizeValue,
    containing_size: Option<f32>,
    padding_border: f32,
    box_sizing: BlockBoxSizing,
) -> f32 {
    let minimum = minimum
        .resolve_definite(containing_size)
        .map(|minimum| specified_outer_size(minimum, padding_border, box_sizing))
        .unwrap_or(padding_border);
    let maximum = maximum
        .resolve_definite(containing_size)
        .map(|maximum| specified_outer_size(maximum, padding_border, box_sizing));
    let value = value.max(minimum);
    maximum.map_or(value, |maximum| value.min(maximum.max(padding_border)))
}

fn specified_outer_size(specified: f32, padding_border: f32, box_sizing: BlockBoxSizing) -> f32 {
    match box_sizing {
        BlockBoxSizing::ContentBox => specified.max(0.0) + padding_border,
        BlockBoxSizing::BorderBox => specified.max(padding_border),
    }
}

fn to_taffy_rect(sides: PhysicalSides<f32>) -> taffy::Rect<f32> {
    taffy::Rect {
        left: sides.left,
        right: sides.right,
        top: sides.top,
        bottom: sides.bottom,
    }
}

struct ChildIter<'a>(slice::Iter<'a, AlgorithmNodeId>);

impl Iterator for ChildIter<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().copied().map(AlgorithmNodeId::into_taffy)
    }
}

struct AlgorithmRun<'a, S, Context, Source, Measure> {
    tree: &'a mut AlgorithmTree<S, Context, Source>,
    measure: Measure,
    line_constraints: Option<FloatLineConstraints>,
    marker: PhantomData<&'a mut Context>,
}

#[derive(Clone, Copy)]
struct BlockChildInput {
    border_box_width: f32,
    containing_width: f32,
    available_width: f32,
    containing_height: Option<f32>,
    available_height: taffy::AvailableSpace,
}

impl<S, Context, Source, Measure> AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
    Measure: FnMut(
        AlgorithmSize<Option<f32>>,
        AlgorithmSize<AlgorithmAvailableSpace>,
        AlgorithmNodeId,
        Option<&mut Context>,
        Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32>,
{
    fn style(&self, id: NodeId) -> &Style {
        sealed::AlgorithmStyle::as_taffy_style(
            &self.tree.nodes[AlgorithmNodeId::from_taffy(id).index()].style,
        )
    }

    fn block_subtree_deferral(&self, node: AlgorithmNodeId) -> Option<BlockDeferral> {
        let mut active_left_float = false;
        let mut active_right_float = false;
        for child in self.tree.nodes[node.index()].children.iter().copied() {
            let child_node = &self.tree.nodes[child.index()];
            let child_style = child_node.block_style;
            if let Some(deferral) = child_style.deferral() {
                return Some(deferral);
            }
            if child_style.establishes_bfc
                && child_style.float == FloatSide::None
                && !self.owns_direct_float_lane(child)
                && !child_node.float_avoidance_enabled
            {
                return Some(BlockDeferral::IndependentFormattingContext);
            }

            match child_style.clear {
                ClearSide::None => {},
                ClearSide::Left => active_left_float = false,
                ClearSide::Right => active_right_float = false,
                ClearSide::Both => {
                    active_left_float = false;
                    active_right_float = false;
                },
            }

            if child_style.float != FloatSide::None {
                match child_style.float {
                    FloatSide::None => {},
                    FloatSide::Left => active_left_float = true,
                    FloatSide::Right => active_right_float = true,
                }
                if child_node.kind == AlgorithmKind::Block
                    && let Some(deferral) = self.block_subtree_deferral(child)
                {
                    return Some(deferral);
                }
                continue;
            }

            let floats_are_active = active_left_float || active_right_float;
            if floats_are_active
                && child_style.establishes_bfc
                && !child_node.float_avoidance_enabled
            {
                return Some(BlockDeferral::FloatFormattingContextAvoidance);
            }
            if floats_are_active
                && !child_style.establishes_bfc
                && !child_node.float_line_constraints_enabled
                && self.has_line_boxes_in_same_bfc(child)
            {
                return Some(BlockDeferral::FloatLineExclusion);
            }

            if child_node.kind == AlgorithmKind::Block {
                if !child_style.establishes_bfc && self.exports_float_state(child) {
                    return Some(BlockDeferral::NestedFloatState);
                }
                if let Some(deferral) = self.block_subtree_deferral(child) {
                    return Some(deferral);
                }
            }
        }
        None
    }

    fn owns_direct_float_lane(&self, node: AlgorithmNodeId) -> bool {
        self.tree.nodes[node.index()]
            .children
            .iter()
            .copied()
            .any(|child| self.tree.nodes[child.index()].block_style.float != FloatSide::None)
    }

    fn exports_float_state(&self, node: AlgorithmNodeId) -> bool {
        self.tree.nodes[node.index()]
            .children
            .iter()
            .copied()
            .any(|child| {
                let child_node = &self.tree.nodes[child.index()];
                let style = child_node.block_style;
                style.float != FloatSide::None
                    || style.clear != ClearSide::None
                    || (child_node.kind == AlgorithmKind::Block
                        && !style.establishes_bfc
                        && self.exports_float_state(child))
            })
    }

    fn has_line_boxes_in_same_bfc(&self, node: AlgorithmNodeId) -> bool {
        let node = &self.tree.nodes[node.index()];
        node.context.is_some()
            || node.children.iter().copied().any(|child| {
                let child_node = &self.tree.nodes[child.index()];
                let style = child_node.block_style;
                style.float == FloatSide::None
                    && !style.establishes_bfc
                    && self.has_line_boxes_in_same_bfc(child)
            })
    }

    fn compute_block_child(
        &mut self,
        child: AlgorithmNodeId,
        input: BlockChildInput,
        line_constraints: Option<FloatLineConstraints>,
    ) -> LayoutOutput {
        if line_constraints.is_some() {
            // Float geometry is not represented in Taffy's cache key. Force
            // a measured inline leaf through the caller on the final pass.
            self.tree.nodes[child.index()].cache.clear();
        }
        let previous_line_constraints =
            std::mem::replace(&mut self.line_constraints, line_constraints);
        let output = self.compute_node(
            child.into_taffy(),
            LayoutInput {
                run_mode: RunMode::PerformLayout,
                sizing_mode: SizingMode::InherentSize,
                axis: taffy::RequestedAxis::Both,
                known_dimensions: taffy::Size {
                    width: Some(input.border_box_width),
                    height: None,
                },
                parent_size: taffy::Size {
                    width: Some(input.containing_width),
                    height: input.containing_height,
                },
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::Definite(input.available_width),
                    height: input
                        .containing_height
                        .map_or(input.available_height, taffy::AvailableSpace::Definite),
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            },
            None,
        );
        self.line_constraints = previous_line_constraints;
        output
    }

    fn clear_subtree_cache(&mut self, node: AlgorithmNodeId) {
        let children = self.tree.nodes[node.index()].children.clone();
        self.tree.nodes[node.index()].cache.clear();
        for child in children {
            self.clear_subtree_cache(child);
        }
    }

    fn compute_owned_block_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
    ) -> Result<LayoutOutput, BlockDeferral> {
        if inputs.run_mode != RunMode::PerformLayout
            || inputs.sizing_mode != SizingMode::InherentSize
            || inputs.axis != taffy::RequestedAxis::Both
        {
            return Err(BlockDeferral::BackendSizingMode);
        }

        let node = AlgorithmNodeId::from_taffy(node_id);
        let node_index = node.index();
        let parent_is_block = self.tree.nodes[node_index]
            .parent
            .is_none_or(|parent| self.tree.nodes[parent.index()].kind == AlgorithmKind::Block);
        if !parent_is_block {
            return Err(BlockDeferral::BackendSizingMode);
        }

        let style = self.tree.nodes[node_index].block_style;
        if let Some(deferral) = style.deferral() {
            return Err(deferral);
        }
        // The logical engine supports a definite vertical containing block,
        // but auto block-size finalisation for vertical flow needs a deferred
        // physical conversion pass. Keep that boundary named for now.
        if !style.flow.is_horizontal() {
            return Err(BlockDeferral::OrthogonalAutoBlockSize);
        }

        let parent_physical = PhysicalOptionalSize::from_taffy(inputs.parent_size);
        let available_physical = PhysicalOptionalSize::from_available(inputs.available_space);
        let parent_logical = logical_optional_size(style.containing_flow, parent_physical);
        let available_logical = logical_optional_size(style.containing_flow, available_physical);
        let containing_inline = parent_logical
            .inline
            .or(available_logical.inline)
            .ok_or(BlockDeferral::IndefiniteInlineSize)?;
        let containing_block_size = parent_logical.block.or(available_logical.block);
        let padding = style.resolved_padding(containing_inline);
        let border = style.border;
        let padding_border = padding.zip_map(border, |padding, border| padding + border);

        let mut outer = PhysicalOptionalSize::from_taffy(inputs.known_dimensions);
        outer.width = outer.width.or_else(|| {
            resolve_outer_dimension(
                style.size.width,
                style.min_size.width,
                style.max_size.width,
                parent_physical.width,
                padding_border.left + padding_border.right,
                style.box_sizing,
            )
        });
        outer.height = outer.height.or_else(|| {
            resolve_outer_dimension(
                style.size.height,
                style.min_size.height,
                style.max_size.height,
                parent_physical.height,
                padding_border.top + padding_border.bottom,
                style.box_sizing,
            )
        });
        if outer.width.is_none() {
            outer.width = available_physical.width;
        }

        let content_width = outer
            .width
            .map(|width| (width - padding_border.left - padding_border.right).max(0.0))
            .ok_or(BlockDeferral::IndefiniteInlineSize)?;
        let content_height = outer
            .height
            .map(|height| (height - padding_border.top - padding_border.bottom).max(0.0));

        if let Some(deferral) = self.block_subtree_deferral(node) {
            return Err(deferral);
        }

        let is_layout_root = self.tree.nodes[node_index].parent.is_none();
        let collapse_parent_start = style
            .child_margin_collapse(
                containing_inline,
                containing_block_size,
                is_layout_root,
                false,
            )
            .block_start;
        let content_box = PhysicalSize {
            width: content_width,
            height: content_height.unwrap_or(0.0),
        };
        let mut formatting_context = BlockFormattingContext::with_margin_collapse(
            BlockContainingBlock {
                flow: style.flow,
                content_box,
            },
            collapse_parent_start,
        );
        let children = self.tree.nodes[node_index].children.clone();
        for (order, child) in children.into_iter().enumerate() {
            let child_style = self.tree.nodes[child.index()].block_style;
            let inline = if child_style.float == FloatSide::None {
                solve_in_flow_inline_size(child_style, content_width)
            } else {
                solve_float_inline_size(child_style, content_width)
            };
            let provisional_margin_state = BlockMarginState::from_box(
                child_style,
                content_width,
                CollapsedMargin::ZERO,
                CollapsedMargin::ZERO,
                child_style.child_margin_collapse(content_width, content_height, false, true),
                false,
            );
            let avoids_floats = child_style.float == FloatSide::None
                && self.tree.nodes[child.index()].float_avoidance_enabled
                && formatting_context.float_exclusion_count() != 0;
            let mut float_avoiding_placement = None;
            let child_output = if avoids_floats {
                let mut measured_block_size = 0.0;
                let attempts = formatting_context.float_exclusion_count() * 2 + 3;
                let mut measured = None;
                for _ in 0..attempts {
                    let candidate = formatting_context.float_avoiding_placement(
                        child_style,
                        provisional_margin_state,
                        measured_block_size,
                    );
                    // The float band is not represented in Taffy's cache
                    // key, and earlier intrinsic/root probes may have cached
                    // this isolated subtree at the full containing width.
                    self.clear_subtree_cache(child);
                    let output = self.compute_block_child(
                        child,
                        BlockChildInput {
                            border_box_width: candidate.inline_size.border_box,
                            containing_width: content_width,
                            available_width: candidate.inline_size.border_box,
                            containing_height: content_height,
                            available_height: inputs.available_space.height,
                        },
                        None,
                    );
                    let child_size = PhysicalSize {
                        width: output.size.width,
                        height: output.size.height,
                    };
                    let actual_block_size = style.flow.logical_size(child_size).block;
                    let final_candidate = formatting_context.float_avoiding_placement(
                        child_style,
                        provisional_margin_state,
                        actual_block_size,
                    );
                    if (final_candidate.inline_size.border_box - candidate.inline_size.border_box)
                        .abs()
                        <= 0.01
                    {
                        float_avoiding_placement = Some(final_candidate);
                        measured = Some(output);
                        break;
                    }
                    measured_block_size = actual_block_size;
                }
                measured.ok_or(BlockDeferral::FloatFormattingContextAvoidance)?
            } else {
                let line_constraints = if child_style.float == FloatSide::None
                    && self.tree.nodes[child.index()].float_line_constraints_enabled
                {
                    let block_start = formatting_context
                        .hypothetical_in_flow_block_start(child_style, provisional_margin_state);
                    formatting_context.float_line_constraints(block_start)
                } else {
                    None
                };
                self.compute_block_child(
                    child,
                    BlockChildInput {
                        border_box_width: inline.border_box,
                        containing_width: content_width,
                        available_width: content_width,
                        containing_height: content_height,
                        available_height: inputs.available_space.height,
                    },
                    line_constraints,
                )
            };
            let child_size = PhysicalSize {
                width: child_output.size.width,
                height: child_output.size.height,
            };
            let placement = if child_style.float != FloatSide::None {
                formatting_context.place_float(child_style, child_size)
            } else {
                let child_margin_state =
                    if self.tree.nodes[child.index()].kind == AlgorithmKind::Block {
                        self.tree.nodes[child.index()]
                            .block_margins
                            .ok_or(BlockDeferral::ParentMarginCollapse)?
                    } else {
                        let child_size_logical = style.flow.logical_size(child_size);
                        let child_has_line_boxes = self.tree.nodes[child.index()].context.is_some();
                        let child_collapses_through = child_style.can_collapse_through(
                            content_width,
                            content_height,
                            false,
                            child_has_line_boxes,
                            true,
                        ) && child_size_logical.block == 0.0;
                        BlockMarginState::from_box(
                            child_style,
                            content_width,
                            CollapsedMargin::ZERO,
                            CollapsedMargin::ZERO,
                            child_style.child_margin_collapse(
                                content_width,
                                content_height,
                                false,
                                true,
                            ),
                            child_collapses_through,
                        )
                    };
                if child_style.clear != ClearSide::None && child_margin_state.collapses_through {
                    return Err(BlockDeferral::ClearanceThroughCollapsedBox);
                }
                if let Some(float_avoiding_placement) = float_avoiding_placement {
                    formatting_context.place_float_avoiding_in_flow(
                        child_style,
                        child_size,
                        child_margin_state,
                        float_avoiding_placement,
                    )
                } else {
                    formatting_context.place_in_flow_with_margins(
                        child_style,
                        child_size,
                        child_margin_state,
                    )
                }
            };
            let child_padding = child_style.resolved_padding(content_width);
            let child_border = child_style.border;
            let logical_margin = LogicalSides {
                inline_start: placement.margin_inline_start,
                inline_end: placement.margin_inline_end,
                block_start: 0.0,
                block_end: 0.0,
            };
            let child_margin = style.flow.physical_sides(logical_margin);
            let location = taffy::Point {
                x: padding_border.left + placement.rect.x,
                y: padding_border.top + placement.rect.y,
            };
            let mut child_layout = Layout::with_order(
                order
                    .try_into()
                    .expect("block child order exceeded u32::MAX"),
            );
            child_layout.location = location;
            child_layout.size = child_output.size;
            child_layout.scrollbar_size = taffy::Size::ZERO;
            child_layout.padding = to_taffy_rect(child_padding);
            child_layout.border = to_taffy_rect(child_border);
            child_layout.margin = to_taffy_rect(child_margin);
            self.set_unrounded_layout(child.into_taffy(), &child_layout);
        }

        let all_children_collapse_through = formatting_context.all_children_collapse_through();
        let margin_collapse = style.child_margin_collapse(
            containing_inline,
            containing_block_size,
            is_layout_root,
            all_children_collapse_through,
        );
        let collapses_through = style.can_collapse_through(
            containing_inline,
            containing_block_size,
            is_layout_root,
            false,
            all_children_collapse_through,
        );
        let collapse_parent_end = margin_collapse.block_end || collapses_through;
        let used_content_height = if is_layout_root || style.establishes_bfc {
            formatting_context.used_block_size_containing_floats(collapse_parent_end)
        } else {
            formatting_context.used_block_size_with_margin_collapse(collapse_parent_end)
        };
        let auto_outer_height = used_content_height + padding_border.top + padding_border.bottom;
        let final_height = outer.height.unwrap_or_else(|| {
            clamp_outer_dimension(
                auto_outer_height,
                style.min_size.height,
                style.max_size.height,
                parent_physical.height,
                padding_border.top + padding_border.bottom,
                style.box_sizing,
            )
        });
        let final_size = taffy::Size {
            width: outer.width.unwrap_or_default(),
            height: final_height,
        };
        self.tree.nodes[node_index].block_margins = Some(BlockMarginState::from_box(
            style,
            containing_inline,
            formatting_context.first_child_margin(),
            formatting_context.last_child_margin(),
            margin_collapse,
            collapses_through,
        ));
        Ok(LayoutOutput::from_outer_size(final_size))
    }

    fn compute_node(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_context: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return compute_hidden_layout(self, node_id);
        }

        compute_cached_layout(self, node_id, inputs, |tree, node_id, inputs| {
            let node_index = AlgorithmNodeId::from_taffy(node_id).index();
            let kind = tree.tree.nodes[node_index].kind;

            match kind {
                AlgorithmKind::Hidden => compute_hidden_layout(tree, node_id),
                AlgorithmKind::Block if block_context.is_none() => {
                    match tree.compute_owned_block_layout(node_id, inputs) {
                        Ok(output) => {
                            tree.tree.nodes[node_index].block_algorithm =
                                Some(BlockAlgorithm::Buckram);
                            output
                        },
                        Err(_) => {
                            tree.tree.nodes[node_index].block_algorithm =
                                Some(BlockAlgorithm::Taffy);
                            tree.tree.nodes[node_index].block_margins = None;
                            compute_block_layout(tree, node_id, inputs, None)
                        },
                    }
                },
                AlgorithmKind::Block => {
                    tree.tree.nodes[node_index].block_algorithm = Some(BlockAlgorithm::Taffy);
                    tree.tree.nodes[node_index].block_margins = None;
                    compute_block_layout(tree, node_id, inputs, block_context)
                },
                AlgorithmKind::Flex => compute_flexbox_layout(tree, node_id, inputs),
                AlgorithmKind::Grid => compute_grid_layout(tree, node_id, inputs),
                AlgorithmKind::Leaf => {
                    let node = &mut tree.tree.nodes[node_index];
                    let style = sealed::AlgorithmStyle::as_taffy_style(&node.style);
                    let context = node.context.as_mut();
                    let measure = &mut tree.measure;
                    let line_constraints = tree.line_constraints.as_ref();
                    compute_leaf_layout(
                        inputs,
                        style,
                        |_, _| 0.0,
                        |known, available| {
                            let measured = measure(
                                AlgorithmSize::new(known.width, known.height),
                                AlgorithmSize::new(
                                    from_taffy_available(available.width),
                                    from_taffy_available(available.height),
                                ),
                                AlgorithmNodeId::from_taffy(node_id),
                                context,
                                line_constraints,
                            );
                            taffy::Size {
                                width: measured.width,
                                height: measured.height,
                            }
                        },
                    )
                },
            }
        })
    }
}

impl<S, Context, Source, Measure> TraversePartialTree
    for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
{
    type ChildIter<'a>
        = ChildIter<'a>
    where
        Self: 'a;

    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        ChildIter(
            self.tree.nodes[AlgorithmNodeId::from_taffy(parent_node_id).index()]
                .children
                .iter(),
        )
    }

    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.tree.nodes[AlgorithmNodeId::from_taffy(parent_node_id).index()]
            .children
            .len()
    }

    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        self.tree.nodes[AlgorithmNodeId::from_taffy(parent_node_id).index()].children[child_index]
            .into_taffy()
    }
}

impl<S, Context, Source, Measure> TraverseTree for AlgorithmRun<'_, S, Context, Source, Measure> where
    S: AlgorithmStyle
{
}

impl<S, Context, Source, Measure> LayoutPartialTree
    for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
    Measure: FnMut(
        AlgorithmSize<Option<f32>>,
        AlgorithmSize<AlgorithmAvailableSpace>,
        AlgorithmNodeId,
        Option<&mut Context>,
        Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32>,
{
    type CoreContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        self.style(node_id)
    }

    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()].unrounded_layout = *layout;
    }

    fn resolve_calc_value(&self, _value: *const (), _basis: f32) -> f32 {
        0.0
    }

    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute_node(node_id, inputs, None)
    }
}

impl<S, Context, Source, Measure> CacheTree for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
{
    fn cache_get(&self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()]
            .cache
            .get(input)
    }

    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()]
            .cache
            .store(input, output);
    }

    fn cache_clear(&mut self, node_id: NodeId) {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()]
            .cache
            .clear();
    }
}

impl<S, Context, Source, Measure> LayoutBlockContainer
    for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
    Measure: FnMut(
        AlgorithmSize<Option<f32>>,
        AlgorithmSize<AlgorithmAvailableSpace>,
        AlgorithmNodeId,
        Option<&mut Context>,
        Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32>,
{
    type BlockContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type BlockItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_> {
        self.style(node_id)
    }

    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_> {
        self.style(child_node_id)
    }

    fn compute_block_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_context: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute_node(node_id, inputs, block_context)
    }
}

impl<S, Context, Source, Measure> LayoutFlexboxContainer
    for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
    Measure: FnMut(
        AlgorithmSize<Option<f32>>,
        AlgorithmSize<AlgorithmAvailableSpace>,
        AlgorithmNodeId,
        Option<&mut Context>,
        Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32>,
{
    type FlexboxContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        self.style(node_id)
    }

    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        self.style(child_node_id)
    }
}

impl<S, Context, Source, Measure> LayoutGridContainer
    for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
    Measure: FnMut(
        AlgorithmSize<Option<f32>>,
        AlgorithmSize<AlgorithmAvailableSpace>,
        AlgorithmNodeId,
        Option<&mut Context>,
        Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32>,
{
    type GridContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        self.style(node_id)
    }

    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        self.style(child_node_id)
    }
}

impl<S, Context, Source, Measure> RoundTree for AlgorithmRun<'_, S, Context, Source, Measure>
where
    S: AlgorithmStyle,
{
    fn get_unrounded_layout(&self, node_id: NodeId) -> Layout {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()].unrounded_layout
    }

    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.tree.nodes[AlgorithmNodeId::from_taffy(node_id).index()].final_layout = *layout;
    }
}

#[cfg(test)]
mod tests {
    use taffy::prelude::{Dimension, Display, Style, fr, length};

    use super::*;

    fn available(width: f32, height: f32) -> AlgorithmSize<AlgorithmAvailableSpace> {
        AlgorithmSize::new(
            AlgorithmAvailableSpace::Definite(width),
            AlgorithmAvailableSpace::Definite(height),
        )
    }

    fn zero_measure(
        _known: AlgorithmSize<Option<f32>>,
        _available: AlgorithmSize<AlgorithmAvailableSpace>,
        _node: AlgorithmNodeId,
        _context: Option<&mut ()>,
        _line_constraints: Option<&FloatLineConstraints>,
    ) -> AlgorithmSize<f32> {
        AlgorithmSize::new(0.0, 0.0)
    }

    #[test]
    fn flex_dispatch_preserves_exact_placements_and_sources() {
        let mut tree = AlgorithmTree::<Style, (), &str>::new();
        let child_style = Style {
            size: taffy::Size {
                width: Dimension::length(50.0),
                height: Dimension::length(20.0),
            },
            ..Style::default()
        };
        let first = tree.new_with_children(AlgorithmKind::Leaf, child_style.clone(), &[], "first");
        let second = tree.new_with_children(AlgorithmKind::Leaf, child_style, &[], "second");
        let root = tree.new_with_children(
            AlgorithmKind::Flex,
            Style {
                display: Display::Flex,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::length(40.0),
                },
                ..Style::default()
            },
            &[first, second],
            "root",
        );

        tree.compute_layout_with_measure(root, available(200.0, 40.0), zero_measure);

        assert_eq!(tree.source(second), &"second");
        assert_eq!(
            tree.layout(first),
            AlgorithmLayout {
                width: 50.0,
                height: 20.0,
                ..AlgorithmLayout::default()
            }
        );
        assert_eq!(
            tree.layout(second),
            AlgorithmLayout {
                x: 50.0,
                width: 50.0,
                height: 20.0,
                ..AlgorithmLayout::default()
            }
        );
    }

    #[test]
    fn grid_dispatch_preserves_exact_track_geometry() {
        let mut tree = AlgorithmTree::<Style, (), u8>::new();
        let children = (0..4)
            .map(|source| {
                tree.new_with_children(AlgorithmKind::Leaf, Style::default(), &[], source)
            })
            .collect::<Vec<_>>();
        let root = tree.new_with_children(
            AlgorithmKind::Grid,
            Style {
                display: Display::Grid,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::length(100.0),
                },
                grid_template_columns: vec![length(100.0_f32), fr(1.0_f32)],
                grid_template_rows: vec![length(50.0_f32), length(50.0_f32)],
                ..Style::default()
            },
            &children,
            9,
        );

        tree.compute_layout_with_measure(root, available(200.0, 100.0), zero_measure);

        assert_eq!(
            (tree.layout(children[0]).x, tree.layout(children[0]).y),
            (0.0, 0.0)
        );
        assert_eq!(
            (tree.layout(children[1]).x, tree.layout(children[1]).y),
            (100.0, 0.0)
        );
        assert_eq!(
            (tree.layout(children[2]).x, tree.layout(children[2]).y),
            (0.0, 50.0)
        );
        assert_eq!(
            (tree.layout(children[3]).x, tree.layout(children[3]).y),
            (100.0, 50.0)
        );
    }

    #[test]
    fn buckram_kind_selects_the_algorithm_independently_of_backend_display() {
        let mut tree = AlgorithmTree::<Style, (), u8>::new();
        let child_style = Style {
            size: taffy::Size {
                width: Dimension::length(50.0),
                height: Dimension::length(20.0),
            },
            ..Style::default()
        };
        let first = tree.new_with_children(AlgorithmKind::Leaf, child_style.clone(), &[], 1);
        let second = tree.new_with_children(AlgorithmKind::Leaf, child_style, &[], 2);
        let root = tree.new_with_children(
            AlgorithmKind::Flex,
            Style {
                // Deliberately contradictory. Dispatch must follow Buckram's
                // formatting role, not this private backend field.
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::length(40.0),
                },
                ..Style::default()
            },
            &[first, second],
            0,
        );

        tree.compute_layout_with_measure(root, available(200.0, 40.0), zero_measure);

        assert_eq!(tree.kind(root), AlgorithmKind::Flex);
        assert_eq!(tree.layout(first).x, 0.0);
        assert_eq!(tree.layout(second).x, 50.0);
        assert_eq!(tree.layout(second).y, 0.0);
    }

    #[test]
    fn buckram_block_flow_uses_css_inputs_instead_of_backend_sizes() {
        let mut tree = AlgorithmTree::<Style, (), u8>::new();
        let backend_child_style = Style {
            size: taffy::Size {
                // Deliberately contradictory: Buckram's CSS input says 80px.
                width: Dimension::length(10.0),
                height: Dimension::length(20.0),
            },
            ..Style::default()
        };
        let block_child_style = BlockStyle {
            size: crate::BlockDimensions::new(
                BlockSizeValue::Length(crate::FlowLength::px(80.0)),
                BlockSizeValue::Length(crate::FlowLength::px(20.0)),
            ),
            ..BlockStyle::default()
        };
        let first = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            block_child_style,
            backend_child_style.clone(),
            &[],
            1,
        );
        let second = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            block_child_style,
            backend_child_style,
            &[],
            2,
        );
        let root = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle::default(),
            Style {
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[first, second],
            0,
        );

        tree.compute_layout_with_measure(root, available(200.0, 100.0), zero_measure);

        assert_eq!(tree.layout(first).width, 80.0);
        assert_eq!(tree.layout(second).width, 80.0);
        assert_eq!(tree.layout(first).y, 0.0);
        assert_eq!(tree.layout(second).y, 20.0);
        assert_eq!(tree.layout(root).height, 40.0);
        assert_eq!(tree.block_algorithm(root), Some(BlockAlgorithm::Buckram));
    }

    #[test]
    fn buckram_block_flow_propagates_parent_child_and_empty_margin_chains() {
        fn block_style(height: f32, margin_top: f32, margin_bottom: f32) -> BlockStyle {
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Auto,
                    BlockSizeValue::Length(crate::FlowLength::px(height)),
                ),
                margin: PhysicalSides {
                    top: crate::FlowLengthAuto::Value(crate::FlowLength::px(margin_top)),
                    right: crate::FlowLengthAuto::ZERO,
                    bottom: crate::FlowLengthAuto::Value(crate::FlowLength::px(margin_bottom)),
                    left: crate::FlowLengthAuto::ZERO,
                },
                ..BlockStyle::default()
            }
        }

        let mut tree = AlgorithmTree::<Style, (), u8>::new();
        let child = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            block_style(20.0, 30.0, 40.0),
            Style {
                size: taffy::Size {
                    width: Dimension::auto(),
                    height: Dimension::length(20.0),
                },
                ..Style::default()
            },
            &[],
            2,
        );
        let parent = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle {
                margin: PhysicalSides {
                    top: crate::FlowLengthAuto::Value(crate::FlowLength::px(10.0)),
                    right: crate::FlowLengthAuto::ZERO,
                    bottom: crate::FlowLengthAuto::Value(crate::FlowLength::px(15.0)),
                    left: crate::FlowLengthAuto::ZERO,
                },
                ..BlockStyle::default()
            },
            Style {
                display: Display::Block,
                ..Style::default()
            },
            &[child],
            1,
        );
        let after = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            block_style(10.0, 12.0, 0.0),
            Style {
                size: taffy::Size {
                    width: Dimension::auto(),
                    height: Dimension::length(10.0),
                },
                ..Style::default()
            },
            &[],
            3,
        );
        let root = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle::default(),
            Style {
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[parent, after],
            0,
        );

        tree.compute_layout_with_measure(root, available(200.0, 200.0), zero_measure);

        assert_eq!(tree.layout(parent).y, 30.0);
        assert_eq!(tree.layout(child).y, 0.0);
        assert_eq!(tree.layout(after).y, 90.0);
        assert_eq!(tree.layout(root).height, 100.0);
        assert_eq!(tree.block_algorithm(parent), Some(BlockAlgorithm::Buckram));
        assert_eq!(tree.block_algorithm(root), Some(BlockAlgorithm::Buckram));
        assert_eq!(
            tree.block_margins(parent),
            Some(BlockMarginState {
                block_start: CollapsedMargin::from_margin(30.0),
                block_end: CollapsedMargin::from_margin(40.0),
                collapses_through: false,
            })
        );
    }

    #[test]
    fn buckram_places_direct_floats_and_clearance_inside_an_independent_bfc() {
        fn float_style(side: FloatSide, width: f32, height: f32) -> BlockStyle {
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Length(crate::FlowLength::px(width)),
                    BlockSizeValue::Length(crate::FlowLength::px(height)),
                ),
                float: side,
                establishes_bfc: true,
                ..BlockStyle::default()
            }
        }
        fn backend_size(width: f32, height: f32) -> Style {
            Style {
                size: taffy::Size {
                    width: Dimension::length(width),
                    height: Dimension::length(height),
                },
                ..Style::default()
            }
        }

        let mut tree = AlgorithmTree::<Style, (), u8>::new();
        let left = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            float_style(FloatSide::Left, 80.0, 40.0),
            backend_size(80.0, 40.0),
            &[],
            1,
        );
        let right = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            float_style(FloatSide::Right, 60.0, 70.0),
            backend_size(60.0, 70.0),
            &[],
            2,
        );
        let clear = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Auto,
                    BlockSizeValue::Length(crate::FlowLength::px(10.0)),
                ),
                clear: ClearSide::Both,
                ..BlockStyle::default()
            },
            backend_size(200.0, 10.0),
            &[],
            3,
        );
        let root = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle {
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style {
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[left, right, clear],
            0,
        );

        tree.compute_layout_with_measure(root, available(200.0, 200.0), zero_measure);

        assert_eq!((tree.layout(left).x, tree.layout(left).y), (0.0, 0.0));
        assert_eq!((tree.layout(right).x, tree.layout(right).y), (140.0, 0.0));
        assert_eq!((tree.layout(clear).x, tree.layout(clear).y), (0.0, 70.0));
        assert_eq!(tree.layout(root).height, 80.0);
        assert_eq!(tree.block_algorithm(root), Some(BlockAlgorithm::Buckram));
    }

    #[test]
    fn buckram_remeasures_opted_in_bfcs_inside_the_float_band() {
        let mut tree = AlgorithmTree::<Style, Vec<f32>, u8>::new();
        let float = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Length(crate::FlowLength::px(80.0)),
                    BlockSizeValue::Length(crate::FlowLength::px(40.0)),
                ),
                float: FloatSide::Left,
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style {
                size: taffy::Size {
                    width: Dimension::length(80.0),
                    height: Dimension::length(40.0),
                },
                ..Style::default()
            },
            &[],
            1,
        );
        let auto_bfc = tree.new_leaf_with_context_and_block_style(
            BlockStyle {
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style::default(),
            Vec::new(),
            2,
        );
        tree.enable_float_avoidance(auto_bfc);
        let definite_bfc = tree.new_leaf_with_context_and_block_style(
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Length(crate::FlowLength::px(150.0)),
                    BlockSizeValue::Auto,
                ),
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style::default(),
            Vec::new(),
            3,
        );
        tree.enable_float_avoidance(definite_bfc);
        let root = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle {
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style {
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[float, auto_bfc, definite_bfc],
            0,
        );

        tree.compute_layout_with_measure(
            root,
            available(200.0, 200.0),
            |known, available, _, context, _| {
                let width = known.width.unwrap_or(match available.width {
                    AlgorithmAvailableSpace::Definite(width) => width,
                    AlgorithmAvailableSpace::MinContent => 0.0,
                    AlgorithmAvailableSpace::MaxContent => f32::INFINITY,
                });
                if let Some(context) = context {
                    context.push(width);
                }
                AlgorithmSize::new(width, 20.0)
            },
        );

        assert_eq!(tree.block_algorithm(root), Some(BlockAlgorithm::Buckram));
        assert_eq!(tree.context(auto_bfc), Some(&vec![120.0]));
        assert_eq!(tree.context(definite_bfc), Some(&vec![150.0]));
        assert_eq!(
            tree.layout(auto_bfc),
            AlgorithmLayout {
                x: 80.0,
                y: 0.0,
                width: 120.0,
                height: 20.0,
            }
        );
        assert_eq!(
            tree.layout(definite_bfc),
            AlgorithmLayout {
                x: 0.0,
                y: 40.0,
                width: 150.0,
                height: 20.0,
            }
        );
        assert_eq!(tree.layout(root).height, 60.0);
    }

    #[test]
    fn buckram_delivers_float_constraints_to_a_direct_inline_leaf() {
        let mut tree = AlgorithmTree::<Style, Vec<crate::FloatAvailableSpace>, u8>::new();
        let float = tree.new_with_children_and_block_style(
            AlgorithmKind::Leaf,
            BlockStyle {
                size: crate::BlockDimensions::new(
                    BlockSizeValue::Length(crate::FlowLength::px(80.0)),
                    BlockSizeValue::Length(crate::FlowLength::px(40.0)),
                ),
                float: FloatSide::Left,
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style {
                size: taffy::Size {
                    width: Dimension::length(80.0),
                    height: Dimension::length(40.0),
                },
                ..Style::default()
            },
            &[],
            1,
        );
        let lines = tree.new_leaf_with_context_and_block_style(
            BlockStyle::anonymous(FlowAxes::HORIZONTAL_LTR, FlowAxes::HORIZONTAL_LTR),
            Style {
                display: Display::Block,
                ..Style::default()
            },
            Vec::new(),
            2,
        );
        tree.enable_float_line_constraints(lines);
        let root = tree.new_with_children_and_block_style(
            AlgorithmKind::Block,
            BlockStyle {
                establishes_bfc: true,
                ..BlockStyle::default()
            },
            Style {
                display: Display::Block,
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &[float, lines],
            0,
        );

        tree.compute_layout_with_measure(
            root,
            available(200.0, 200.0),
            |known, _, _, context, constraints| {
                let Some(context) = context else {
                    return AlgorithmSize::new(0.0, 0.0);
                };
                if let Some(constraints) = constraints {
                    *context = [0.0, 20.0, 40.0]
                        .map(|line_top| constraints.available_space(line_top, 18.0))
                        .to_vec();
                }
                AlgorithmSize::new(known.width.unwrap_or(200.0), known.height.unwrap_or(60.0))
            },
        );

        assert_eq!(
            tree.context(lines).expect("line context"),
            &[
                crate::FloatAvailableSpace {
                    inline_start: 80.0,
                    inline_size: 120.0,
                },
                crate::FloatAvailableSpace {
                    inline_start: 80.0,
                    inline_size: 120.0,
                },
                crate::FloatAvailableSpace {
                    inline_start: 0.0,
                    inline_size: 200.0,
                },
            ]
        );
        assert_eq!(tree.layout(lines).height, 60.0);
        assert_eq!(tree.layout(root).height, 60.0);
        assert_eq!(tree.block_algorithm(root), Some(BlockAlgorithm::Buckram));
    }
}
