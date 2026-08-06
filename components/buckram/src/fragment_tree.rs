//! One-to-many CSS layout fragments and their tree relationships.

use std::{collections::HashMap, hash::Hash, ops::Deref};

use crate::{BoxId, CssBoxTree, FlowAxes, LogicalRect, PhysicalRect, PhysicalSize};

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
    /// Distance from this fragment's logical block-start edge to its first
    /// baseline.
    pub first: Option<f32>,
    /// Distance from this fragment's logical block-start edge to its last
    /// baseline.
    pub last: Option<f32>,
}

impl Baselines {
    /// Construct finite baseline offsets in the fragment's own logical flow.
    ///
    /// A baseline offset may be negative: a negative margin or offset can
    /// place a child's baseline above this fragment's block-start edge
    /// (WPT `css/CSS2/css21-errata/s-11-1-1b-005.html` does exactly this with
    /// a `margin-top: -15px` table cell). Only a non-finite offset is
    /// invalid.
    pub fn new(first: Option<f32>, last: Option<f32>) -> Option<Self> {
        [first, last]
            .into_iter()
            .flatten()
            .all(f32::is_finite)
            .then_some(Self { first, last })
    }

    /// The synthesized baseline for a formatting context with no line-box
    /// baseline. CSS uses the block-end edge for that fallback in this
    /// unfragmented lane.
    pub fn synthesized_from_block_end(block_size: f32) -> Self {
        Self::new(Some(block_size.max(0.0)), Some(block_size.max(0.0)))
            .expect("a finite non-negative block size has a valid synthesized baseline")
    }
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
    flow: FlowAxes,
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
            flow: FlowAxes::HORIZONTAL_LTR,
            physical_rect: rect,
        }
    }

    /// Construct a fragment from standards-owned logical geometry.
    pub fn from_logical(
        box_id: BoxId,
        logical_rect: LogicalRect,
        containing_block: PhysicalSize,
        flow: FlowAxes,
    ) -> Self {
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
            flow,
            physical_rect: flow.physical_rect(logical_rect, containing_block),
        }
    }

    /// Preserve a physical consumer rectangle while attaching the logical
    /// geometry that produced it. This is used when a host has already
    /// accumulated ancestor origins at its physical fragment edge.
    pub fn from_physical_with_logical(
        box_id: BoxId,
        physical_rect: PhysicalRect,
        logical_rect: LogicalRect,
        flow: FlowAxes,
    ) -> Self {
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
            flow,
            physical_rect,
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

    pub fn flow(&self) -> FlowAxes {
        self.flow
    }

    pub fn physical_rect(&self) -> PhysicalRect {
        self.physical_rect
    }

    /// Attach formatting-context baseline outputs before the fragment enters
    /// the tree. The values remain logical offsets and are not derived from
    /// the physical rectangle.
    pub fn with_baselines(mut self, baselines: Baselines) -> Self {
        debug_assert!(Baselines::new(baselines.first, baselines.last).is_some());
        self.baselines = baselines;
        self
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
    ///
    /// K4e4 makes the choice this makes explicit: boxes are registered in
    /// materialization order, outermost first, so a node that generates an
    /// anonymous box around its principal box answers with the outer one. For
    /// a table element that is the table wrapper box, which is the box that
    /// participates in flow, carries `transform` and `opacity` under CSS
    /// Tables 3 section 3.6.1, and contains the captions - the right box for
    /// rectangle queries, hit targets, and paint-effect anchors.
    pub fn get(&self, node: Id) -> Option<&Fragment> {
        self.fragments_for_node(node).next()
    }

    /// The fragment of the node's principal box: the element's own box rather
    /// than an anonymous box generated around it.
    ///
    /// A table element's principal box is the table grid box, which owns its
    /// background, borders, and the `width` and `height` properties under
    /// CSS 2.1 section 17.4. Everything else answers with [`Self::get`].
    pub fn principal_fragment(&self, node: Id) -> Option<&Fragment> {
        self.boxes
            .principal_box(node)
            .and_then(|principal| self.fragments.fragments_for_box(principal).next())
            .or_else(|| self.get(node))
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
                FlowAxes::HORIZONTAL_LTR,
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
                FlowAxes::HORIZONTAL_LTR,
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
                FlowAxes::HORIZONTAL_LTR,
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

    #[test]
    fn logical_fragment_geometry_derives_physical_geometry_at_the_edge() {
        let mut boxes = CssBoxTree::default();
        let flow = FlowAxes::new(crate::WritingMode::VerticalRl, crate::Direction::Ltr);
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::BLOCK_FLOW,
                flow,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let logical = LogicalRect {
            inline_start: 20.0,
            block_start: 30.0,
            inline_size: 40.0,
            block_size: 70.0,
        };
        let fragment = Fragment::from_logical(
            box_id,
            logical,
            PhysicalSize {
                width: 300.0,
                height: 200.0,
            },
            flow,
        );

        assert_eq!(fragment.logical_rect, logical);
        assert_eq!(fragment.flow(), flow);
        assert_eq!(
            fragment.physical_rect(),
            PhysicalRect {
                x: 200.0,
                y: 20.0,
                width: 70.0,
                height: 40.0,
            }
        );
    }

    /// A negative margin can place a child's baseline above its parent's
    /// block-start edge, so a negative offset is a valid baseline. Rejecting
    /// it crashed baseline propagation on
    /// WPT `css/CSS2/css21-errata/s-11-1-1b-005.html`.
    #[test]
    fn baselines_accept_negative_offsets_and_reject_non_finite_ones() {
        assert!(Baselines::new(Some(-15.0), Some(-15.0)).is_some());
        assert!(Baselines::new(Some(-15.0), Some(20.0)).is_some());
        assert!(Baselines::new(Some(f32::NAN), None).is_none());
        assert!(Baselines::new(None, Some(f32::INFINITY)).is_none());
    }

    #[test]
    fn fragment_baselines_are_logical_outputs_not_physical_coordinates() {
        let mut boxes = CssBoxTree::default();
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::BLOCK_FLOW,
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let baselines = Baselines::new(Some(6.0), Some(18.0)).expect("valid baselines");
        let fragment = Fragment::from_horizontal_physical(
            box_id,
            PhysicalRect {
                x: 40.0,
                y: 90.0,
                width: 120.0,
                height: 30.0,
            },
        )
        .with_baselines(baselines);

        assert_eq!(fragment.baselines, baselines);
        assert_eq!(fragment.physical_rect().y, 90.0);
        assert_ne!(fragment.baselines.first, Some(fragment.physical_rect().y));
        assert_ne!(
            fragment.baselines.last,
            Some(fragment.physical_rect().height)
        );
    }
}
