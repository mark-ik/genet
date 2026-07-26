//! CSS box identity, roles, tree position, and source provenance.

use std::{
    collections::HashMap,
    hash::Hash,
    ops::{Index, IndexMut},
};

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
    TableWrapper,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositioningScheme {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
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
    pub positioning: PositioningScheme,
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
        positioning: PositioningScheme,
        replaced: bool,
        formatting_context: Option<FormattingContextKind>,
        containing_block: ContainingBlock,
    ) -> Self {
        Self {
            origin,
            display,
            positioning,
            replaced,
            formatting_context,
            containing_block,
            parent: None,
            children: Vec::new(),
        }
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

    fn box_for(
        origin: BoxOrigin<u8>,
        display: DisplayRole,
        containing_block: ContainingBlock,
    ) -> CssBox<u8> {
        CssBox::new(
            origin,
            display,
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
}
