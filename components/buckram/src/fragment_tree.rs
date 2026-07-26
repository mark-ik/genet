//! One-to-many CSS layout fragments and their tree relationships.

use std::{collections::HashMap, hash::Hash, ops::Deref};

use crate::{BoxId, CssBoxTree};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Geometry in the fragment's inline and block axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRect {
    pub inline_start: f32,
    pub block_start: f32,
    pub inline_size: f32,
    pub block_size: f32,
}

impl LogicalRect {
    /// K0 migration from the lane's current horizontal physical geometry.
    ///
    /// K3 replaces this compatibility constructor with writing-mode-aware
    /// logical layout at the algorithm boundary.
    pub fn from_horizontal_physical(rect: PhysicalRect) -> Self {
        Self {
            inline_start: rect.x,
            block_start: rect.y,
            inline_size: rect.width,
            block_size: rect.height,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FragmentId(u32);

impl FragmentId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FragmentationContextId(u32);

impl FragmentationContextId {
    /// The unfragmented root context used during K0.
    pub const INITIAL: Self = Self(0);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BreakToken {
    /// Opaque algorithm-owned continuation position.
    pub resume_at: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Baselines {
    pub first: Option<f32>,
    pub last: Option<f32>,
}

/// One fragment produced by one CSS box.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    id: FragmentId,
    box_id: BoxId,
    parent: Option<FragmentId>,
    containing_fragment: Option<FragmentId>,
    fragmentation_context: FragmentationContextId,
    pub logical_rect: LogicalRect,
    pub continuation: Option<BreakToken>,
    pub baselines: Baselines,
    pub overflow: LogicalRect,
    physical_rect: PhysicalRect,
}

impl Fragment {
    /// Construct a K0 fragment from the lane's current horizontal geometry.
    pub fn from_horizontal_physical(box_id: BoxId, rect: PhysicalRect) -> Self {
        let logical_rect = LogicalRect::from_horizontal_physical(rect);
        Self {
            id: FragmentId(u32::MAX),
            box_id,
            parent: None,
            containing_fragment: None,
            fragmentation_context: FragmentationContextId::INITIAL,
            logical_rect,
            continuation: None,
            baselines: Baselines::default(),
            overflow: logical_rect,
            physical_rect: rect,
        }
    }

    pub fn id(&self) -> FragmentId {
        self.id
    }

    pub fn box_id(&self) -> BoxId {
        self.box_id
    }

    pub fn parent(&self) -> Option<FragmentId> {
        self.parent
    }

    pub fn containing_fragment(&self) -> Option<FragmentId> {
        self.containing_fragment
    }

    pub fn fragmentation_context(&self) -> FragmentationContextId {
        self.fragmentation_context
    }

    pub fn physical_rect(&self) -> PhysicalRect {
        self.physical_rect
    }
}

/// K0 compatibility for physical consumers. The fragment tree still owns the
/// fragment and its logical geometry.
impl Deref for Fragment {
    type Target = PhysicalRect;

    fn deref(&self) -> &Self::Target {
        &self.physical_rect
    }
}

/// Fragments in tree order, indexed independently by box identity.
#[derive(Clone, Debug, Default)]
pub struct FragmentTree {
    roots: Vec<FragmentId>,
    fragments: Vec<Fragment>,
    by_box: HashMap<BoxId, Vec<FragmentId>>,
}

impl FragmentTree {
    pub fn roots(&self) -> &[FragmentId] {
        &self.roots
    }

    pub fn get(&self, id: FragmentId) -> Option<&Fragment> {
        self.fragments.get(id.index())
    }

    pub fn fragments_for_box(&self, box_id: BoxId) -> impl Iterator<Item = &Fragment> {
        self.by_box
            .get(&box_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.get(*id))
    }

    pub fn fragment_ids_for_box(&self, box_id: BoxId) -> &[FragmentId] {
        self.by_box.get(&box_id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    pub fn push(
        &mut self,
        mut fragment: Fragment,
        parent: Option<FragmentId>,
        containing_fragment: Option<FragmentId>,
    ) -> FragmentId {
        let id = FragmentId(
            self.fragments
                .len()
                .try_into()
                .expect("a fragment tree exceeded u32::MAX fragments"),
        );
        fragment.id = id;
        fragment.parent = parent;
        fragment.containing_fragment = containing_fragment;
        let box_id = fragment.box_id;
        self.fragments.push(fragment);
        self.by_box.entry(box_id).or_default().push(id);
        if parent.is_none() {
            self.roots.push(id);
        }
        id
    }
}

/// The standards-owned result of one layout pass.
#[derive(Clone, Debug)]
pub struct LayoutResult<Id> {
    boxes: CssBoxTree<Id>,
    fragments: FragmentTree,
}

impl<Id> LayoutResult<Id>
where
    Id: Copy + Eq + Hash,
{
    pub fn new(boxes: CssBoxTree<Id>, fragments: FragmentTree) -> Self {
        Self { boxes, fragments }
    }

    pub fn boxes(&self) -> &CssBoxTree<Id> {
        &self.boxes
    }

    pub fn fragments(&self) -> &FragmentTree {
        &self.fragments
    }

    pub fn fragment_ids_for_node(&self, node: Id) -> Vec<FragmentId> {
        self.boxes
            .boxes_for_node(node)
            .iter()
            .flat_map(|box_id| self.fragments.fragment_ids_for_box(*box_id))
            .copied()
            .collect()
    }

    pub fn fragments_for_node(&self, node: Id) -> impl Iterator<Item = &Fragment> {
        self.boxes
            .boxes_for_node(node)
            .iter()
            .flat_map(|box_id| self.fragments.fragments_for_box(*box_id))
    }

    /// Compatibility lookup for current single-rectangle consumers.
    ///
    /// New fragment-aware consumers use [`Self::fragments_for_node`].
    pub fn get(&self, node: Id) -> Option<&Fragment> {
        self.fragments_for_node(node).next()
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoxOrigin, ContainingBlock, CssBox, DisplayRole, PositioningScheme};

    #[test]
    fn one_box_owns_many_tree_fragments() {
        let mut boxes = CssBoxTree::default();
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::INLINE_FLOW,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let mut fragments = FragmentTree::default();
        let first = fragments.push(
            Fragment::from_horizontal_physical(
                box_id,
                PhysicalRect {
                    width: 20.0,
                    height: 10.0,
                    ..PhysicalRect::default()
                },
            ),
            None,
            None,
        );
        let second = fragments.push(
            Fragment::from_horizontal_physical(
                box_id,
                PhysicalRect {
                    x: 20.0,
                    width: 30.0,
                    height: 10.0,
                    ..PhysicalRect::default()
                },
            ),
            None,
            None,
        );
        let layout = LayoutResult::new(boxes, fragments);

        assert_eq!(layout.fragment_ids_for_node(1), vec![first, second]);
        assert_eq!(layout.fragments_for_node(1).count(), 2);
        assert_eq!(layout.get(1).map(Fragment::id), Some(first));
    }

    #[test]
    fn fragment_tree_records_parent_and_containing_fragment() {
        let mut boxes = CssBoxTree::default();
        let parent_box = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::BLOCK_FLOW,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let child_box = boxes.push(
            CssBox::new(
                BoxOrigin::Element(2u8),
                DisplayRole::BLOCK_FLOW,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Box(parent_box),
            ),
            Some(parent_box),
            true,
        );
        let mut fragments = FragmentTree::default();
        let parent = fragments.push(
            Fragment::from_horizontal_physical(parent_box, PhysicalRect::default()),
            None,
            None,
        );
        let child = fragments.push(
            Fragment::from_horizontal_physical(child_box, PhysicalRect::default()),
            Some(parent),
            Some(parent),
        );

        assert_eq!(
            fragments.get(child).and_then(Fragment::parent),
            Some(parent)
        );
        assert_eq!(
            fragments.get(child).and_then(Fragment::containing_fragment),
            Some(parent)
        );
        assert_eq!(fragments.roots(), &[parent]);
    }
}
