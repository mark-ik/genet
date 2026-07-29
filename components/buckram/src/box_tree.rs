//! CSS box identity, roles, tree position, and source provenance.

use std::{
    collections::HashMap,
    hash::Hash,
    ops::{Index, IndexMut},
};

use crate::{FloatSide, FlowAxes};

/// Stable identity within one generated [`CssBoxTree`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BoxId(u32);

impl BoxId {
    /// Dense index used by diagnostics and side tables scoped to this tree.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The CSS-defined source of a generated box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoxOrigin<Id> {
    Element(Id),
    Text(Id),
    Pseudo {
        owner: Id,
        pseudo: PseudoElement,
    },
    Anonymous {
        owner: Option<Id>,
        kind: AnonymousBoxKind,
    },
}

impl<Id: Copy> BoxOrigin<Id> {
    /// DOM node whose semantics or content caused this box to be generated.
    pub fn node(self) -> Option<Id> {
        match self {
            Self::Element(node) | Self::Text(node) => Some(node),
            Self::Pseudo { owner, .. } => Some(owner),
            Self::Anonymous { owner, .. } => owner,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PseudoElement {
    Before,
    After,
    Marker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnonymousBoxKind {
    Block,
    Inline,
    Marker,
    TableWrapper,
    TableGrid,
    TableRowGroup,
    TableRow,
    TableCell,
}

/// Whether the computed display value generates a principal box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoxGeneration {
    Normal,
    None,
    Contents,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOutside {
    Block,
    Inline,
    RunIn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayInside {
    Flow,
    FlowRoot,
    Flex,
    Grid,
    Table,
    Ruby,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternalTableRole {
    /// The anonymous outer box that participates in ordinary flow.
    Wrapper,
    /// The table grid proper. A source `display: table` box keeps this role.
    Grid,
    RowGroup,
    HeaderGroup,
    FooterGroup,
    Row,
    Cell,
    ColumnGroup,
    Column,
    Caption,
}

/// Parsed display semantics before algorithm lowering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayRole {
    pub generation: BoxGeneration,
    pub outside: Option<DisplayOutside>,
    pub inside: Option<DisplayInside>,
    pub list_item: bool,
    pub internal_table: Option<InternalTableRole>,
}

impl DisplayRole {
    pub const NONE: Self = Self {
        generation: BoxGeneration::None,
        outside: None,
        inside: None,
        list_item: false,
        internal_table: None,
    };

    pub const CONTENTS: Self = Self {
        generation: BoxGeneration::Contents,
        outside: None,
        inside: None,
        list_item: false,
        internal_table: None,
    };

    pub const BLOCK_FLOW: Self = Self {
        generation: BoxGeneration::Normal,
        outside: Some(DisplayOutside::Block),
        inside: Some(DisplayInside::Flow),
        list_item: false,
        internal_table: None,
    };

    pub const INLINE_FLOW: Self = Self {
        generation: BoxGeneration::Normal,
        outside: Some(DisplayOutside::Inline),
        inside: Some(DisplayInside::Flow),
        list_item: false,
        internal_table: None,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormattingContextKind {
    Block,
    Inline,
    Flex,
    Grid,
    Table,
}

/// The formatting context that generated a floated box.
///
/// This survives blockification, inline splitting, and anonymous-box fixup.
/// It is intentionally distinct from the box's outer and inner display roles
/// and from the formatting context the generated box itself establishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatContextProvenance {
    Block,
    Inline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositioningScheme {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

impl PositioningScheme {
    fn is_in_flow(self) -> bool {
        matches!(self, Self::Static | Self::Relative | Self::Sticky)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainingBlock {
    Initial,
    Box(BoxId),
    Pending(ContainingBlockRule),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainingBlockRule {
    NormalFlow,
    AbsolutePositioned,
    FixedPositioned,
}

/// One generated CSS box.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssBox<Id> {
    pub origin: BoxOrigin<Id>,
    pub display: DisplayRole,
    pub flow: FlowAxes,
    pub positioning: PositioningScheme,
    pub float: FloatSide,
    pub float_context: FloatContextProvenance,
    pub replaced: bool,
    pub formatting_context: Option<FormattingContextKind>,
    pub containing_block: ContainingBlock,
    parent: Option<BoxId>,
    children: Vec<BoxId>,
}

impl<Id> CssBox<Id> {
    pub fn new(
        origin: BoxOrigin<Id>,
        display: DisplayRole,
        flow: FlowAxes,
        positioning: PositioningScheme,
        replaced: bool,
        formatting_context: Option<FormattingContextKind>,
        containing_block: ContainingBlock,
    ) -> Self {
        Self {
            origin,
            display,
            flow,
            positioning,
            float: FloatSide::None,
            float_context: FloatContextProvenance::Block,
            replaced,
            formatting_context,
            containing_block,
            parent: None,
            children: Vec::new(),
        }
    }

    pub fn with_float(mut self, float: FloatSide) -> Self {
        self.float = float;
        self
    }

    pub fn with_float_context(mut self, float_context: FloatContextProvenance) -> Self {
        self.float_context = float_context;
        self
    }

    pub fn parent(&self) -> Option<BoxId> {
        self.parent
    }

    pub fn children(&self) -> &[BoxId] {
        &self.children
    }
}

/// CSS-generated boxes plus their source provenance.
#[derive(Clone, Debug)]
pub struct CssBoxTree<Id> {
    boxes: Vec<CssBox<Id>>,
    roots: Vec<BoxId>,
    principal_boxes: HashMap<Id, BoxId>,
    boxes_by_node: HashMap<Id, Vec<BoxId>>,
}

/// One source item presented to CSS box generation.
///
/// The style integration resolves computed values into these semantics.
/// Buckram owns the tree transformation after that boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoxTreeInput<Id> {
    pub origin: BoxOrigin<Id>,
    pub display: DisplayRole,
    pub flow: FlowAxes,
    pub positioning: PositioningScheme,
    pub float: FloatSide,
    pub replaced: bool,
    pub collapsible_whitespace: bool,
    pub children: Vec<Self>,
}

impl<Id> BoxTreeInput<Id> {
    pub fn new(
        origin: BoxOrigin<Id>,
        display: DisplayRole,
        flow: FlowAxes,
        positioning: PositioningScheme,
        replaced: bool,
        children: Vec<Self>,
    ) -> Self {
        Self {
            origin,
            display,
            flow,
            positioning,
            float: FloatSide::None,
            replaced,
            collapsible_whitespace: false,
            children,
        }
    }

    pub fn with_float(mut self, float: FloatSide) -> Self {
        self.float = float;
        self
    }

    pub fn text(origin: BoxOrigin<Id>, flow: FlowAxes, collapsible_whitespace: bool) -> Self {
        Self {
            origin,
            display: DisplayRole::INLINE_FLOW,
            flow,
            positioning: PositioningScheme::Static,
            float: FloatSide::None,
            replaced: false,
            collapsible_whitespace,
            children: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct ProtoBox<Id> {
    origin: BoxOrigin<Id>,
    display: DisplayRole,
    flow: FlowAxes,
    positioning: PositioningScheme,
    float: FloatSide,
    float_context: FloatContextProvenance,
    replaced: bool,
    collapsible_whitespace: bool,
    principal: bool,
    formatting_context: Option<FormattingContextKind>,
    children: Vec<Self>,
}

impl<Id: Copy> ProtoBox<Id> {
    fn from_input(
        input: BoxTreeInput<Id>,
        item_child: bool,
        float_context: FloatContextProvenance,
    ) -> Self {
        let blockify_as_item = item_child && !matches!(input.origin, BoxOrigin::Text(_));
        Self {
            origin: input.origin,
            display: blockified_display(
                input.display,
                input.positioning,
                input.float,
                blockify_as_item,
            ),
            flow: input.flow,
            positioning: input.positioning,
            float: input.float,
            float_context,
            replaced: input.replaced,
            collapsible_whitespace: input.collapsible_whitespace,
            principal: !matches!(
                input.origin,
                BoxOrigin::Text(_) | BoxOrigin::Pseudo { .. } | BoxOrigin::Anonymous { .. }
            ),
            formatting_context: None,
            children: Vec::new(),
        }
    }

    fn is_in_flow_block(&self) -> bool {
        self.positioning.is_in_flow()
            && self.float == FloatSide::None
            && (self.display.outside == Some(DisplayOutside::Block)
                || matches!(
                    self.display.internal_table,
                    Some(role) if role != InternalTableRole::Wrapper
                ))
    }

    fn is_in_flow_inline(&self) -> bool {
        self.positioning.is_in_flow()
            && self.float == FloatSide::None
            && (self.display.outside == Some(DisplayOutside::Inline)
                || matches!(self.origin, BoxOrigin::Text(_)))
    }

    fn is_only_collapsible_whitespace(&self) -> bool {
        self.collapsible_whitespace
            || (!self.children.is_empty()
                && self
                    .children
                    .iter()
                    .all(Self::is_only_collapsible_whitespace))
    }

    fn owner(&self) -> Option<Id> {
        self.origin.node()
    }
}

/// Generate the CSS box tree and run the structural CSS Display fixups.
pub fn generate_box_tree<Id>(roots: impl IntoIterator<Item = BoxTreeInput<Id>>) -> CssBoxTree<Id>
where
    Id: Copy + Eq + Hash,
{
    let normalized = roots
        .into_iter()
        .flat_map(|root| normalize_input(root, false, FloatContextProvenance::Block))
        .collect::<Vec<_>>();
    let mut tree = CssBoxTree::default();
    for root in normalized {
        materialize(&mut tree, root, None);
    }
    tree
}

fn normalize_input<Id>(
    input: BoxTreeInput<Id>,
    item_child: bool,
    float_context: FloatContextProvenance,
) -> Vec<ProtoBox<Id>>
where
    Id: Copy,
{
    match input.display.generation {
        BoxGeneration::None => Vec::new(),
        BoxGeneration::Contents => input
            .children
            .into_iter()
            .flat_map(|child| normalize_input(child, item_child, float_context))
            .collect(),
        BoxGeneration::Normal => {
            let mut input = input;
            let children_are_items = matches!(
                input.display.inside,
                Some(DisplayInside::Flex | DisplayInside::Grid)
            );
            let child_float_context = child_float_context(&input, item_child);
            let children = std::mem::take(&mut input.children)
                .into_iter()
                .flat_map(|child| normalize_input(child, children_are_items, child_float_context))
                .collect::<Vec<_>>();
            let children = repair_missing_table_parents(
                children,
                input.origin.node(),
                input.flow,
                input.display,
            );
            let mut proto = ProtoBox::from_input(input, item_child, float_context);
            proto.children = children;

            if proto.display.list_item
                && let Some(owner) = proto.owner()
            {
                proto.children.insert(0, marker_box(owner, proto.flow));
            }

            if proto.display.outside == Some(DisplayOutside::Inline)
                && proto.children.iter().any(ProtoBox::is_in_flow_block)
            {
                return split_inline_around_blocks(proto);
            }

            if proto.display.inside == Some(DisplayInside::Table) {
                let outside = proto.display.outside;
                proto.display.internal_table = Some(InternalTableRole::Grid);
                fix_children(&mut proto);
                return vec![table_wrapper(proto, outside)];
            }

            fix_children(&mut proto);
            vec![proto]
        },
    }
}

fn child_float_context<Id>(input: &BoxTreeInput<Id>, item_child: bool) -> FloatContextProvenance {
    let blockify_as_item = item_child && !matches!(input.origin, BoxOrigin::Text(_));
    let display = blockified_display(
        input.display,
        input.positioning,
        input.float,
        blockify_as_item,
    );
    if input.positioning.is_in_flow()
        && input.float == FloatSide::None
        && display.outside == Some(DisplayOutside::Inline)
        && display.inside == Some(DisplayInside::Flow)
    {
        FloatContextProvenance::Inline
    } else {
        FloatContextProvenance::Block
    }
}

fn blockified_display(
    mut display: DisplayRole,
    positioning: PositioningScheme,
    float: FloatSide,
    item_child: bool,
) -> DisplayRole {
    if (item_child
        || float != FloatSide::None
        || matches!(
            positioning,
            PositioningScheme::Absolute | PositioningScheme::Fixed
        ))
        && display.outside == Some(DisplayOutside::Inline)
    {
        display.outside = Some(DisplayOutside::Block);
    }
    display
}

fn split_inline_around_blocks<Id>(mut proto: ProtoBox<Id>) -> Vec<ProtoBox<Id>>
where
    Id: Copy,
{
    let children = std::mem::take(&mut proto.children);
    let mut pieces = Vec::new();
    let mut current = proto.clone();
    current.children.clear();
    let mut first = true;

    for child in children {
        if child.is_in_flow_block() {
            current.principal = proto.principal && first;
            fix_children(&mut current);
            pieces.push(current);
            pieces.push(child);
            current = proto.clone();
            current.principal = false;
            current.children.clear();
            first = false;
        } else {
            current.children.push(child);
        }
    }

    current.principal = proto.principal && first;
    fix_children(&mut current);
    pieces.push(current);
    pieces
}

fn fix_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    if let Some(role) = proto.display.internal_table {
        match role {
            InternalTableRole::Wrapper => fix_table_wrapper_children(proto),
            InternalTableRole::Grid => {
                fix_table_children(proto);
                proto.formatting_context = Some(FormattingContextKind::Table);
            },
            InternalTableRole::RowGroup
            | InternalTableRole::HeaderGroup
            | InternalTableRole::FooterGroup => fix_row_group_children(proto),
            InternalTableRole::Row => fix_row_children(proto),
            InternalTableRole::Cell | InternalTableRole::Caption => fix_flow_children(proto),
            InternalTableRole::ColumnGroup | InternalTableRole::Column => {
                proto.formatting_context = Some(FormattingContextKind::Table);
            },
        }
        return;
    }

    match proto.display.inside {
        Some(DisplayInside::Flex) => {
            fix_item_children(proto);
            proto.formatting_context = Some(FormattingContextKind::Flex);
        },
        Some(DisplayInside::Grid) => {
            fix_item_children(proto);
            proto.formatting_context = Some(FormattingContextKind::Grid);
        },
        Some(DisplayInside::Table) => {
            fix_table_children(proto);
            proto.formatting_context = Some(FormattingContextKind::Table);
        },
        Some(DisplayInside::Flow | DisplayInside::FlowRoot) | None => fix_flow_children(proto),
        Some(DisplayInside::Ruby) => {
            proto.formatting_context = None;
        },
    }
}

/// CSS Display's missing-parent-wrapper stage. It runs after descendants have
/// been normalized, but before the surrounding non-table parent chooses its
/// own formatting context. A consecutive run of table parts receives one
/// anonymous grid and wrapper; the grid then applies the missing-child stage
/// below. Out-of-flow descendants never join that run.
fn repair_missing_table_parents<Id>(
    children: Vec<ProtoBox<Id>>,
    owner: Option<Id>,
    flow: FlowAxes,
    parent_display: DisplayRole,
) -> Vec<ProtoBox<Id>>
where
    Id: Copy,
{
    if parent_accepts_table_parts(parent_display) {
        return children;
    }

    let mut fixed = Vec::new();
    let mut table_parts = Vec::new();
    let flush = |fixed: &mut Vec<ProtoBox<Id>>, table_parts: &mut Vec<ProtoBox<Id>>| {
        if table_parts.is_empty() {
            return;
        }
        let grid = anonymous_table_box(
            owner,
            AnonymousBoxKind::TableGrid,
            InternalTableRole::Grid,
            flow,
            std::mem::take(table_parts),
        );
        fixed.push(table_wrapper(grid, Some(DisplayOutside::Block)));
    };

    for child in children {
        if child.positioning.is_in_flow() && is_table_part(child.display.internal_table) {
            table_parts.push(child);
        } else {
            flush(&mut fixed, &mut table_parts);
            fixed.push(child);
        }
    }
    flush(&mut fixed, &mut table_parts);
    fixed
}

fn parent_accepts_table_parts(display: DisplayRole) -> bool {
    display.inside == Some(DisplayInside::Table)
        || matches!(
            display.internal_table,
            Some(
                InternalTableRole::Wrapper
                    | InternalTableRole::Grid
                    | InternalTableRole::RowGroup
                    | InternalTableRole::HeaderGroup
                    | InternalTableRole::FooterGroup
                    | InternalTableRole::Row
                    | InternalTableRole::ColumnGroup
            )
        )
}

fn is_table_part(role: Option<InternalTableRole>) -> bool {
    matches!(
        role,
        Some(
            InternalTableRole::RowGroup
                | InternalTableRole::HeaderGroup
                | InternalTableRole::FooterGroup
                | InternalTableRole::Row
                | InternalTableRole::Cell
                | InternalTableRole::ColumnGroup
                | InternalTableRole::Column
                | InternalTableRole::Caption
        )
    )
}

fn fix_flow_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    let has_block = proto.children.iter().any(ProtoBox::is_in_flow_block);
    if !has_block {
        proto.formatting_context = Some(FormattingContextKind::Inline);
        return;
    }

    let owner = proto.owner();
    let children = std::mem::take(&mut proto.children);
    proto.children = wrap_inline_runs(children, owner, AnonymousBoxKind::Block, proto.flow);
    proto.formatting_context = Some(FormattingContextKind::Block);
}

fn fix_item_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    let owner = proto.owner();
    let children = std::mem::take(&mut proto.children);
    proto.children = wrap_inline_runs(children, owner, AnonymousBoxKind::Block, proto.flow);
}

fn wrap_inline_runs<Id>(
    children: Vec<ProtoBox<Id>>,
    owner: Option<Id>,
    kind: AnonymousBoxKind,
    flow: FlowAxes,
) -> Vec<ProtoBox<Id>>
where
    Id: Copy,
{
    let mut fixed = Vec::new();
    let mut run = Vec::new();
    let flush = |fixed: &mut Vec<ProtoBox<Id>>, run: &mut Vec<ProtoBox<Id>>| {
        if run.is_empty() {
            return;
        }
        if run.iter().all(ProtoBox::is_only_collapsible_whitespace) {
            run.clear();
            return;
        }
        fixed.push(anonymous_block(owner, kind, flow, std::mem::take(run)));
    };

    for child in children {
        if child.is_in_flow_inline() {
            run.push(child);
        } else {
            flush(&mut fixed, &mut run);
            fixed.push(child);
        }
    }
    flush(&mut fixed, &mut run);
    fixed
}

fn fix_table_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    let owner = proto.owner();
    let flow = proto.flow;
    let children = std::mem::take(&mut proto.children);
    let mut fixed = Vec::new();
    let mut improper = Vec::new();
    let mut cells = Vec::new();

    let flush = |fixed: &mut Vec<ProtoBox<Id>>, improper: &mut Vec<ProtoBox<Id>>| {
        if improper.is_empty() {
            return;
        }
        let cell = anonymous_table_box(
            owner,
            AnonymousBoxKind::TableCell,
            InternalTableRole::Cell,
            flow,
            std::mem::take(improper),
        );
        let row = anonymous_table_box(
            owner,
            AnonymousBoxKind::TableRow,
            InternalTableRole::Row,
            flow,
            vec![cell],
        );
        fixed.push(anonymous_table_box(
            owner,
            AnonymousBoxKind::TableRowGroup,
            InternalTableRole::RowGroup,
            flow,
            vec![row],
        ));
    };
    let flush_cells = |fixed: &mut Vec<ProtoBox<Id>>, cells: &mut Vec<ProtoBox<Id>>| {
        if cells.is_empty() {
            return;
        }
        let row = anonymous_table_box(
            owner,
            AnonymousBoxKind::TableRow,
            InternalTableRole::Row,
            flow,
            std::mem::take(cells),
        );
        fixed.push(anonymous_table_box(
            owner,
            AnonymousBoxKind::TableRowGroup,
            InternalTableRole::RowGroup,
            flow,
            vec![row],
        ));
    };

    for child in children {
        if child.is_only_collapsible_whitespace() {
            continue;
        }
        if !child.positioning.is_in_flow() {
            flush(&mut fixed, &mut improper);
            flush_cells(&mut fixed, &mut cells);
            fixed.push(child);
            continue;
        }
        if matches!(
            child.display.internal_table,
            Some(
                InternalTableRole::RowGroup
                    | InternalTableRole::HeaderGroup
                    | InternalTableRole::FooterGroup
                    | InternalTableRole::Caption
                    | InternalTableRole::ColumnGroup
                    | InternalTableRole::Column
            )
        ) {
            flush(&mut fixed, &mut improper);
            flush_cells(&mut fixed, &mut cells);
            fixed.push(child);
        } else if child.display.internal_table == Some(InternalTableRole::Row) {
            flush(&mut fixed, &mut improper);
            flush_cells(&mut fixed, &mut cells);
            fixed.push(anonymous_table_box(
                owner,
                AnonymousBoxKind::TableRowGroup,
                InternalTableRole::RowGroup,
                flow,
                vec![child],
            ));
        } else if child.display.internal_table == Some(InternalTableRole::Cell) {
            flush(&mut fixed, &mut improper);
            cells.push(child);
        } else {
            flush_cells(&mut fixed, &mut cells);
            improper.push(child);
        }
    }
    flush(&mut fixed, &mut improper);
    flush_cells(&mut fixed, &mut cells);
    proto.children = fixed;
}

/// A table wrapper owns captions and ordinary flow; its grid remains a
/// distinct child with the source table's principal provenance.
fn fix_table_wrapper_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    proto.formatting_context = Some(FormattingContextKind::Block);
}

fn fix_row_group_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    let owner = proto.owner();
    let flow = proto.flow;
    let children = std::mem::take(&mut proto.children);
    let mut fixed = Vec::new();
    let mut improper = Vec::new();
    let flush = |fixed: &mut Vec<ProtoBox<Id>>, improper: &mut Vec<ProtoBox<Id>>| {
        if improper.is_empty() {
            return;
        }
        fixed.push(anonymous_table_box(
            owner,
            AnonymousBoxKind::TableRow,
            InternalTableRole::Row,
            flow,
            std::mem::take(improper),
        ));
    };

    for child in children {
        if child.is_only_collapsible_whitespace() {
            continue;
        }
        if !child.positioning.is_in_flow() {
            flush(&mut fixed, &mut improper);
            fixed.push(child);
            continue;
        }
        if child.display.internal_table == Some(InternalTableRole::Row) {
            flush(&mut fixed, &mut improper);
            fixed.push(child);
        } else {
            improper.push(child);
        }
    }
    flush(&mut fixed, &mut improper);
    proto.children = fixed;
    proto.formatting_context = Some(FormattingContextKind::Table);
}

fn fix_row_children<Id>(proto: &mut ProtoBox<Id>)
where
    Id: Copy,
{
    let owner = proto.owner();
    let flow = proto.flow;
    let children = std::mem::take(&mut proto.children);
    proto.children = children
        .into_iter()
        .filter(|child| !child.is_only_collapsible_whitespace())
        .map(|child| {
            if !child.positioning.is_in_flow()
                || child.display.internal_table == Some(InternalTableRole::Cell)
            {
                child
            } else {
                anonymous_table_box(
                    owner,
                    AnonymousBoxKind::TableCell,
                    InternalTableRole::Cell,
                    flow,
                    vec![child],
                )
            }
        })
        .collect();
    proto.formatting_context = Some(FormattingContextKind::Table);
}

fn marker_box<Id>(owner: Id, flow: FlowAxes) -> ProtoBox<Id>
where
    Id: Copy,
{
    ProtoBox {
        origin: BoxOrigin::Pseudo {
            owner,
            pseudo: PseudoElement::Marker,
        },
        display: DisplayRole::INLINE_FLOW,
        flow,
        positioning: PositioningScheme::Static,
        float: FloatSide::None,
        float_context: FloatContextProvenance::Block,
        replaced: false,
        collapsible_whitespace: false,
        principal: false,
        formatting_context: None,
        children: Vec::new(),
    }
}

fn anonymous_block<Id>(
    owner: Option<Id>,
    kind: AnonymousBoxKind,
    flow: FlowAxes,
    children: Vec<ProtoBox<Id>>,
) -> ProtoBox<Id>
where
    Id: Copy,
{
    ProtoBox {
        origin: BoxOrigin::Anonymous { owner, kind },
        display: DisplayRole::BLOCK_FLOW,
        flow,
        positioning: PositioningScheme::Static,
        float: FloatSide::None,
        float_context: FloatContextProvenance::Block,
        replaced: false,
        collapsible_whitespace: false,
        principal: false,
        formatting_context: Some(FormattingContextKind::Inline),
        children,
    }
}

fn anonymous_table_box<Id>(
    owner: Option<Id>,
    kind: AnonymousBoxKind,
    role: InternalTableRole,
    flow: FlowAxes,
    children: Vec<ProtoBox<Id>>,
) -> ProtoBox<Id>
where
    Id: Copy,
{
    let mut proto = ProtoBox {
        origin: BoxOrigin::Anonymous { owner, kind },
        display: DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(role),
        },
        flow,
        positioning: PositioningScheme::Static,
        float: FloatSide::None,
        float_context: FloatContextProvenance::Block,
        replaced: false,
        collapsible_whitespace: false,
        principal: false,
        formatting_context: Some(FormattingContextKind::Table),
        children,
    };
    fix_children(&mut proto);
    proto
}

/// Wrap one grid in the anonymous outer box CSS 2.1 assigns to a table root.
/// Captions are siblings of the grid inside that wrapper. Their eventual
/// top/bottom placement belongs to K4e; retaining them here prevents later
/// table topology from inventing a second wrapper identity.
fn table_wrapper<Id>(mut grid: ProtoBox<Id>, outside: Option<DisplayOutside>) -> ProtoBox<Id>
where
    Id: Copy,
{
    let owner = grid.owner();
    let mut captions = Vec::new();
    grid.children.retain(|child| {
        if child.display.internal_table == Some(InternalTableRole::Caption) {
            captions.push(child.clone());
            false
        } else {
            true
        }
    });
    let positioning = std::mem::replace(&mut grid.positioning, PositioningScheme::Static);
    let float = std::mem::replace(&mut grid.float, FloatSide::None);
    let float_context = grid.float_context;
    grid.float_context = FloatContextProvenance::Block;

    captions.push(grid);
    ProtoBox {
        origin: BoxOrigin::Anonymous {
            owner,
            kind: AnonymousBoxKind::TableWrapper,
        },
        display: DisplayRole {
            generation: BoxGeneration::Normal,
            outside,
            inside: Some(DisplayInside::Flow),
            list_item: false,
            internal_table: Some(InternalTableRole::Wrapper),
        },
        flow: captions
            .last()
            .map(|grid| grid.flow)
            .unwrap_or(FlowAxes::HORIZONTAL_LTR),
        positioning,
        float,
        float_context,
        replaced: false,
        collapsible_whitespace: false,
        principal: false,
        formatting_context: Some(FormattingContextKind::Block),
        children: captions,
    }
}

fn materialize<Id>(tree: &mut CssBoxTree<Id>, proto: ProtoBox<Id>, parent: Option<BoxId>) -> BoxId
where
    Id: Copy + Eq + Hash,
{
    let children = proto.children;
    let id = tree.push(
        CssBox::new(
            proto.origin,
            proto.display,
            proto.flow,
            proto.positioning,
            proto.replaced,
            proto.formatting_context,
            parent.map_or(ContainingBlock::Initial, |_| {
                ContainingBlock::Pending(match proto.positioning {
                    PositioningScheme::Absolute => ContainingBlockRule::AbsolutePositioned,
                    PositioningScheme::Fixed => ContainingBlockRule::FixedPositioned,
                    _ => ContainingBlockRule::NormalFlow,
                })
            }),
        )
        .with_float_context(proto.float_context)
        .with_float(proto.float),
        parent,
        proto.principal,
    );
    for child in children {
        materialize(tree, child, Some(id));
    }
    id
}

impl<Id> Default for CssBoxTree<Id> {
    fn default() -> Self {
        Self {
            boxes: Vec::new(),
            roots: Vec::new(),
            principal_boxes: HashMap::new(),
            boxes_by_node: HashMap::new(),
        }
    }
}

impl<Id> CssBoxTree<Id>
where
    Id: Copy + Eq + Hash,
{
    pub fn roots(&self) -> &[BoxId] {
        &self.roots
    }

    pub fn principal_box(&self, node: Id) -> Option<BoxId> {
        self.principal_boxes.get(&node).copied()
    }

    pub fn boxes_for_node(&self, node: Id) -> &[BoxId] {
        self.boxes_by_node
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn origin_node(&self, box_id: BoxId) -> Option<Id> {
        self[box_id].origin.node()
    }

    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (BoxId, &CssBox<Id>)> {
        self.boxes
            .iter()
            .enumerate()
            .map(|(index, css_box)| (BoxId(index as u32), css_box))
    }

    /// Add one generated box.
    ///
    /// Box generation is owned by the integrating style engine until Buckram
    /// owns the complete CSS Display fixup algorithm.
    pub fn push(
        &mut self,
        mut css_box: CssBox<Id>,
        parent: Option<BoxId>,
        principal: bool,
    ) -> BoxId {
        let id = BoxId(
            self.boxes
                .len()
                .try_into()
                .expect("a CSS box tree exceeded u32::MAX boxes"),
        );
        css_box.parent = parent;
        self.boxes.push(css_box);

        if let Some(parent) = parent {
            self[parent].children.push(id);
        } else {
            self.roots.push(id);
        }

        if let Some(node) = self[id].origin.node() {
            self.boxes_by_node.entry(node).or_default().push(id);
            if principal {
                let previous = self.principal_boxes.insert(node, id);
                assert!(
                    previous.is_none(),
                    "a source node cannot have two principal boxes"
                );
            }
        } else {
            assert!(!principal, "a principal box must have source provenance");
        }
        id
    }
}

impl<Id> Index<BoxId> for CssBoxTree<Id> {
    type Output = CssBox<Id>;

    fn index(&self, id: BoxId) -> &Self::Output {
        &self.boxes[id.index()]
    }
}

impl<Id> IndexMut<BoxId> for CssBoxTree<Id> {
    fn index_mut(&mut self, id: BoxId) -> &mut Self::Output {
        &mut self.boxes[id.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        origin: BoxOrigin<u8>,
        display: DisplayRole,
        children: Vec<BoxTreeInput<u8>>,
    ) -> BoxTreeInput<u8> {
        BoxTreeInput::new(
            origin,
            display,
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            children,
        )
    }

    fn text(node: u8, whitespace: bool) -> BoxTreeInput<u8> {
        BoxTreeInput::text(BoxOrigin::Text(node), FlowAxes::HORIZONTAL_LTR, whitespace)
    }

    fn box_for(
        origin: BoxOrigin<u8>,
        display: DisplayRole,
        containing_block: ContainingBlock,
    ) -> CssBox<u8> {
        CssBox::new(
            origin,
            display,
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            None,
            containing_block,
        )
    }

    #[test]
    fn split_inline_keeps_one_principal_and_all_box_provenance() {
        let mut tree = CssBoxTree::default();
        let first = tree.push(
            box_for(
                BoxOrigin::Element(1),
                DisplayRole::INLINE_FLOW,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let block = tree.push(
            box_for(
                BoxOrigin::Element(2),
                DisplayRole::BLOCK_FLOW,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let second = tree.push(
            box_for(
                BoxOrigin::Element(1),
                DisplayRole::INLINE_FLOW,
                ContainingBlock::Initial,
            ),
            None,
            false,
        );

        assert_eq!(tree.principal_box(1), Some(first));
        assert_eq!(tree.boxes_for_node(1), &[first, second]);
        assert_eq!(tree.origin_node(block), Some(2));
    }

    #[test]
    fn anonymous_table_fixup_remains_traceable_to_its_owner() {
        let mut tree = CssBoxTree::default();
        let wrapper = tree.push(
            box_for(
                BoxOrigin::Anonymous {
                    owner: Some(7),
                    kind: AnonymousBoxKind::TableWrapper,
                },
                DisplayRole::BLOCK_FLOW,
                ContainingBlock::Initial,
            ),
            None,
            false,
        );
        let row = tree.push(
            box_for(
                BoxOrigin::Anonymous {
                    owner: Some(7),
                    kind: AnonymousBoxKind::TableRow,
                },
                DisplayRole {
                    generation: BoxGeneration::Normal,
                    outside: None,
                    inside: None,
                    list_item: false,
                    internal_table: Some(InternalTableRole::Row),
                },
                ContainingBlock::Box(wrapper),
            ),
            Some(wrapper),
            false,
        );

        assert_eq!(tree[wrapper].children(), &[row]);
        assert_eq!(tree.boxes_for_node(7), &[wrapper, row]);
        assert_eq!(tree.origin_node(row), Some(7));
    }

    #[test]
    fn none_and_contents_are_distinct_suppression_states() {
        assert_eq!(DisplayRole::NONE.generation, BoxGeneration::None);
        assert_eq!(DisplayRole::CONTENTS.generation, BoxGeneration::Contents);
        assert_ne!(DisplayRole::NONE, DisplayRole::CONTENTS);
    }

    #[test]
    fn pseudo_and_replaced_boxes_keep_their_semantics() {
        let mut tree = CssBoxTree::default();
        let pseudo = tree.push(
            box_for(
                BoxOrigin::Pseudo {
                    owner: 3,
                    pseudo: PseudoElement::Marker,
                },
                DisplayRole::INLINE_FLOW,
                ContainingBlock::Initial,
            ),
            None,
            false,
        );
        let image = tree.push(
            CssBox::new(
                BoxOrigin::Element(4),
                DisplayRole::INLINE_FLOW,
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                true,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );

        assert_eq!(tree.origin_node(pseudo), Some(3));
        assert!(tree[image].replaced);
        assert_eq!(tree.principal_box(4), Some(image));
    }

    #[test]
    fn generated_before_and_after_boxes_keep_pseudo_origin() {
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![
                input(
                    BoxOrigin::Pseudo {
                        owner: 1,
                        pseudo: PseudoElement::Before,
                    },
                    DisplayRole::INLINE_FLOW,
                    Vec::new(),
                ),
                input(
                    BoxOrigin::Pseudo {
                        owner: 1,
                        pseudo: PseudoElement::After,
                    },
                    DisplayRole::INLINE_FLOW,
                    Vec::new(),
                ),
            ],
        )]);
        let parent = tree.principal_box(1).expect("parent");
        let children = tree[parent].children();

        assert_eq!(children.len(), 2);
        assert_eq!(
            tree[children[0]].origin,
            BoxOrigin::Pseudo {
                owner: 1,
                pseudo: PseudoElement::Before,
            }
        );
        assert_eq!(
            tree[children[1]].origin,
            BoxOrigin::Pseudo {
                owner: 1,
                pseudo: PseudoElement::After,
            }
        );
    }

    #[test]
    fn mixed_flow_children_receive_anonymous_block_wrappers() {
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![
                text(2, false),
                input(BoxOrigin::Element(3), DisplayRole::BLOCK_FLOW, Vec::new()),
                text(4, false),
            ],
        )]);
        let parent = tree.principal_box(1).expect("parent");
        let children = tree[parent].children();

        assert_eq!(children.len(), 3);
        assert!(matches!(
            tree[children[0]].origin,
            BoxOrigin::Anonymous {
                kind: AnonymousBoxKind::Block,
                ..
            }
        ));
        assert_eq!(tree[children[0]].children().len(), 1);
        assert_eq!(tree.origin_node(tree[children[0]].children()[0]), Some(2));
        assert_eq!(tree.origin_node(children[1]), Some(3));
        assert!(matches!(
            tree[children[2]].origin,
            BoxOrigin::Anonymous {
                kind: AnonymousBoxKind::Block,
                ..
            }
        ));
    }

    #[test]
    fn inline_with_block_child_splits_into_continuation_boxes() {
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![input(
                BoxOrigin::Element(2),
                DisplayRole::INLINE_FLOW,
                vec![
                    text(3, false),
                    input(BoxOrigin::Element(4), DisplayRole::BLOCK_FLOW, Vec::new()),
                    text(5, false),
                ],
            )],
        )]);
        let split = tree.boxes_for_node(2);

        assert_eq!(split.len(), 2);
        assert_eq!(tree.principal_box(2), Some(split[0]));
        assert_ne!(
            tree[split[0]].parent(),
            tree[tree.principal_box(4).unwrap()].parent()
        );
        assert_eq!(tree[split[0]].children().len(), 1);
        assert_eq!(tree[split[1]].children().len(), 1);
    }

    #[test]
    fn float_context_provenance_survives_inline_splitting_and_anonymous_fixup() {
        let inline_float = input(BoxOrigin::Element(3), DisplayRole::INLINE_FLOW, Vec::new())
            .with_float(FloatSide::Left);
        let block_float = input(BoxOrigin::Element(5), DisplayRole::INLINE_FLOW, Vec::new())
            .with_float(FloatSide::Left);
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![input(
                BoxOrigin::Element(2),
                DisplayRole::INLINE_FLOW,
                vec![
                    text(6, false),
                    inline_float,
                    input(
                        BoxOrigin::Element(4),
                        DisplayRole::BLOCK_FLOW,
                        vec![block_float],
                    ),
                    text(7, false),
                ],
            )],
        )]);
        let continuations = tree.boxes_for_node(2);
        let inline_float = tree.principal_box(3).expect("inline float");
        let block_float = tree.principal_box(5).expect("block float");

        assert_eq!(continuations.len(), 2);
        for continuation in continuations {
            let wrapper = tree[*continuation].parent().expect("anonymous wrapper");
            assert!(matches!(
                tree[wrapper].origin,
                BoxOrigin::Anonymous {
                    kind: AnonymousBoxKind::Block,
                    ..
                }
            ));
        }
        assert_eq!(
            tree[inline_float].float_context,
            FloatContextProvenance::Inline
        );
        assert_eq!(
            tree[block_float].float_context,
            FloatContextProvenance::Block
        );
    }

    #[test]
    fn contents_flattens_children_while_none_suppresses_its_subtree() {
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![
                input(
                    BoxOrigin::Element(2),
                    DisplayRole::CONTENTS,
                    vec![input(
                        BoxOrigin::Element(3),
                        DisplayRole::BLOCK_FLOW,
                        Vec::new(),
                    )],
                ),
                input(
                    BoxOrigin::Element(4),
                    DisplayRole::NONE,
                    vec![input(
                        BoxOrigin::Element(5),
                        DisplayRole::BLOCK_FLOW,
                        Vec::new(),
                    )],
                ),
            ],
        )]);
        let parent = tree.principal_box(1).expect("parent");

        assert_eq!(tree.principal_box(2), None);
        assert_eq!(tree.principal_box(4), None);
        assert_eq!(tree.principal_box(5), None);
        assert_eq!(tree[tree.principal_box(3).unwrap()].parent(), Some(parent));
    }

    #[test]
    fn list_item_generates_a_marker_with_owner_provenance() {
        let mut list_item = DisplayRole::BLOCK_FLOW;
        list_item.list_item = true;
        let tree = generate_box_tree([input(
            BoxOrigin::Element(7),
            list_item,
            vec![text(8, false)],
        )]);
        let item = tree.principal_box(7).expect("list item");
        let marker = tree[item].children()[0];

        assert_eq!(
            tree[marker].origin,
            BoxOrigin::Pseudo {
                owner: 7,
                pseudo: PseudoElement::Marker,
            }
        );
    }

    #[test]
    fn floated_inline_is_blockified_without_becoming_in_flow_block_content() {
        let floated = input(
            BoxOrigin::Element(2),
            DisplayRole::INLINE_FLOW,
            vec![text(3, false)],
        )
        .with_float(FloatSide::Left);
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![text(4, false), floated, text(5, false)],
        )]);
        let parent = tree.principal_box(1).expect("parent");
        let float = tree.principal_box(2).expect("float");

        assert_eq!(tree[float].display.outside, Some(DisplayOutside::Block));
        assert_eq!(tree[float].float, FloatSide::Left);
        assert_eq!(
            tree[parent].formatting_context,
            Some(FormattingContextKind::Inline),
            "an out-of-flow float does not turn its parent's inline content into in-flow blocks"
        );
    }

    #[test]
    fn flex_blockifies_each_element_and_wraps_only_text_runs() {
        let flex = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: Some(DisplayOutside::Block),
            inside: Some(DisplayInside::Flex),
            list_item: false,
            internal_table: None,
        };
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            flex,
            vec![
                input(BoxOrigin::Element(2), DisplayRole::INLINE_FLOW, Vec::new()),
                input(BoxOrigin::Element(3), DisplayRole::INLINE_FLOW, Vec::new()),
                text(4, false),
                text(5, false),
                text(6, true),
            ],
        )]);
        let flex = tree.principal_box(1).expect("flex container");
        let children = tree[flex].children();

        assert_eq!(children.len(), 3);
        assert_eq!(tree.origin_node(children[0]), Some(2));
        assert_eq!(tree.origin_node(children[1]), Some(3));
        assert_eq!(
            tree[children[0]].display.outside,
            Some(DisplayOutside::Block)
        );
        assert_eq!(
            tree[children[1]].display.outside,
            Some(DisplayOutside::Block)
        );
        assert!(matches!(
            tree[children[2]].origin,
            BoxOrigin::Anonymous {
                kind: AnonymousBoxKind::Block,
                ..
            }
        ));
        assert_eq!(tree[children[2]].children().len(), 3);
    }

    #[test]
    fn table_fixup_inserts_missing_row_group_row_and_cell() {
        let table = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: Some(DisplayOutside::Block),
            inside: Some(DisplayInside::Table),
            list_item: false,
            internal_table: None,
        };
        let tree = generate_box_tree([input(BoxOrigin::Element(1), table, vec![text(2, false)])]);
        let table = tree.principal_box(1).expect("table");
        let group = tree[table].children()[0];
        let row = tree[group].children()[0];
        let cell = tree[row].children()[0];

        assert_eq!(
            tree[group].display.internal_table,
            Some(InternalTableRole::RowGroup)
        );
        assert_eq!(
            tree[row].display.internal_table,
            Some(InternalTableRole::Row)
        );
        assert_eq!(
            tree[cell].display.internal_table,
            Some(InternalTableRole::Cell)
        );
        assert_eq!(tree[cell].children().len(), 1);
    }

    #[test]
    fn table_root_has_distinct_wrapper_and_grid_provenance() {
        let table = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: Some(DisplayOutside::Block),
            inside: Some(DisplayInside::Table),
            list_item: false,
            internal_table: None,
        };
        let caption = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(InternalTableRole::Caption),
        };
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            table,
            vec![
                input(BoxOrigin::Element(2), caption, Vec::new()),
                input(
                    BoxOrigin::Element(3),
                    DisplayRole {
                        generation: BoxGeneration::Normal,
                        outside: None,
                        inside: None,
                        list_item: false,
                        internal_table: Some(InternalTableRole::Row),
                    },
                    Vec::new(),
                ),
            ],
        )]);
        let grid = tree.principal_box(1).expect("source table grid");
        let wrapper = tree[grid].parent().expect("anonymous table wrapper");

        assert_eq!(
            tree[wrapper].origin,
            BoxOrigin::Anonymous {
                owner: Some(1),
                kind: AnonymousBoxKind::TableWrapper,
            }
        );
        assert_eq!(
            tree[wrapper].display.internal_table,
            Some(InternalTableRole::Wrapper)
        );
        assert_eq!(tree[grid].origin, BoxOrigin::Element(1));
        assert_eq!(
            tree[grid].display.internal_table,
            Some(InternalTableRole::Grid)
        );
        assert_eq!(tree[wrapper].children().len(), 2);
        assert_eq!(tree.origin_node(tree[wrapper].children()[0]), Some(2));
        assert_eq!(tree[wrapper].children()[1], grid);
        assert_eq!(
            tree[tree[grid].children()[0]].display.internal_table,
            Some(InternalTableRole::RowGroup)
        );
    }

    #[test]
    fn missing_table_parents_wrap_a_cell_run_once() {
        let cell = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(InternalTableRole::Cell),
        };
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            DisplayRole::BLOCK_FLOW,
            vec![
                input(BoxOrigin::Element(2), cell, Vec::new()),
                input(BoxOrigin::Element(3), cell, Vec::new()),
            ],
        )]);
        let parent = tree.principal_box(1).expect("flow parent");
        let wrapper = tree[parent].children()[0];
        let grid = tree[wrapper].children()[0];
        let group = tree[grid].children()[0];
        let row = tree[group].children()[0];

        assert_eq!(
            tree[wrapper].display.internal_table,
            Some(InternalTableRole::Wrapper)
        );
        assert_eq!(
            tree[grid].display.internal_table,
            Some(InternalTableRole::Grid)
        );
        assert_eq!(
            tree[group].display.internal_table,
            Some(InternalTableRole::RowGroup)
        );
        assert_eq!(tree[row].children().len(), 2);
        assert_eq!(tree.origin_node(tree[row].children()[0]), Some(2));
        assert_eq!(tree.origin_node(tree[row].children()[1]), Some(3));
    }

    #[test]
    fn table_fixup_discards_whitespace_and_preserves_out_of_flow_children() {
        let table = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: Some(DisplayOutside::Block),
            inside: Some(DisplayInside::Table),
            list_item: false,
            internal_table: None,
        };
        let cell = DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(InternalTableRole::Cell),
        };
        let mut out_of_flow = input(BoxOrigin::Element(3), cell, Vec::new());
        out_of_flow.positioning = PositioningScheme::Absolute;
        let tree = generate_box_tree([input(
            BoxOrigin::Element(1),
            table,
            vec![
                text(2, true),
                out_of_flow,
                input(BoxOrigin::Element(4), DisplayRole::BLOCK_FLOW, Vec::new()),
            ],
        )]);
        let grid = tree.principal_box(1).expect("table grid");
        let children = tree[grid].children();

        assert_eq!(children.len(), 2);
        assert_eq!(tree.origin_node(children[0]), Some(3));
        assert_eq!(tree[children[0]].positioning, PositioningScheme::Absolute);
        let group = children[1];
        let row = tree[group].children()[0];
        let generated_cell = tree[row].children()[0];
        assert!(matches!(
            tree[generated_cell].origin,
            BoxOrigin::Anonymous {
                kind: AnonymousBoxKind::TableCell,
                ..
            }
        ));
    }
}
