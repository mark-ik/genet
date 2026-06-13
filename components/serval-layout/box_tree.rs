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
use parley::LayoutContext;
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
use crate::construct::{
    block_pseudo_content, establishes_inline_context, flows_inline, gather_inline_content,
    gather_inline_group, is_replaced, list_marker_content, list_marker_inline_run,
    list_marker_is_inside, replaced_px_size, run_for_element,
};
use crate::fragment::FragmentPlane;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;
use crate::text_measure::{
    measure_inline_content, shape_leaf, ColorBrush, InlineContent, TextMeasureCtx,
};

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

/// Which generated-content pseudo a [`BoxSource::Pseudo`] box realizes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PseudoKind {
    Before,
    After,
}

/// What a box's identity is — its style already lives on the node, so this is
/// for *paint / hit-test* routing: it carries the originating DOM node for the
/// `dom_id`-keyed concerns (scroll offsets, replaced/background images, canvas
/// background propagation, hit-test target) and marks boxes that own no DOM
/// element of their own. A box-tree-driven paint walk reads this instead of
/// assuming every box maps 1:1 to a DOM node.
#[derive(Clone, Copy)]
pub(crate) enum BoxSource<Id> {
    /// A real DOM element or text node; `Id` is that node.
    Element(Id),
    /// An anonymous block box wrapping a run of inline-level children. `Id` is the
    /// borrowed first-member key it is stored under; it paints no box decorations
    /// of its own.
    Anonymous(Id),
    /// Block-level generated content (`::before` / `::after`). `Id` is the
    /// originating element; the box is not script-visible (no `node_map` entry, no
    /// `FragmentPlane` identity), and a hit on it routes back to `Id`.
    Pseudo(Id, PseudoKind),
}

impl<Id: Copy> BoxSource<Id> {
    /// The DOM node this box's `dom_id`-keyed paint concerns (scroll, images,
    /// canvas-bg, hit-test) resolve against: the element for a real box, the
    /// borrowed key for an anonymous wrapper (matching the legacy DOM walk, which
    /// reached the anonymous box under that key), the originating element for a
    /// pseudo (so a hit routes there).
    pub(crate) fn dom_id(self) -> Id {
        match self {
            BoxSource::Element(id) | BoxSource::Anonymous(id) | BoxSource::Pseudo(id, _) => id,
        }
    }

    /// Whether this box paints no box decorations of its own. A pseudo box *does*
    /// paint its decorations (its own padding / background / border), so only an
    /// anonymous wrapper is decoration-less.
    pub(crate) fn is_anonymous(self) -> bool {
        matches!(self, BoxSource::Anonymous(_))
    }
}

/// One box in the arena.
pub(crate) struct BoxNode<Id> {
    /// Cascaded style, read by `TaffyStyloStyle` (and paint, post box-tree
    /// re-root). A cheap refcount clone of the cascade's primary
    /// `Arc<ComputedValues>` (or the shared initial values for anonymous leaves).
    pub(crate) style: ServoArc<ComputedValues>,
    /// Arena indices of child boxes, in document order.
    pub(crate) children: Vec<usize>,
    /// `Some` => a measured leaf (inline formatting context / bare text);
    /// parley measures it via [`measure_inline_content`].
    pub(crate) inline_content: Option<InlineContent<Id>>,
    /// `Some` for a list item (`<li>`): its hanging marker (a bullet or ordinal)
    /// as single-run inline content. Shaped into
    /// [`TextMeasureCtx::marker_layouts`](crate::text_measure::TextMeasureCtx)
    /// after layout; paint hangs it to the left of the item's content box.
    pub(crate) marker: Option<InlineContent<Id>>,
    /// `Some((w, h))` => a replaced leaf (`<img>`) measured to this size
    /// (intrinsic from the `ImagePlane`, overridden by definite CSS
    /// width/height). Mutually exclusive with `inline_content`.
    pub(crate) replaced_size: Option<(f32, f32)>,
    /// Paint/hit-test identity (see [`BoxSource`]). An [`BoxSource::Anonymous`]
    /// box wraps a run of a mixed container's inline-level children: it has no DOM
    /// element of its own, so it paints no box decorations — its `node_map` key is
    /// a borrowed descendant node whose style (background / border) must not be
    /// painted on this box. Its inline content (e.g. an inline-block as an
    /// `InlineBox`) still paints at its own size.
    pub(crate) source: BoxSource<Id>,
    cache: Cache,
    unrounded_layout: Layout,
    pub(crate) final_layout: Layout,
}

impl<Id> BoxNode<Id> {
    fn new(style: ServoArc<ComputedValues>, source: BoxSource<Id>) -> Self {
        Self {
            style,
            children: Vec::new(),
            inline_content: None,
            marker: None,
            replaced_size: None,
            source,
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

    /// The inline content (styled runs + replaced boxes) of a measured
    /// leaf, keyed by its Taffy `NodeId` — paint emission reads this to
    /// extract positioned glyphs. `None` for block nodes / replaced
    /// leaves. Mirrors `TaffyTree::get_node_context` on the old oracle.
    pub fn get_node_context(&self, id: NodeId) -> Option<&InlineContent<Id>> {
        self.nodes.get(idx(id)).and_then(|n| n.inline_content.as_ref())
    }

    /// The root box's arena index — the entry point for the box-tree paint walk.
    pub(crate) fn root_arena(&self) -> usize {
        self.root
    }

    /// The box node at arena index `arena`.
    pub(crate) fn node(&self, arena: usize) -> &BoxNode<Id> {
        &self.nodes[arena]
    }

    /// The arena `NodeId` for `arena` — the key under which this box's shaped
    /// text / marker `parley::Layout` is cached in [`TextMeasureCtx`].
    pub(crate) fn arena_node_id(&self, arena: usize) -> NodeId {
        nid(arena)
    }

    /// Re-point each *directly mutated* element box's cached paint style to the
    /// plane's freshly cascaded value. Paint reads `BoxNode::style` (the box-tree
    /// paint re-root), and the `RepaintOnly` apply path keeps this box tree (its
    /// geometry is still valid — `transform` / color are paint-tier), so without
    /// this refresh the painted node keeps the style cloned at the last full
    /// layout: a per-frame `transform` (the orrery camera + node motion) or a
    /// color change lands in the plane but never reaches emit until a relayout.
    /// Keyed by the mutated DOM ids through `node_map`; only `Element` boxes are
    /// refreshed (an `Anonymous` wrapper paints no decorations; a `Pseudo` box
    /// carries the pseudo cascade, not the element's). Inherited-only changes on
    /// undirtied descendants are out of scope — the orrery / chrome restyle the
    /// element itself, which is what lands in the mutation set.
    pub(crate) fn refresh_styles_for<I>(&mut self, styles: &StylePlane<Id>, mutated: I)
    where
        I: IntoIterator<Item = Id>,
    {
        for id in mutated {
            let Some(&node_id) = self.node_map.get(&id) else { continue };
            let i = idx(node_id);
            if matches!(self.nodes[i].source, BoxSource::Element(eid) if eid == id) {
                self.nodes[i].style = style_of(styles, id);
            }
        }
    }

    /// Whether the box for DOM `id` is an anonymous box (paints no box
    /// decorations of its own — see [`BoxNode::anonymous`]). Paint emission
    /// reads this to skip background / border / shadow on anonymous wrappers.
    pub fn is_anonymous(&self, id: Id) -> bool {
        self.node_map
            .get(&id)
            .and_then(|&t| self.nodes.get(idx(t)))
            .is_some_and(|n| matches!(n.source, BoxSource::Anonymous(_)))
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

    // The layout root. Two shapes of `LayoutDom::document()`:
    //   - A `Document` wrapper node (the normal case): its first element
    //     child (`<html>`, skipping comments/doctype) is the real root.
    //   - An element (a re-rooted `SubtreeView`, whose `document()` is the
    //     subtree root, e.g. `<body>`): that element *is* the root, and
    //     all of its children must be laid out.
    let doc = NodeRef::document(dom);
    let root = if matches!(dom.kind(doc.id()), NodeKind::Element) {
        build_node(dom, styles, images, doc, &mut tree)
    } else {
        match doc
            .dom_children()
            .find(|c| matches!(dom.kind(c.id()), NodeKind::Element))
        {
            Some(elem) => build_node(dom, styles, images, elem, &mut tree),
            None => tree.push(BoxNode::new(initial_style(), BoxSource::Element(doc.id()))),
        }
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

    // Block-level `::before` / `::after` generated content becomes a synthetic
    // block box child (first / last). Building them first means an element that
    // would otherwise be a replaced or inline-formatting leaf is instead a block
    // container holding the pseudo box(es) around its content.
    let before = build_block_pseudo(styles, elem.id(), PseudoKind::Before, tree);
    let after = build_block_pseudo(styles, elem.id(), PseudoKind::After, tree);
    let has_block_pseudo = before.is_some() || after.is_some();

    // Replaced leaf: a lone <img> (mixed-with-text <img>s flow inside an
    // inline-context leaf and are handled there, not here).
    if !has_block_pseudo && is_replaced(dom, elem.id()) {
        let mut node = BoxNode::new(style, BoxSource::Element(elem.id()));
        node.replaced_size = Some(replaced_px_size(dom, styles, images, elem.id()));
        let i = tree.push(node);
        tree.node_map.insert(elem.id(), nid(i));
        return i;
    }

    // Inline formatting context: one measured leaf gathering the inline
    // subtree's runs + boxes; inline children get no boxes of their own.
    if !has_block_pseudo && establishes_inline_context(dom, styles, elem) {
        let mut node = BoxNode::new(style, BoxSource::Element(elem.id()));
        let mut content = gather_inline_content(dom, styles, images, elem);
        // List marker: `inside` flows as the item's first inline run; `outside`
        // (the default) hangs to the left as a separate shaped layout.
        if list_marker_is_inside(styles, elem.id()) {
            if let Some(run) = list_marker_inline_run(dom, styles, elem.id()) {
                content.runs.insert(0, run);
            }
        } else {
            node.marker = list_marker_content(dom, styles, elem.id());
        }
        node.inline_content = Some(content);
        let i = tree.push(node);
        tree.node_map.insert(elem.id(), nid(i));
        return i;
    }

    // Block / mixed children. Each run of (non-replaced) inline-level children —
    // non-blank text, inline-blocks, and `display:inline` elements — is wrapped
    // in an anonymous block box: a line carrying them as atomic inline content,
    // so an inline-block is shrink-to-fit and flows rather than being laid out as
    // a stretched block child. Block-level elements and replaced boxes (`<img>`)
    // get their own box. Whitespace-only text between blocks is collapsible
    // (CSS 2.1 §9.2.2.1).
    let mut children = Vec::new();
    // A block `::before` is the first in-flow child.
    children.extend(before);
    let mut group: Vec<NodeRef<'a, D>> = Vec::new();
    for child in elem.dom_children() {
        match dom.kind(child.id()) {
            NodeKind::Text => {
                if dom.text(child.id()).is_some_and(|t| !t.chars().all(char::is_whitespace)) {
                    group.push(child);
                }
            },
            NodeKind::Element if flows_inline(dom, styles, child.id()) => group.push(child),
            NodeKind::Element => {
                flush_anon_group(dom, styles, images, elem.id(), &mut group, &mut children, tree);
                children.push(build_node(dom, styles, images, child, tree));
            },
            _ => {},
        }
    }
    flush_anon_group(dom, styles, images, elem.id(), &mut group, &mut children, tree);
    // A block `::after` is the last in-flow child.
    children.extend(after);
    let mut node = BoxNode::new(style, BoxSource::Element(elem.id()));
    node.children = children;
    node.marker = list_marker_content(dom, styles, elem.id());
    let i = tree.push(node);
    tree.node_map.insert(elem.id(), nid(i));
    i
}

/// Build a synthetic block box for the element's block-level `::before` /
/// `::after` generated content, returning its arena index, or `None` when there
/// is no such pseudo (see [`block_pseudo_content`]). The box is a measured leaf
/// carrying the generated run as inline content, styled by the pseudo cascade; it
/// has no `node_map` entry (not script-visible) and routes hits to `elem` via
/// [`BoxSource::Pseudo`].
fn build_block_pseudo<Id: Copy + Eq + Hash>(
    styles: &StylePlane<Id>,
    elem: Id,
    kind: PseudoKind,
    tree: &mut BoxTree<Id>,
) -> Option<usize> {
    let (style, content) = block_pseudo_content(styles, elem, kind)?;
    let mut node = BoxNode::new(style, BoxSource::Pseudo(elem, kind));
    node.inline_content = Some(content);
    Some(tree.push(node))
}

/// Flush a pending run of inline-level children (`group`) into one anonymous
/// block box: a measured inline leaf carrying them as atomic inline content,
/// flagged `anonymous` so paint skips its (DOM-key's) box decorations. The box
/// is keyed in `node_map` by its first member, so the DOM-driven paint walk
/// reaches it; the other members have no box of their own (their content lives
/// in this box's `InlineContent`). No-op for an empty group; clears it.
#[allow(clippy::too_many_arguments)]
fn flush_anon_group<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    styling: D::NodeId,
    group: &mut Vec<NodeRef<'a, D>>,
    children: &mut Vec<usize>,
    tree: &mut BoxTree<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let Some(first) = group.first() else { return };
    let key = first.id();
    let content = gather_inline_group(dom, styles, images, styling, group);
    let mut node = BoxNode::new(initial_style(), BoxSource::Anonymous(key));
    node.inline_content = Some(content);
    let i = tree.push(node);
    tree.node_map.insert(key, nid(i));
    children.push(i);
    group.clear();
}

/// Clone the cascaded primary style for `id`, or the shared initial
/// values if the cascade has no entry for it.
fn style_of<Id: Copy + Eq + Hash>(styles: &StylePlane<Id>, id: Id) -> ServoArc<ComputedValues> {
    styles
        .get(id)
        .and_then(|e| e.borrow_data().map(|d| d.styles.primary().clone()))
        .unwrap_or_else(initial_style)
}

/// The `TaffyStyloStyle` GAT — owned (an `Arc` clone), so it carries no
/// borrow of the tree.
type NodeStyle = TaffyStyloStyle<ServoArc<ComputedValues>>;

/// `CoreStyle` wrapper that delegates to a `TaffyStyloStyle` but can
/// force a definite `size`.
///
/// Two jobs:
/// 1. **Replaced sizing.** For a lone `<img>`, the oracle bakes the
///    intrinsic/CSS size into the node's owned `taffy::Style`, so the
///    parent's block layout uses it instead of stretching the auto-width
///    box to the container. The box tree reads size from
///    `ComputedValues` (`auto`), so it injects the resolved replaced
///    size via `size_override` — and it must do so in the *parent's*
///    child-style query (`get_block_child_style`), since that's where
///    the stretch decision is made.
/// 2. **`BlockItemStyle` float/clear.** `stylo_taffy 0.3.0-alpha.4`'s
///    `TaffyStyloStyle` implements `BlockItemStyle` but only overrides
///    `is_table`, leaving `float()`/`clear()` at the `None` defaults
///    (they work through the owned-`Style` path, not the zero-copy
///    wrapper). This type forwards them via `stylo_taffy::convert`,
///    restoring block-float parity. (Upstream fix candidate.)
struct CssStyle {
    inner: NodeStyle,
    size_override: Option<taffy::Size<taffy::Dimension>>,
}

impl CssStyle {
    #[inline]
    fn new(inner: NodeStyle) -> Self {
        Self { inner, size_override: None }
    }

    #[inline]
    fn with_size(inner: NodeStyle, size: taffy::Size<taffy::Dimension>) -> Self {
        Self { inner, size_override: Some(size) }
    }
}

impl taffy::CoreStyle for CssStyle {
    type CustomIdent = style::Atom;

    #[inline]
    fn size(&self) -> taffy::Size<taffy::Dimension> {
        self.size_override.unwrap_or_else(|| self.inner.size())
    }

    // Everything else delegates to the inner `TaffyStyloStyle`.
    #[inline]
    fn box_generation_mode(&self) -> taffy::BoxGenerationMode {
        self.inner.box_generation_mode()
    }
    #[inline]
    fn is_block(&self) -> bool {
        self.inner.is_block()
    }
    #[inline]
    fn is_compressible_replaced(&self) -> bool {
        self.inner.is_compressible_replaced()
    }
    #[inline]
    fn box_sizing(&self) -> taffy::BoxSizing {
        self.inner.box_sizing()
    }
    #[inline]
    fn direction(&self) -> taffy::Direction {
        self.inner.direction()
    }
    #[inline]
    fn overflow(&self) -> taffy::Point<taffy::Overflow> {
        self.inner.overflow()
    }
    #[inline]
    fn scrollbar_width(&self) -> f32 {
        self.inner.scrollbar_width()
    }
    #[inline]
    fn position(&self) -> taffy::Position {
        self.inner.position()
    }
    #[inline]
    fn inset(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        self.inner.inset()
    }
    #[inline]
    fn min_size(&self) -> taffy::Size<taffy::Dimension> {
        self.inner.min_size()
    }
    #[inline]
    fn max_size(&self) -> taffy::Size<taffy::Dimension> {
        self.inner.max_size()
    }
    #[inline]
    fn aspect_ratio(&self) -> Option<f32> {
        self.inner.aspect_ratio()
    }
    #[inline]
    fn margin(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        self.inner.margin()
    }
    #[inline]
    fn padding(&self) -> taffy::Rect<taffy::LengthPercentage> {
        self.inner.padding()
    }
    #[inline]
    fn border(&self) -> taffy::Rect<taffy::LengthPercentage> {
        self.inner.border()
    }
}

impl taffy::BlockItemStyle for CssStyle {
    #[inline]
    fn is_table(&self) -> bool {
        taffy::BlockItemStyle::is_table(&self.inner)
    }
    #[inline]
    fn float(&self) -> taffy::Float {
        stylo_taffy::convert::float(self.inner.0.clone_float())
    }
    #[inline]
    fn clear(&self) -> taffy::Clear {
        stylo_taffy::convert::clear(self.inner.0.clone_clear())
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

    /// Style for `n` as a `CssStyle`, baking in the replaced (`<img>`)
    /// definite size so the parent's block layout sizes it intrinsically
    /// rather than stretching the auto-width box.
    #[inline]
    fn css_style(&self, n: NodeId) -> CssStyle {
        let node = self.node(n);
        let inner = TaffyStyloStyle(node.style.clone());
        match node.replaced_size {
            Some((w, h)) => CssStyle::with_size(
                inner,
                taffy::Size {
                    width: taffy::Dimension::length(w),
                    height: taffy::Dimension::length(h),
                },
            ),
            None => CssStyle::new(inner),
        }
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
                    let style = tree.css_style(node);
                    match tree.tree.nodes[key].replaced_size {
                        // Replaced element: definite size (intrinsic/CSS).
                        // `css_style` already forced the leaf's `size`, so
                        // the measure value is only a fallback.
                        Some((w, h)) => taffy::compute_leaf_layout(
                            inputs,
                            &style,
                            |_, _| 0.0,
                            |_, _| Size { width: w, height: h },
                        ),
                        // Text / inline formatting context: parley measures.
                        None => taffy::compute_leaf_layout(
                            inputs,
                            &style,
                            |_, _| 0.0,
                            |known, avail| match &tree.tree.nodes[key].inline_content {
                                Some(content) => measure_inline_content(
                                    tree.text_ctx,
                                    content,
                                    node,
                                    known,
                                    avail,
                                ),
                                None => Size::ZERO,
                            },
                        ),
                    }
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
        = CssStyle
    where
        Self: 'b;

    #[inline]
    fn get_block_container_style(&self, node: NodeId) -> NodeStyle {
        self.get_core_container_style(node)
    }

    #[inline]
    fn get_block_child_style(&self, child: NodeId) -> CssStyle {
        self.css_style(child)
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

/// Above this many inline leaves the shaping pre-pass fans out across Rayon;
/// at or below it the leaves shape inline. Small DOMs (chrome UI) stay
/// single-threaded — a work-stealing pool's spin-up costs more than the handful
/// of leaves saves. Tuned conservatively; revisit against a real profile.
const PARALLEL_SHAPE_THRESHOLD: usize = 24;

/// Shape every visible inline leaf's text into its (unbroken) `Layout` and cache
/// it (plus each inline-block sublayout) in `text_ctx`, ahead of Taffy layout.
/// This is the width-independent half of inline measurement (see [`shape_leaf`]);
/// the serial measure walk then only re-breaks the cached layouts per probed
/// width. Above [`PARALLEL_SHAPE_THRESHOLD`] leaves the shaping fans out across
/// Rayon, each worker driving its own cloned `FontContext` (the fontique
/// `Collection` is `Arc`-shared, so the clone is cheap) and a fresh
/// `LayoutContext`; the results merge into the caches single-threaded. Shaping is
/// deterministic, so the parallel and serial paths produce identical layouts.
fn shape_inline_leaves<D>(tree: &BoxTree<D::NodeId>, text_ctx: &mut TextMeasureCtx)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync,
{
    // Visible inline leaves only. A `display: none` leaf is never measured or
    // painted, so skip it; a leaf under a `display: none` *ancestor* is shaped
    // here but harmlessly never read (paint skips the hidden subtree).
    let leaves: Vec<(NodeId, usize)> = (0..tree.nodes.len())
        .filter(|&i| {
            tree.nodes[i].inline_content.is_some()
                && !tree.nodes[i].style.get_box().display.is_none()
        })
        .map(|i| (nid(i), i))
        .collect();
    if leaves.is_empty() {
        return;
    }

    // Shape each leaf into its unbroken `Layout` + its inline-block sublayouts.
    type Shaped = (
        NodeId,
        parley::Layout<ColorBrush>,
        Vec<(usize, parley::Layout<ColorBrush>)>,
    );
    let shaped: Vec<Shaped> = if leaves.len() >= PARALLEL_SHAPE_THRESHOLD {
        use rayon::prelude::*;
        // Each worker clones the base font context (cheap — shared `Collection`)
        // and spins up its own scratch `LayoutContext`.
        let base_font_ctx = &text_ctx.font_ctx;
        leaves
            .par_iter()
            .map_init(
                || (base_font_ctx.clone(), LayoutContext::<ColorBrush>::new()),
                |(font_ctx, layout_ctx), &(tid, i)| {
                    let content = tree.nodes[i].inline_content.as_ref().unwrap();
                    let (layout, subs) = shape_leaf(font_ctx, layout_ctx, content);
                    (tid, layout, subs)
                },
            )
            .collect()
    } else {
        leaves
            .iter()
            .map(|&(tid, i)| {
                let content = tree.nodes[i].inline_content.as_ref().unwrap();
                let (layout, subs) =
                    shape_leaf(&mut text_ctx.font_ctx, &mut text_ctx.layout_ctx, content);
                (tid, layout, subs)
            })
            .collect()
    };

    for (tid, layout, subs) in shaped {
        text_ctx.layouts.insert(tid, layout);
        for (i, l) in subs {
            text_ctx.inline_block_layouts.insert((tid, i), l);
        }
    }
}

/// Lay out `dom` via the box tree against `viewport`, into the caller-held
/// `text_ctx` (reset per pass; its persistent font context is reused so a
/// steady-state frame runs no font discovery). Returns the per-node
/// [`FragmentPlane`] and the [`BoxTree`]; the cached parley layouts for paint
/// emission live in `text_ctx`.
pub fn layout_via_box_tree<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    viewport: Size<AvailableSpace>,
    text_ctx: &mut TextMeasureCtx,
) -> (FragmentPlane<D::NodeId>, BoxTree<D::NodeId>)
where
    D: LayoutDom,
    // `Send + Sync` lets the shaping pre-pass fan inline leaves across Rayon. The
    // two real DOM node ids (`StaticNodeId`, scripted `NodeId`) are `usize`
    // newtypes, so this is free for them.
    D::NodeId: Copy + Eq + Hash + Send + Sync,
{
    text_ctx.reset();
    let mut tree = build_box_tree(dom, styles, images);
    let root = nid(tree.root);

    // Shaping pre-pass. Inline-text shaping (glyph runs, font resolution) is the
    // expensive, width-independent half of inline measurement; line breaking is
    // the cheap, width-dependent half. Shape every visible inline leaf up front
    // and cache the unbroken `Layout`s, so Taffy's serial measure walk below only
    // re-breaks them per probed width (min-content / max-content / final) instead
    // of re-shaping. Large trees fan the shaping across a Rayon pool; small trees
    // (chrome UI) shape inline, where pool spin-up would not pay off. Shaping is
    // pure, so the parallel output is identical to serial — no pixel difference.
    shape_inline_leaves::<D>(&tree, text_ctx);

    {
        let mut view = BoxTreeView {
            tree: &mut tree,
            text_ctx,
        };
        taffy::compute_root_layout(&mut view, root, viewport);
        taffy::round_layout(&mut view, root);
    }

    // Shape each list item's marker into a one-line parley layout keyed by the
    // item's Taffy id, so paint can hang it to the left of the content box.
    for i in 0..tree.nodes.len() {
        if let Some(run) = tree.nodes[i].marker.as_ref().and_then(|m| m.runs.first()) {
            text_ctx.shape_marker(run, nid(i));
        }
        // `text-overflow: ellipsis` leaves: shape `…` in the leaf's own font so
        // paint can draw it where it truncates an overflowing line. Keyed by the
        // leaf's Taffy id, alongside its text layout.
        let ellipsis_style = tree.nodes[i]
            .inline_content
            .as_ref()
            .and_then(|c| c.runs.first())
            .filter(|_| {
                crate::paint_emit::primary_cv(styles, tree.nodes[i].source.dom_id())
                    .as_deref()
                    .is_some_and(crate::paint_emit::text_ellipsis)
            })
            .cloned();
        if let Some(run) = ellipsis_style {
            text_ctx.shape_ellipsis(&run, nid(i));
        }
    }

    let mut fragments = FragmentPlane::new();
    for (dom_id, taffy_id) in tree.node_map.iter() {
        fragments.insert(*dom_id, tree.nodes[idx(*taffy_id)].final_layout);
    }

    (fragments, tree)
}

#[cfg(test)]
mod tests {
    //! Absolute-geometry checks for the box tree. (These began as a
    //! diff-test against the `TaffyTree`/`cv_to_taffy` oracle; once the
    //! box tree reached parity and the oracle was retired, they became
    //! direct assertions on the resulting `FragmentPlane`. The full
    //! HTML→pixel corpus runs through the box tree in
    //! `components/paint/tests/html_to_pixels_e2e.rs`.)

    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::cascade::run_cascade;

    const VIEWPORT: f32 = 128.0;

    /// Cascade + box-tree layout a fixture, returning the fragment plane.
    fn lay(html: &str, sheets: &[&str]) -> (StaticDocument, FragmentPlane<StaticNodeId>) {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::Size2D::new(VIEWPORT, VIEWPORT), sheets, None);
        let images = ImagePlane::decode_from_dom(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(VIEWPORT),
            height: AvailableSpace::Definite(VIEWPORT),
        };
        let mut text_ctx = TextMeasureCtx::new();
        let (fragments, _tree) =
            layout_via_box_tree(&document, &styles, &images, viewport, &mut text_ctx);
        (document, fragments)
    }

    /// Elements with the given local name, in document (pre-order) order.
    fn find_all(doc: &StaticDocument, local: html5ever::LocalName) -> Vec<StaticNodeId> {
        let mut out = Vec::new();
        let mut stack = vec![doc.document()];
        // Pre-order: push children reversed so siblings pop in order.
        let mut order = Vec::new();
        while let Some(id) = stack.pop() {
            order.push(id);
            let kids: Vec<_> = doc.dom_children(id).collect();
            for k in kids.into_iter().rev() {
                stack.push(k);
            }
        }
        for id in order {
            if doc.element_name(id).is_some_and(|q| q.local == local) {
                out.push(id);
            }
        }
        out
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// Two plain block divs stack vertically: the second sits below the
    /// first (relative to their shared parent), not overlapping at the
    /// origin.
    #[test]
    fn block_siblings_stack_vertically() {
        let (doc, frags) = lay(
            "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
            &[".a { width: 60px; height: 40px; }", ".b { width: 60px; height: 40px; }"],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        let a = frags.rect_of(divs[0]).expect(".a fragment");
        let b = frags.rect_of(divs[1]).expect(".b fragment");
        assert!(approx(a.location.y, 0.0), ".a at top, got y={}", a.location.y);
        assert!(approx(a.size.height, 40.0), ".a height 40, got {}", a.size.height);
        assert!(approx(b.location.y, 40.0), ".b stacks below .a (y=40), got y={}", b.location.y);
    }

    /// `::before` / `::after` with string `content` generate inline runs around
    /// the element's own content, ordered before/after it, each carrying the
    /// pseudo's *own* cascaded style (not the element's).
    #[test]
    fn pseudo_before_after_generate_styled_runs() {
        use crate::construct::gather_inline_content;

        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(VIEWPORT, VIEWPORT),
            &[
                "p { color: rgb(0, 0, 255); }",
                "p::before { content: \"X\"; color: rgb(255, 0, 0); }",
                "p::after { content: \"Z\"; }",
            ],
            None,
        );
        let images = ImagePlane::decode_from_dom(&document);
        let p = find_all(&document, html5ever::local_name!("p"))[0];
        let content = gather_inline_content(&document, &styles, &images, NodeRef::new(&document, p));

        let texts: Vec<&str> = content.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts.first().copied(), Some("X"), "::before run first, got {texts:?}");
        assert_eq!(texts.last().copied(), Some("Z"), "::after run last, got {texts:?}");
        assert!(texts.iter().any(|t| t.contains("hi")), "element text present, got {texts:?}");

        // The ::before run uses the pseudo's own red color, the text run the
        // element's blue — proving run_from_computed reads the pseudo cascade.
        let before = content.runs.iter().find(|r| r.text == "X").expect("::before run");
        assert!(
            before.color[0] > 0.99 && before.color[2] < 0.01,
            "::before is its own red, got {:?}",
            before.color
        );
        let hi = content.runs.iter().find(|r| r.text.contains("hi")).expect("text run");
        assert!(hi.color[2] > 0.99 && hi.color[0] < 0.01, "element text is blue, got {:?}", hi.color);
    }

    /// `::first-letter` splits the block's first run at the first typographic
    /// letter, giving that letter its own cascaded style (here red on otherwise
    /// blue text). The remainder keeps the element's style, and leading
    /// punctuation rides with the letter. (Pseudo follow-ups §4.)
    #[test]
    fn first_letter_splits_and_styles_the_opening_letter() {
        use crate::construct::gather_inline_content;

        let document = StaticDocument::parse("<html><body><p>(Hello world</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(VIEWPORT, VIEWPORT),
            &[
                "p { color: rgb(0, 0, 255); }",
                "p::first-letter { color: rgb(255, 0, 0); }",
            ],
            None,
        );
        let images = ImagePlane::decode_from_dom(&document);
        let p = find_all(&document, html5ever::local_name!("p"))[0];
        let content = gather_inline_content(&document, &styles, &images, NodeRef::new(&document, p));

        let texts: Vec<&str> = content.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts.first().copied(), Some("(H"), "leading punct rides the letter, got {texts:?}");
        assert_eq!(
            content.runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            "(Hello world",
            "split preserves the text exactly"
        );

        let first = &content.runs[0];
        assert!(
            first.color[0] > 0.99 && first.color[2] < 0.01,
            "::first-letter is red, got {:?}",
            first.color
        );
        let rest = &content.runs[1];
        assert!(
            rest.color[2] > 0.99 && rest.color[0] < 0.01,
            "remainder keeps the element's blue, got {:?}",
            rest.color
        );
    }

    /// No `::first-letter` rule → the run is not split (one run for the text).
    #[test]
    fn no_first_letter_rule_leaves_one_run() {
        use crate::construct::gather_inline_content;

        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::Size2D::new(VIEWPORT, VIEWPORT), &[], None);
        let images = ImagePlane::decode_from_dom(&document);
        let p = find_all(&document, html5ever::local_name!("p"))[0];
        let content = gather_inline_content(&document, &styles, &images, NodeRef::new(&document, p));
        assert_eq!(content.runs.len(), 1, "no split without a ::first-letter rule");
    }

    /// A list marker takes its `::marker` pseudo's cascade when present, so
    /// `li::marker { color }` recolors the bullet (not the item's own color) —
    /// the lazy `::marker` is resolved into the plane during the cascade.
    #[test]
    fn marker_uses_marker_pseudo_style() {
        use crate::construct::list_marker_content;

        let document = StaticDocument::parse("<html><body><ul><li>item</li></ul></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(VIEWPORT, VIEWPORT),
            &["li { color: rgb(0, 0, 255); }", "li::marker { color: rgb(255, 0, 0); }"],
            None,
        );
        let li = find_all(&document, html5ever::local_name!("li"))[0];
        let content = list_marker_content(&document, &styles, li).expect("li has a marker");
        let run = &content.runs[0];
        assert!(
            run.color[0] > 0.99 && run.color[2] < 0.01,
            "::marker recolors the bullet red, got {:?}",
            run.color
        );
    }

    /// `white-space: nowrap` lays the text on a single line even when it overflows
    /// a narrow box — the same text without it wraps to several lines. (Chrome-UI
    /// truncated-label support.)
    #[test]
    fn white_space_nowrap_stays_one_line() {
        let line_count = |nowrap: bool| {
            let document =
                StaticDocument::parse("<html><body><p>one two three four five six</p></body></html>");
            let ws = if nowrap { "white-space: nowrap;" } else { "" };
            let sheet = format!("p {{ display: block; width: 40px; font-size: 16px; {ws} }}");
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &document,
                &mut styles,
                euclid::Size2D::new(VIEWPORT, VIEWPORT),
                &[sheet.as_str()],
                None,
            );
            let images = ImagePlane::decode_from_dom(&document);
            let viewport = Size {
                width: AvailableSpace::Definite(VIEWPORT),
                height: AvailableSpace::Definite(VIEWPORT),
            };
            let mut text_ctx = TextMeasureCtx::new();
            let (_f, built) =
                layout_via_box_tree(&document, &styles, &images, viewport, &mut text_ctx);
            let p = find_all(&document, html5ever::local_name!("p"))[0];
            let taffy_id = *built.node_map.get(&p).expect("p box");
            text_ctx.layouts.get(&taffy_id).expect("p text laid out").len()
        };
        assert_eq!(line_count(true), 1, "nowrap → a single line");
        assert!(line_count(false) > 1, "wrapping → multiple lines in a 40px box");
    }

    /// A block-`display` `::before` / `::after` becomes a synthetic block box
    /// child (first / last), laid out in block flow: each stretches to the
    /// container width and stacks vertically, with the element's own text between.
    /// The boxes carry [`BoxSource::Pseudo`] (routing hits to the element) and no
    /// `node_map` entry. (Pseudo follow-ups §5.)
    #[test]
    fn block_before_after_pseudo_become_block_children() {
        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(VIEWPORT, VIEWPORT),
            &[
                "html, body, p { display: block; margin: 0; width: 100px; }",
                "p::before { content: \"X\"; display: block; height: 20px; }",
                "p::after { content: \"Y\"; display: block; height: 10px; }",
            ],
            None,
        );
        let images = ImagePlane::decode_from_dom(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(VIEWPORT),
            height: AvailableSpace::Definite(VIEWPORT),
        };
        let mut text_ctx = TextMeasureCtx::new();
        let (_fragments, built) =
            layout_via_box_tree(&document, &styles, &images, viewport, &mut text_ctx);

        let p = find_all(&document, html5ever::local_name!("p"))[0];
        let p_node = built.node(idx(*built.node_map.get(&p).expect("p box")));
        assert_eq!(p_node.children.len(), 3, "::before + anon(text) + ::after");

        let before = built.node(p_node.children[0]);
        let after = built.node(p_node.children[2]);
        assert!(
            matches!(before.source, BoxSource::Pseudo(_, PseudoKind::Before)),
            "first child is the ::before pseudo box"
        );
        assert!(
            matches!(after.source, BoxSource::Pseudo(_, PseudoKind::After)),
            "last child is the ::after pseudo box"
        );

        // Block flow: each pseudo box stretches to the 100px container width and
        // takes its own height; ::before is at the top, ::after below the text.
        assert!(approx(before.final_layout.size.width, 100.0), "::before stretches to width");
        assert!(approx(before.final_layout.size.height, 20.0), "::before is 20px tall");
        assert!(approx(before.final_layout.location.y, 0.0), "::before at the top");
        assert!(
            after.final_layout.location.y > before.final_layout.location.y,
            "::after ({}) sits below ::before ({})",
            after.final_layout.location.y,
            before.final_layout.location.y
        );

        // Not script-visible: the pseudo boxes have no node_map entry.
        let pseudo_arenas = [p_node.children[0], p_node.children[2]];
        for (_, taffy) in built.node_map.iter() {
            assert!(
                !pseudo_arenas.contains(&idx(*taffy)),
                "pseudo box must not be in node_map"
            );
        }
    }

    /// One persistent `TextMeasureCtx` lays out two distinct documents
    /// correctly: `layout_via_box_tree` resets the per-pass caches each call, so
    /// the second pass is not corrupted by the first's stale Taffy-keyed layouts
    /// — the reuse that lets a session skip per-frame font discovery (C2).
    #[test]
    fn persistent_text_ctx_reused_across_distinct_layouts() {
        let viewport = Size {
            width: AvailableSpace::Definite(VIEWPORT),
            height: AvailableSpace::Definite(VIEWPORT),
        };
        let mut text_ctx = TextMeasureCtx::new();

        let lay_one = |w: u32, ctx: &mut TextMeasureCtx| -> f32 {
            let doc = StaticDocument::parse("<html><body><div class=\"x\">hi</div></body></html>");
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            let sheet = format!(".x {{ width: {w}px; height: 20px; }}");
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(VIEWPORT, VIEWPORT),
                &[sheet.as_str()],
                None,
            );
            let images = ImagePlane::decode_from_dom(&doc);
            let (frags, _tree) = layout_via_box_tree(&doc, &styles, &images, viewport, ctx);
            let d = find_all(&doc, html5ever::local_name!("div"))[0];
            frags.rect_of(d).expect("div fragment").size.width
        };

        // Two passes through the SAME context, different widths.
        assert!(approx(lay_one(30, &mut text_ctx), 30.0), "first pass width 30");
        assert!(approx(lay_one(50, &mut text_ctx), 50.0), "reused-ctx second pass width 50");
    }

    /// Block-level floats: two `float: left` divs sit side by side on one
    /// line (where plain blocks would stack). This is the box tree's
    /// float path through the `CssStyle` float/clear forwarding.
    #[test]
    fn float_left_places_blocks_side_by_side() {
        let (doc, frags) = lay(
            "<html><body><div class=\"a\"></div><div class=\"b\"></div></body></html>",
            &[
                ".a { float: left; width: 40px; height: 40px; }",
                ".b { float: left; width: 40px; height: 40px; }",
            ],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        let a = frags.rect_of(divs[0]).expect(".a fragment");
        let b = frags.rect_of(divs[1]).expect(".b fragment");
        assert!(approx(a.location.x, 0.0), ".a at left, got x={}", a.location.x);
        assert!(approx(b.location.x, 40.0), ".b beside .a (x=40), got x={}", b.location.x);
        assert!(approx(b.location.y, 0.0), ".b on the same line as .a (y=0), got y={}", b.location.y);
    }

    /// `position: relative` offsets the box by its inset from in-flow.
    #[test]
    fn relative_position_offsets_box() {
        let (doc, frags) = lay(
            "<html><body><div></div></body></html>",
            &["div { width: 30px; height: 30px; position: relative; top: 20px; left: 20px; }"],
        );
        let div = find_all(&doc, html5ever::local_name!("div"))[0];
        let r = frags.rect_of(div).expect("div fragment");
        assert!(approx(r.location.x, 20.0), "left:20 → x=20, got {}", r.location.x);
        assert!(approx(r.location.y, 20.0), "top:20 → y=20, got {}", r.location.y);
    }

    /// `position: absolute` takes the box out of flow and places it by its
    /// inset relative to the nearest positioned ancestor (the `relative`
    /// container) — overlapping the in-flow sibling rather than stacking after
    /// it. This is the layout half of host overlays/popups: an absolutely-placed
    /// layer atop normal content.
    #[test]
    fn absolute_position_places_box_over_container() {
        let (doc, frags) = lay(
            "<html><body><div class=\"box\">\
                <div class=\"flow\"></div><div class=\"pop\"></div>\
            </div></body></html>",
            &[
                ".box { position: relative; width: 200px; height: 200px; }",
                ".flow { width: 80px; height: 80px; }",
                ".pop { position: absolute; top: 10px; left: 30px; width: 50px; height: 50px; }",
            ],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        // divs in document order: [.box, .flow, .pop]
        let flow = frags.rect_of(divs[1]).expect(".flow fragment");
        let pop = frags.rect_of(divs[2]).expect(".pop fragment");
        // In-flow sibling at the container origin.
        assert!(approx(flow.location.y, 0.0), ".flow in flow at y=0, got {}", flow.location.y);
        // Absolute box placed by its own inset, not after the sibling.
        assert!(approx(pop.location.x, 30.0), "left:30 → x=30, got {}", pop.location.x);
        assert!(approx(pop.location.y, 10.0), "top:10 → y=10, got {}", pop.location.y);
        assert!(approx(pop.size.width, 50.0), ".pop width 50, got {}", pop.size.width);
    }

    /// Inline `style="…"` cascades and drives layout: a box positioned by
    /// inline-style insets lands at those insets over its positioned container —
    /// the same outcome as the stylesheet-driven
    /// [`absolute_position_places_box_over_container`], proving the stylo
    /// adapter's `style_attribute()` is wired end-to-end (parse → cascade →
    /// layout). This is the engine half of host overlays/popups: an overlay can
    /// carry its dynamic `(x, y)` in an inline style.
    #[test]
    fn inline_style_drives_layout() {
        let (doc, frags) = lay(
            "<html><body>\
                <div class=\"box\">\
                    <div style=\"position: absolute; top: 15px; left: 25px; \
                        width: 40px; height: 40px;\"></div>\
                </div>\
            </body></html>",
            &[".box { position: relative; width: 200px; height: 200px; }"],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        // [.box, the inline-styled child]
        let pop = frags.rect_of(divs[1]).expect("inline-styled box fragment");
        assert!(approx(pop.location.x, 25.0), "inline left:25 → x=25, got {}", pop.location.x);
        assert!(approx(pop.location.y, 15.0), "inline top:15 → y=15, got {}", pop.location.y);
        assert!(approx(pop.size.width, 40.0), "inline width:40 → w=40, got {}", pop.size.width);
    }

    /// A percentage inset on an absolutely-positioned box resolves against its
    /// containing block: `top: 100%` lands the box at the bottom of its
    /// positioned ancestor. This is the layout basis for a self-positioning
    /// dropdown (`top: 100%` puts the option list directly below the select
    /// box) — no host rect query needed.
    #[test]
    fn absolute_percent_inset_resolves_against_container() {
        let (doc, frags) = lay(
            "<html><body><div class=\"box\"><div class=\"pop\"></div></div></body></html>",
            &[
                ".box { position: relative; width: 100px; height: 60px; }",
                ".pop { position: absolute; top: 100%; left: 0; width: 50px; height: 20px; }",
            ],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        let pop = frags.rect_of(divs[1]).expect(".pop fragment");
        // top: 100% of the 60px-tall container → y = 60 (the box's bottom edge).
        assert!(approx(pop.location.y, 60.0), "top:100% → y=60, got {}", pop.location.y);
    }

    /// Border-box layout: content `width/height: 40` + `border: 10`
    /// each side lays out a 60×60 border box (CSS content-box default).
    #[test]
    fn border_adds_to_box_size() {
        let (doc, frags) = lay(
            "<html><body><div></div></body></html>",
            &["div { width: 40px; height: 40px; border: 10px solid rgb(0,128,0); }"],
        );
        let div = find_all(&doc, html5ever::local_name!("div"))[0];
        let r = frags.rect_of(div).expect("div fragment");
        assert!(approx(r.size.width, 60.0), "40 content + 20 border = 60, got {}", r.size.width);
        assert!(approx(r.size.height, 60.0), "40 content + 20 border = 60, got {}", r.size.height);
    }

    /// Replaced element: a lone `<img>` (data URI) takes its decoded
    /// intrinsic size (16×16) — the box tree sizes it via the measured
    /// leaf + `get_block_child_style` size override, not by stretching.
    #[test]
    fn img_takes_intrinsic_size() {
        let html = img_html();
        let (doc, frags) = lay(&html, &[]);
        let img = find_all(&doc, html5ever::local_name!("img"))[0];
        let r = frags.rect_of(img).expect("img fragment");
        assert!(approx(r.size.width, 16.0), "intrinsic width 16, got {}", r.size.width);
        assert!(approx(r.size.height, 16.0), "intrinsic height 16, got {}", r.size.height);
    }

    /// A definite CSS `width` overrides the intrinsic on that axis;
    /// the unspecified `height` stays intrinsic.
    #[test]
    fn img_css_width_overrides_intrinsic() {
        let html = img_html();
        let (doc, frags) = lay(&html, &["img { width: 50px; }"]);
        let img = find_all(&doc, html5ever::local_name!("img"))[0];
        let r = frags.rect_of(img).expect("img fragment");
        assert!(approx(r.size.width, 50.0), "css width 50, got {}", r.size.width);
        assert!(approx(r.size.height, 16.0), "intrinsic height 16, got {}", r.size.height);
    }

    /// Two absolutely-positioned siblings with `top: auto` both resolve to the
    /// same static position (the top of their containing block), since each is
    /// out of flow and contributes no height to the other — they overlap rather
    /// than stack. The structure of the `tiled-radial-gradients` reference.
    #[test]
    fn two_absolute_siblings_share_static_position() {
        let (doc, frags) = lay(
            "<html><body><div class=\"outer\">\
                <div class=\"left\"></div><div class=\"right\"></div>\
            </div></body></html>",
            &[
                ".outer { position: absolute; width: 600px; height: 200px; }",
                ".left, .right { position: absolute; width: 300px; height: 200px; }",
                ".left { left: 80px; }",
                ".right { left: 380px; }",
            ],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        // [.outer, .left, .right]
        let left = frags.rect_of(divs[1]).expect(".left fragment");
        let right = frags.rect_of(divs[2]).expect(".right fragment");
        assert!(approx(left.location.x, 80.0), ".left left:80 → x=80, got {}", left.location.x);
        assert!(approx(right.location.x, 380.0), ".right left:380 → x=380, got {}", right.location.x);
        assert!(approx(left.location.y, 0.0), ".left static y=0, got {}", left.location.y);
        assert!(approx(right.location.y, 0.0), ".right static y=0 (not stacked), got {}", right.location.y);
    }

    /// Whitespace-only text between block children is collapsible and generates
    /// no box: two stacked 50px blocks land at y=0 and y=50 even with newlines +
    /// indentation between them in the source. Without the skip, the inter-block
    /// whitespace would add a stray line box and push the second block down.
    #[test]
    fn whitespace_between_blocks_generates_no_box() {
        let (doc, frags) = lay(
            "<html><body><div class=\"a\"></div>\n   \n  <div class=\"b\"></div></body></html>",
            &[".a, .b { display: block; height: 50px; width: 50px; }"],
        );
        let divs = find_all(&doc, html5ever::local_name!("div"));
        let a = frags.rect_of(divs[0]).expect(".a fragment");
        let b = frags.rect_of(divs[1]).expect(".b fragment");
        assert!(approx(a.location.y, 0.0), ".a at y=0, got {}", a.location.y);
        assert!(
            approx(b.location.y, 50.0),
            ".b directly after .a, no whitespace gap, got {}",
            b.location.y
        );
    }

    /// Two replaced `<img>`s in a div (no text) flow side by side: the div
    /// establishes an inline context, so its width spans both imgs rather than
    /// stacking them as block children. (CSS-sized so no decode is needed.)
    #[test]
    fn two_inline_images_flow_side_by_side() {
        let (doc, frags) = lay(
            "<html><body><div>\
                <img style=\"width:16px;height:16px\"/>\
                <img style=\"width:16px;height:16px\"/>\
            </div></body></html>",
            &[],
        );
        let div = find_all(&doc, html5ever::local_name!("div"))[0];
        let r = frags.rect_of(div).expect("div fragment");
        assert!(
            r.size.width >= 32.0,
            "two imgs flow side by side (width >= 2*16), got {}",
            r.size.width
        );
    }

    /// An `<iframe>` with no intrinsic content + no CSS size takes the CSS
    /// default object size, 300×150 (it is a replaced element).
    #[test]
    fn iframe_uses_default_object_size() {
        let (doc, frags) = lay("<html><body><iframe></iframe></body></html>", &[]);
        let iframe = find_all(&doc, html5ever::local_name!("iframe"))[0];
        let r = frags.rect_of(iframe).expect("iframe fragment");
        assert!(approx(r.size.width, 300.0), "iframe default width 300, got {}", r.size.width);
        assert!(approx(r.size.height, 150.0), "iframe default height 150, got {}", r.size.height);
    }

    /// A 16×16 blue PNG as a data-URI `<img>` document.
    fn img_html() -> String {
        use base64::Engine as _;
        let blue = image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 0, 255, 255]));
        let mut png = Vec::new();
        blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode test PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        format!("<html><body><img src=\"data:image/png;base64,{b64}\"></body></html>")
    }
}
