/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Box tree — serval's own layout arena implementing Taffy's trait-impl
//! tree, fed by [`stylo_taffy::TaffyStyloStyle`] (zero-copy over
//! `ComputedValues`).
//!
//! This is the alternative to the owned-`Style` `TaffyTree` model in
//! [`crate::construct`] + [`crate::layout`]. Where `TaffyTree` stores a
//! built `taffy::Style` per node (hence `cv_to_taffy`), the box tree
//! stores the cascade's `Arc<ComputedValues>` per node and lets
//! `TaffyStyloStyle` read layout properties straight off it. Taffy's
//! algorithms stay in Taffy — we implement only the tree shape + style
//! access traits and call `compute_root_layout` / `round_layout`.
//!
//! Increment 1 (this file): the arena, the trait impls, and
//! [`layout_via_box_tree`] producing a [`FragmentPlane`]. The
//! `TaffyTree`-based [`crate::layout::layout`] stays as the diff-test
//! oracle until the box tree reaches parity and the swap lands.
//!
//! Cf. `docs/2026-05-25_box_tree_trait_impl_plan.md`.

#![allow(unsafe_code)] // calc-value resolution casts a raw pointer back to a Stylo calc node.

use std::hash::Hash;
use std::sync::OnceLock;

use layout_dom_api::{LayoutDom, NodeKind};
use rustc_hash::FxHashMap;
use servo_arc::Arc as ServoArc;
use style::properties::style_structs::Font;
use style::properties::ComputedValues;
use stylo_taffy::TaffyStyloStyle;
use taffy::{
    AvailableSpace, Cache, CacheTree, Layout, LayoutBlockContainer, LayoutFlexboxContainer,
    LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree, NodeId, RoundTree, RunMode,
    Size, TraversePartialTree, TraverseTree,
};

use crate::adapter::NodeRef;
use crate::construct::{establishes_inline_context, gather_inline_content, is_replaced, run_for_element};
use crate::fragment::FragmentPlane;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;
use crate::text_measure::{measure_inline_content, InlineContent, TextMeasureCtx};

/// Shared initial `ComputedValues` for anonymous/text leaves, which have
/// no DOM element of their own. They're childless (sized by the measure
/// fn), so the only thing this style contributes is "no padding / border
/// / margin / explicit size" — exactly the CSS-initial values.
fn initial_style() -> ServoArc<ComputedValues> {
    static INIT: OnceLock<ServoArc<ComputedValues>> = OnceLock::new();
    INIT.get_or_init(|| ComputedValues::initial_values_with_font_override(Font::initial_values()))
        .clone()
}

/// `usize` arena index → Taffy `NodeId`.
#[inline]
fn nid(i: usize) -> NodeId {
    NodeId::from(i)
}

/// Taffy `NodeId` → `usize` arena index.
#[inline]
fn idx(n: NodeId) -> usize {
    u64::from(n) as usize
}

/// One box in the arena.
struct BoxNode<Id> {
    /// Cascaded style, read by `TaffyStyloStyle`. A cheap refcount clone
    /// of the cascade's primary `Arc<ComputedValues>` (or the shared
    /// initial values for anonymous leaves).
    style: ServoArc<ComputedValues>,
    /// Arena indices of child boxes, in document order.
    children: Vec<usize>,
    /// `Some` => a measured leaf (inline formatting context / bare text);
    /// parley measures it via [`measure_inline_content`].
    inline_content: Option<InlineContent<Id>>,
    /// `Some((w, h))` => a replaced leaf (`<img>`) measured to this size
    /// (intrinsic from the `ImagePlane`, overridden by definite CSS
    /// width/height). Mutually exclusive with `inline_content`.
    replaced_size: Option<(f32, f32)>,
    cache: Cache,
    unrounded_layout: Layout,
    final_layout: Layout,
}

impl<Id> BoxNode<Id> {
    fn new(style: ServoArc<ComputedValues>) -> Self {
        Self {
            style,
            children: Vec::new(),
            inline_content: None,
            replaced_size: None,
            cache: Cache::new(),
            unrounded_layout: Layout::new(),
            final_layout: Layout::new(),
        }
    }
}

/// serval's layout arena. Built from a `LayoutDom` + `StylePlane`;
/// laid out via Taffy's trait-impl algorithms.
pub struct BoxTree<Id: Copy + Eq + Hash> {
    nodes: Vec<BoxNode<Id>>,
    root: usize,
    /// DOM `NodeId` → Taffy `NodeId` (the arena index as a `NodeId`), so
    /// callers read results back keyed by DOM identity — same contract as
    /// `ConstructedTree::node_map`.
    pub node_map: FxHashMap<Id, NodeId>,
}

impl<Id: Copy + Eq + Hash> BoxTree<Id> {
    fn push(&mut self, node: BoxNode<Id>) -> usize {
        let i = self.nodes.len();
        self.nodes.push(node);
        i
    }
}

/// Build the box tree from `dom` rooted at the document's root element,
/// reading style from `styles` and replaced-element sizes from `images`.
///
/// The root is the document's first element child (`<html>`) — no
/// synthetic wrapper: `compute_root_layout` resolves `<html>`'s UA
/// `width:100%/height:100%` against the viewport available space.
pub fn build_box_tree<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
) -> BoxTree<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut tree = BoxTree {
        nodes: Vec::new(),
        root: 0,
        node_map: FxHashMap::default(),
    };

    // The document's root element (skip comments / doctype the parser
    // may have placed before <html>). Fall back to the document node's
    // own style if there is somehow no element child.
    let doc = NodeRef::document(dom);
    let root_elem = doc
        .dom_children()
        .find(|c| matches!(dom.kind(c.id()), NodeKind::Element));

    let root = match root_elem {
        Some(elem) => build_node(dom, styles, images, elem, &mut tree),
        None => tree.push(BoxNode::new(initial_style())),
    };
    tree.root = root;
    tree
}

/// Recursively build the box for `elem` (an element node) and its
/// descendants; returns its arena index.
fn build_node<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
    tree: &mut BoxTree<D::NodeId>,
) -> usize
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let style = style_of(styles, elem.id());

    // Replaced leaf: a lone <img> (mixed-with-text <img>s flow inside an
    // inline-context leaf and are handled there, not here).
    if is_replaced(dom, elem.id()) {
        let mut node = BoxNode::new(style);
        node.replaced_size = Some(replaced_size(styles, images, elem.id()));
        let i = tree.push(node);
        tree.node_map.insert(elem.id(), nid(i));
        return i;
    }

    // Inline formatting context: one measured leaf gathering the inline
    // subtree's runs + boxes; inline children get no boxes of their own.
    if establishes_inline_context(dom, styles, elem) {
        let mut node = BoxNode::new(style);
        node.inline_content = Some(gather_inline_content(dom, styles, elem));
        let i = tree.push(node);
        tree.node_map.insert(elem.id(), nid(i));
        return i;
    }

    // Block / mixed: build child boxes, recursing into elements and
    // turning bare text into one-run inline leaves (mirrors
    // `construct::build_children`).
    let mut children = Vec::new();
    for child in elem.dom_children() {
        match dom.kind(child.id()) {
            NodeKind::Element => children.push(build_node(dom, styles, images, child, tree)),
            NodeKind::Text => {
                let text = dom.text(child.id()).unwrap_or("").to_string();
                let content = InlineContent {
                    runs: vec![run_for_element(styles, elem.id(), text)],
                    boxes: Vec::new(),
                };
                let mut node = BoxNode::new(initial_style());
                node.inline_content = Some(content);
                let i = tree.push(node);
                // Text nodes are addressable too (the oracle inserts them).
                tree.node_map.insert(child.id(), nid(i));
                children.push(i);
            },
            _ => {},
        }
    }
    let mut node = BoxNode::new(style);
    node.children = children;
    let i = tree.push(node);
    tree.node_map.insert(elem.id(), nid(i));
    i
}

/// Clone the cascaded primary style for `id`, or the shared initial
/// values if the cascade has no entry for it.
fn style_of<Id: Copy + Eq + Hash>(styles: &StylePlane<Id>, id: Id) -> ServoArc<ComputedValues> {
    styles
        .get(id)
        .and_then(|e| e.borrow_data().map(|d| d.styles.primary().clone()))
        .unwrap_or_else(initial_style)
}

/// Pixel size for a replaced `<img>` leaf: the decoded intrinsic size
/// from the `ImagePlane`, with each axis overridden by a definite CSS
/// `width`/`height` if the cascade set one. Matches the oracle, where
/// `apply_intrinsic_image_sizes` fills auto axes from the decoded size.
fn replaced_size<Id: Copy + Eq + Hash>(
    styles: &StylePlane<Id>,
    images: &ImagePlane<Id>,
    id: Id,
) -> (f32, f32) {
    let (mut w, mut h) = images
        .get(id)
        .map(|d| (d.width as f32, d.height as f32))
        .unwrap_or((0.0, 0.0));

    // Definite CSS size wins over intrinsic, per axis.
    if let Some(entry) = styles.get(id) {
        if let Some(data) = entry.borrow_data() {
            let pos = data.styles.primary().get_position();
            if let Some(cw) = definite_px(&pos.width) {
                w = cw;
            }
            if let Some(ch) = definite_px(&pos.height) {
                h = ch;
            }
        }
    }
    (w, h)
}

/// A CSS `Size` as definite pixels, or `None` for `auto` / percentage /
/// intrinsic keywords (which leave the intrinsic image size in place).
fn definite_px(size: &style::values::computed::Size) -> Option<f32> {
    use style::values::computed::Size as CssSize;
    match size {
        CssSize::LengthPercentage(lp) => lp.0.to_length().map(|l| l.px()),
        _ => None,
    }
}

/// The `TaffyStyloStyle` GAT — owned (an `Arc` clone), so it carries no
/// borrow of the tree.
type NodeStyle = TaffyStyloStyle<ServoArc<ComputedValues>>;

/// Block-item style adapter.
///
/// `stylo_taffy 0.3.0-alpha.4`'s `TaffyStyloStyle` implements
/// `BlockItemStyle` but **only** overrides `is_table` — it leaves
/// `float()`/`clear()` at the trait defaults (`None`), so block floats
/// are invisible through the zero-copy wrapper (they work through the
/// owned-`Style` `to_taffy_style` path, which sets the fields). This
/// newtype delegates every `CoreStyle` method to the inner
/// `TaffyStyloStyle` and forwards `float`/`clear` via
/// `stylo_taffy::convert`, restoring block-float parity with the oracle.
/// (Upstream fix candidate: forward these in the wrapper.)
struct BlockItem(NodeStyle);

impl taffy::CoreStyle for BlockItem {
    type CustomIdent = style::Atom;

    #[inline]
    fn box_generation_mode(&self) -> taffy::BoxGenerationMode {
        self.0.box_generation_mode()
    }
    #[inline]
    fn is_block(&self) -> bool {
        self.0.is_block()
    }
    #[inline]
    fn is_compressible_replaced(&self) -> bool {
        self.0.is_compressible_replaced()
    }
    #[inline]
    fn box_sizing(&self) -> taffy::BoxSizing {
        self.0.box_sizing()
    }
    #[inline]
    fn direction(&self) -> taffy::Direction {
        self.0.direction()
    }
    #[inline]
    fn overflow(&self) -> taffy::Point<taffy::Overflow> {
        self.0.overflow()
    }
    #[inline]
    fn scrollbar_width(&self) -> f32 {
        self.0.scrollbar_width()
    }
    #[inline]
    fn position(&self) -> taffy::Position {
        self.0.position()
    }
    #[inline]
    fn inset(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        self.0.inset()
    }
    #[inline]
    fn size(&self) -> taffy::Size<taffy::Dimension> {
        self.0.size()
    }
    #[inline]
    fn min_size(&self) -> taffy::Size<taffy::Dimension> {
        self.0.min_size()
    }
    #[inline]
    fn max_size(&self) -> taffy::Size<taffy::Dimension> {
        self.0.max_size()
    }
    #[inline]
    fn aspect_ratio(&self) -> Option<f32> {
        self.0.aspect_ratio()
    }
    #[inline]
    fn margin(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        self.0.margin()
    }
    #[inline]
    fn padding(&self) -> taffy::Rect<taffy::LengthPercentage> {
        self.0.padding()
    }
    #[inline]
    fn border(&self) -> taffy::Rect<taffy::LengthPercentage> {
        self.0.border()
    }
}

impl taffy::BlockItemStyle for BlockItem {
    #[inline]
    fn is_table(&self) -> bool {
        taffy::BlockItemStyle::is_table(&self.0)
    }
    #[inline]
    fn float(&self) -> taffy::Float {
        stylo_taffy::convert::float(self.0 .0.clone_float())
    }
    #[inline]
    fn clear(&self) -> taffy::Clear {
        stylo_taffy::convert::clear(self.0 .0.clone_clear())
    }
}

/// View bundling the tree (owns the nodes) with the parley measure
/// context, so the measure closure in `compute_child_layout` can reach
/// `TextMeasureCtx` while Taffy holds `&mut` to the tree — the same
/// split Taffy's own `TaffyView` uses.
struct BoxTreeView<'a, Id: Copy + Eq + Hash> {
    tree: &'a mut BoxTree<Id>,
    text_ctx: &'a mut TextMeasureCtx,
}

impl<Id: Copy + Eq + Hash> BoxTreeView<'_, Id> {
    #[inline]
    fn node(&self, n: NodeId) -> &BoxNode<Id> {
        &self.tree.nodes[idx(n)]
    }

    /// Unified dispatch that both `LayoutPartialTree::compute_child_layout`
    /// and `LayoutBlockContainer::compute_block_child_layout` delegate to —
    /// the latter threading `block_ctx` so floats see their block
    /// formatting context (mirrors Taffy's own `TaffyView`).
    fn compute_child_layout_inner(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut taffy::BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return taffy::compute_hidden_layout(self, node);
        }

        taffy::compute_cached_layout(self, node, inputs, |tree, node, inputs| {
            let key = idx(node);
            let display = tree.tree.nodes[key].style.clone_display();
            let has_children = !tree.tree.nodes[key].children.is_empty();

            use taffy::Display;
            let taffy_display = stylo_taffy::convert::display(display);
            match (taffy_display, has_children) {
                (Display::None, _) => taffy::compute_hidden_layout(tree, node),
                (Display::Block, true) => {
                    taffy::compute_block_layout(tree, node, inputs, block_ctx)
                },
                (Display::Flex, true) => taffy::compute_flexbox_layout(tree, node, inputs),
                (Display::Grid, true) => taffy::compute_grid_layout(tree, node, inputs),
                // Leaf: replaced (<img>) or text/inline measured by parley.
                (_, false) => {
                    let style = TaffyStyloStyle(tree.tree.nodes[key].style.clone());
                    let replaced = tree.tree.nodes[key].replaced_size;
                    taffy::compute_leaf_layout(inputs, &style, |_, _| 0.0, |known, avail| {
                        if let Some((w, h)) = replaced {
                            return Size { width: w, height: h };
                        }
                        match &tree.tree.nodes[key].inline_content {
                            Some(content) => {
                                measure_inline_content(tree.text_ctx, content, node, known, avail)
                            },
                            None => Size::ZERO,
                        }
                    })
                },
            }
        })
    }
}

impl<Id: Copy + Eq + Hash> TraversePartialTree for BoxTreeView<'_, Id> {
    type ChildIter<'b>
        = std::iter::Map<std::slice::Iter<'b, usize>, fn(&usize) -> NodeId>
    where
        Self: 'b;

    #[inline]
    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().map(|i| nid(*i))
    }

    #[inline]
    fn child_count(&self, parent: NodeId) -> usize {
        self.node(parent).children.len()
    }

    #[inline]
    fn get_child_id(&self, parent: NodeId, index: usize) -> NodeId {
        nid(self.node(parent).children[index])
    }
}

impl<Id: Copy + Eq + Hash> TraverseTree for BoxTreeView<'_, Id> {}

impl<Id: Copy + Eq + Hash> LayoutPartialTree for BoxTreeView<'_, Id> {
    type CoreContainerStyle<'b>
        = NodeStyle
    where
        Self: 'b;
    type CustomIdent = style::Atom;

    #[inline]
    fn get_core_container_style(&self, node: NodeId) -> NodeStyle {
        TaffyStyloStyle(self.node(node).style.clone())
    }

    #[inline]
    fn resolve_calc_value(&self, val: *const (), basis: f32) -> f32 {
        use style::values::computed::length_percentage::CalcLengthPercentage;
        use style::values::computed::Length;
        // SAFETY: `val` is the pointer `stylo_taffy::convert::length_percentage`
        // packed into `CompactLength::calc` — a `*const CalcLengthPercentage`
        // borrowed from the live `ComputedValues` this tree holds (kept
        // alive for the whole layout pass).
        let calc = unsafe { &*(val as *const CalcLengthPercentage) };
        calc.resolve(Length::new(basis)).px()
    }

    #[inline]
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.tree.nodes[idx(node)].unrounded_layout = *layout;
    }

    #[inline]
    fn compute_child_layout(&mut self, node: NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute_child_layout_inner(node, inputs, None)
    }
}

impl<Id: Copy + Eq + Hash> CacheTree for BoxTreeView<'_, Id> {
    #[inline]
    fn cache_get(&self, node: NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.node(node).cache.get(input)
    }

    #[inline]
    fn cache_store(&mut self, node: NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.tree.nodes[idx(node)].cache.store(input, output);
    }

    #[inline]
    fn cache_clear(&mut self, node: NodeId) {
        self.tree.nodes[idx(node)].cache.clear();
    }
}

impl<Id: Copy + Eq + Hash> RoundTree for BoxTreeView<'_, Id> {
    #[inline]
    fn get_unrounded_layout(&self, node: NodeId) -> Layout {
        self.node(node).unrounded_layout
    }

    #[inline]
    fn set_final_layout(&mut self, node: NodeId, layout: &Layout) {
        self.tree.nodes[idx(node)].final_layout = *layout;
    }
}

impl<Id: Copy + Eq + Hash> LayoutBlockContainer for BoxTreeView<'_, Id> {
    type BlockContainerStyle<'b>
        = NodeStyle
    where
        Self: 'b;
    type BlockItemStyle<'b>
        = BlockItem
    where
        Self: 'b;

    #[inline]
    fn get_block_container_style(&self, node: NodeId) -> NodeStyle {
        self.get_core_container_style(node)
    }

    #[inline]
    fn get_block_child_style(&self, child: NodeId) -> BlockItem {
        BlockItem(self.get_core_container_style(child))
    }

    #[inline]
    fn compute_block_child_layout(
        &mut self,
        node: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut taffy::BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute_child_layout_inner(node, inputs, block_ctx)
    }
}

impl<Id: Copy + Eq + Hash> LayoutFlexboxContainer for BoxTreeView<'_, Id> {
    type FlexboxContainerStyle<'b>
        = NodeStyle
    where
        Self: 'b;
    type FlexboxItemStyle<'b>
        = NodeStyle
    where
        Self: 'b;

    #[inline]
    fn get_flexbox_container_style(&self, node: NodeId) -> NodeStyle {
        self.get_core_container_style(node)
    }

    #[inline]
    fn get_flexbox_child_style(&self, child: NodeId) -> NodeStyle {
        self.get_core_container_style(child)
    }
}

impl<Id: Copy + Eq + Hash> LayoutGridContainer for BoxTreeView<'_, Id> {
    type GridContainerStyle<'b>
        = NodeStyle
    where
        Self: 'b;
    type GridItemStyle<'b>
        = NodeStyle
    where
        Self: 'b;

    #[inline]
    fn get_grid_container_style(&self, node: NodeId) -> NodeStyle {
        self.get_core_container_style(node)
    }

    #[inline]
    fn get_grid_child_style(&self, child: NodeId) -> NodeStyle {
        self.get_core_container_style(child)
    }
}

/// Lay out `dom` via the box tree against `viewport`, returning the
/// per-node [`FragmentPlane`] and the `TextMeasureCtx` (cached parley
/// layouts, for paint emission — same outputs as [`crate::layout::layout`]).
pub fn layout_via_box_tree<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    viewport: Size<AvailableSpace>,
) -> (FragmentPlane<D::NodeId>, BoxTree<D::NodeId>, TextMeasureCtx)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut tree = build_box_tree(dom, styles, images);
    let mut text_ctx = TextMeasureCtx::new();
    let root = nid(tree.root);

    {
        let mut view = BoxTreeView {
            tree: &mut tree,
            text_ctx: &mut text_ctx,
        };
        taffy::compute_root_layout(&mut view, root, viewport);
        taffy::round_layout(&mut view, root);
    }

    let mut fragments = FragmentPlane::new();
    for (dom_id, taffy_id) in tree.node_map.iter() {
        fragments.insert(*dom_id, tree.nodes[idx(*taffy_id)].final_layout);
    }

    (fragments, tree, text_ctx)
}

#[cfg(test)]
mod tests {
    //! Diff-test: the box tree (TaffyStyloStyle, zero-copy) must produce
    //! the same `FragmentPlane` as the `TaffyTree`-based oracle
    //! (`crate::layout::layout`, via the `cv_to_taffy` converter). When
    //! these agree across the corpus, the box tree can replace the
    //! oracle and `cv_to_taffy` retires.

    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::cascade::run_cascade;
    use crate::layout::layout as oracle_layout;

    const VIEWPORT: f32 = 128.0;

    /// Cascade a fixture into a `StylePlane` the same way the e2e
    /// pipeline does (UA defaults + the test sheets, then refresh the
    /// owned Taffy styles the *oracle* reads). The box tree ignores the
    /// refreshed `taffy` field and reads `ComputedValues` directly.
    fn cascade(html: &str, sheets: &[&str]) -> (StaticDocument, StylePlane<StaticNodeId>) {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(VIEWPORT, VIEWPORT),
            sheets,
        );
        styles.refresh_taffy_from_cascade();
        (document, styles)
    }

    /// Run both pipelines over the fixture and assert every node in the
    /// oracle's fragment plane has a box-tree rect within `eps`, and the
    /// two planes cover the same node set.
    fn assert_parity(html: &str, sheets: &[&str]) {
        let (document, styles) = cascade(html, sheets);
        let images = ImagePlane::new();
        let viewport = Size {
            width: AvailableSpace::Definite(VIEWPORT),
            height: AvailableSpace::Definite(VIEWPORT),
        };

        let (oracle, _, _) = oracle_layout(&document, &styles, viewport);
        let (boxed, _, _) = layout_via_box_tree(&document, &styles, &images, viewport);

        assert_eq!(
            oracle.len(),
            boxed.len(),
            "fragment count mismatch: oracle={} box={} for {html:?}",
            oracle.len(),
            boxed.len()
        );

        let eps = 0.5; // sub-pixel; both pipelines round to the grid.
        for (id, o) in oracle.iter() {
            let b = boxed
                .rect_of(*id)
                .unwrap_or_else(|| panic!("box tree missing node {id:?} for {html:?}"));
            let close = |a: f32, c: f32, what: &str| {
                assert!(
                    (a - c).abs() <= eps,
                    "{what} mismatch for node {id:?} in {html:?}: oracle={a} box={c}"
                );
            };
            close(o.location.x, b.location.x, "location.x");
            close(o.location.y, b.location.y, "location.y");
            close(o.size.width, b.size.width, "size.width");
            close(o.size.height, b.size.height, "size.height");
        }
    }

    #[test]
    fn parity_block_siblings_stack() {
        assert_parity(
            "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
            &[
                "body { background-color: rgb(255,255,255); }",
                ".a { width: 60px; height: 40px; }",
                ".b { width: 60px; height: 40px; }",
            ],
        );
    }

    #[test]
    fn parity_nested_padding_offset() {
        assert_parity(
            "<html><body><div></div></body></html>",
            &[
                "body { padding-left: 40px; padding-top: 40px; }",
                "div { width: 30px; height: 30px; }",
            ],
        );
    }

    #[test]
    fn parity_inline_text_flow() {
        assert_parity(
            "<html><body><p>Hello <b>world</b> !</p></body></html>",
            &[],
        );
    }

    #[test]
    fn parity_borders_border_box() {
        assert_parity(
            "<html><body><div></div></body></html>",
            &["div { width: 40px; height: 40px; border: 10px solid rgb(0,128,0); }"],
        );
    }

    #[test]
    fn parity_relative_position() {
        assert_parity(
            "<html><body><div></div></body></html>",
            &["div { width: 30px; height: 30px; position: relative; top: 20px; left: 20px; }"],
        );
    }

    #[test]
    fn parity_cascaded_font_size() {
        assert_parity(
            "<html><body><p>Hello</p></body></html>",
            &["p { font-size: 32px; }"],
        );
    }

    #[test]
    fn parity_plain_paragraph() {
        assert_parity("<html><body><p>Hello, serval!</p></body></html>", &[]);
    }

    #[test]
    fn parity_float_left_blocks() {
        assert_parity(
            "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
            &[
                ".a { float: left; width: 40px; height: 40px; }",
                ".b { float: left; width: 40px; height: 40px; }",
            ],
        );
    }
}
