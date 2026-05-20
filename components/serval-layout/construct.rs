/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM walk → Taffy tree construction.
//!
//! Walks a `LayoutDom` via `NodeRef`'s structural primitives, attaches
//! the style entry from `StylePlane`, and builds a
//! `taffy::TaffyTree<InlineContent>` ready for Taffy's
//! `compute_layout_with_measure`.
//!
//! ## Box generation
//!
//! - A **block** element (children that aren't all inline) becomes a
//!   Taffy node with child boxes; `build_children` recurses.
//! - An element that **establishes an inline formatting context** (all
//!   children inline — text + `display:inline` elements) becomes a
//!   single Taffy **leaf** whose [`InlineContent`] gathers the inline
//!   subtree's text into per-run styled spans. parley lays the runs
//!   out together (text + inline elements flow on shared lines). The
//!   inline children don't get their own Taffy nodes.
//! - A bare **text** node in a non-inline (mixed / block) parent
//!   becomes a one-run inline leaf.
//!
//! Inline-context detection reads the cascade's `display`; with no
//! cascade (hand-rolled style fixtures) every element is treated as
//! block, preserving the pre-inline behavior.
//!
//! Returns the constructed Taffy tree, the root Taffy NodeId, and a
//! `NodeId → taffy::NodeId` mapping so callers can read layout results
//! back keyed by their DOM identity.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use rustc_hash::FxHashMap;
use taffy::TaffyTree;
use taffy::prelude::TaffyAuto;

use crate::adapter::NodeRef;
use crate::style::StylePlane;
use crate::text_measure::{FontFamilySpec, GenericFamilyKind, InlineContent, InlineRun};

/// Default font size used for runs whose element has no cascaded
/// `font-size` (hand-rolled style fixtures). 16 px matches the
/// CSS/UA-stylesheet convention and parley's own default.
const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Output of construction: the Taffy tree, the root, and the DOM↔Taffy id
/// mapping for reading results back. The tree is parameterized by
/// [`InlineContent`] so inline leaves carry their styled text runs
/// through to the measure function.
pub struct ConstructedTree<NodeId: Copy + Eq + Hash> {
    pub tree: TaffyTree<InlineContent>,
    pub root: taffy::NodeId,
    /// DOM NodeId → Taffy NodeId. Sparse only for nodes that don't get
    /// Taffy entries (e.g., comments, the document node when treated
    /// as a synthetic root wrapper); element and text nodes are both
    /// present.
    pub node_map: FxHashMap<NodeId, taffy::NodeId>,
}

/// Build a Taffy tree from a `LayoutDom` rooted at `dom.document()`,
/// reading style from `styles`. Element nodes become Taffy nodes with
/// child lists; text nodes become Taffy leaves carrying [`TextLeaf`]
/// context. Comment / processing-instruction nodes are still skipped.
///
/// The Taffy root is a synthetic node wrapping the document; its Taffy
/// style defaults to a viewport-shaped block container.
pub fn construct<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    viewport: taffy::Size<taffy::AvailableSpace>,
) -> ConstructedTree<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut tree: TaffyTree<InlineContent> = TaffyTree::new();
    let mut node_map: FxHashMap<D::NodeId, taffy::NodeId> = FxHashMap::default();

    let root_ref = NodeRef::document(dom);
    let root_children = build_children(dom, styles, root_ref, &mut tree, &mut node_map);

    // Synthetic root takes the viewport dimensions explicitly. This
    // is the initial containing block — `<html>`'s `width: 100%` /
    // `height: 100%` (from the UA defaults) resolves against this
    // size, which transitively gives `<body>` and its descendants a
    // base to measure against. Without explicit dimensions on the
    // root, percentage sizes have nothing to resolve to and an empty
    // document lays out as 0×0.
    let root_style = taffy::Style {
        display: taffy::Display::Block,
        size: taffy::Size {
            width: available_space_to_dimension(viewport.width),
            height: available_space_to_dimension(viewport.height),
        },
        ..Default::default()
    };
    let root = tree
        .new_with_children(root_style, &root_children)
        .expect("Taffy: failed to create root");

    ConstructedTree { tree, root, node_map }
}

/// Translate Taffy's `AvailableSpace` (the layout-time constraint)
/// into a `Dimension` (the style-time size). `Definite(v)` becomes
/// an explicit pixel length; `MinContent` / `MaxContent` collapse to
/// `Auto`, since the root has nothing larger to size against.
fn available_space_to_dimension(a: taffy::AvailableSpace) -> taffy::Dimension {
    match a {
        taffy::AvailableSpace::Definite(v) => taffy::Dimension::length(v),
        _ => taffy::Dimension::AUTO,
    }
}

/// Recursively build Taffy nodes for `parent`'s element + text
/// descendants and return the list of Taffy NodeIds for them in DOM
/// order.
fn build_children<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    parent: NodeRef<'a, D>,
    tree: &mut TaffyTree<InlineContent>,
    node_map: &mut FxHashMap<D::NodeId, taffy::NodeId>,
) -> Vec<taffy::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut children = Vec::new();
    for child in parent.dom_children() {
        let taffy_id = match dom.kind(child.id()) {
            NodeKind::Element => {
                let style = styles.taffy_style(child.id());
                if establishes_inline_context(dom, styles, child) {
                    // Inline formatting context: gather the inline
                    // subtree into one measured leaf; don't recurse
                    // (inline children have no Taffy nodes of their own).
                    let content = gather_inline_content(dom, styles, child);
                    tree.new_leaf_with_context(style, content)
                        .expect("Taffy: failed to create inline-context leaf")
                } else {
                    let grand = build_children(dom, styles, child, tree, node_map);
                    tree.new_with_children(style, &grand)
                        .expect("Taffy: failed to create element node")
                }
            }
            NodeKind::Text => {
                let text = dom.text(child.id()).unwrap_or("").to_string();
                // Bare text in a block context: one run styled by the
                // parent element (size / family / weight / italic / color).
                let content = InlineContent {
                    runs: vec![run_for_element(styles, parent.id(), text)],
                };
                tree.new_leaf_with_context(taffy::Style::default(), content)
                    .expect("Taffy: failed to create text leaf")
            }
            // Comments / processing instructions / document fragments
            // don't render — skip.
            _ => continue,
        };
        node_map.insert(child.id(), taffy_id);
        children.push(taffy_id);
    }
    children
}

/// Whether `elem` establishes an inline formatting context: it has at
/// least one child and every element child is `display:inline` (text
/// children are inline by nature). Comments / PIs are ignored. With no
/// cascade data (`is_inline_element` → `None`), the element is treated
/// as block — preserving the pre-inline behavior for hand-rolled
/// style fixtures.
fn establishes_inline_context<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut has_child = false;
    for child in elem.dom_children() {
        match dom.kind(child.id()) {
            NodeKind::Text => has_child = true,
            NodeKind::Element => {
                has_child = true;
                // Inline *and* not replaced. Replaced inline content
                // (`<img>`) needs parley InlineBox placeholders, which
                // the gather doesn't produce yet — so an element with a
                // replaced child isn't treated as a pure-inline-text
                // context. It stays block, and the replaced child keeps
                // its own box (renders as before). Mixing flowed text
                // with `<img>` on one line is a follow-up.
                if is_replaced(dom, child.id())
                    || !is_inline_element(styles, child.id()).unwrap_or(false)
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    has_child
}

/// Whether an element is replaced content we render as its own box
/// rather than as flowed inline text. v1: just `<img>`.
fn is_replaced<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom.element_name(id)
        .is_some_and(|q| q.local == html5ever::local_name!("img"))
}

/// Read an element's cascaded outer display: `Some(true)` for
/// `display:inline`, `Some(false)` for block-level, `None` when the
/// cascade hasn't run for this element.
fn is_inline_element<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::specified::box_::DisplayOutside;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let display = data.styles.primary().get_box().display;
    Some(matches!(display.outside(), DisplayOutside::Inline))
}

/// Gather an inline-context element's subtree into [`InlineContent`].
/// Walks in document order; each text node becomes a run styled by the
/// nearest enclosing inline element (which carries the cascade).
fn gather_inline_content<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
) -> InlineContent
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut runs = Vec::new();
    gather_runs(dom, styles, elem, &mut runs);
    InlineContent { runs }
}

/// Recursive helper for [`gather_inline_content`]. `node`'s direct
/// text children are styled by `node` (the enclosing inline element);
/// element children recurse with themselves as the new styling element.
fn gather_runs<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    node: NodeRef<'a, D>,
    runs: &mut Vec<InlineRun>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    for child in node.dom_children() {
        match dom.kind(child.id()) {
            NodeKind::Text => {
                let text = dom.text(child.id()).unwrap_or("").to_string();
                if !text.is_empty() {
                    runs.push(run_for_element(styles, node.id(), text));
                }
            }
            NodeKind::Element => gather_runs(dom, styles, child, runs),
            _ => {}
        }
    }
}

/// Build an [`InlineRun`] for `text` styled by element `id`'s cascade
/// (size / family / weight / italic), defaulting where the cascade
/// hasn't run.
fn run_for_element<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    text: String,
) -> InlineRun {
    InlineRun {
        text,
        font_size: font_size_of(styles, id).unwrap_or(DEFAULT_FONT_SIZE),
        font_family: font_family_of(styles, id).unwrap_or_default(),
        weight: font_weight_of(styles, id).unwrap_or(400.0),
        italic: font_italic_of(styles, id).unwrap_or(false),
        // Per-run color from the styling element's cascaded `color`.
        color: text_color_of(styles, id).unwrap_or([0.0, 0.0, 0.0, 1.0]),
    }
}

/// Read an element's cascaded text `color` as straight RGBA in
/// `[0, 1]`. `None` when the cascade hasn't run.
fn text_color_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<[f32; 4]> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let absolute = data.styles.primary().get_inherited_text().color;
    let srgb = absolute.into_srgb_legacy();
    Some(*srgb.raw_components())
}

/// Read an element's cascaded `font-size` in CSS px. Returns `None`
/// when the cascade hasn't been applied to that element (hand-rolled
/// style fixtures); the caller defaults to `DEFAULT_FONT_SIZE`.
fn font_size_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_size.computed_size().px())
}

/// Read an element's cascaded `font-family` and collapse the family
/// list to its first entry (probe scope — no fallback-chain walking).
/// Returns `None` when the cascade hasn't run for this element.
fn font_family_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<FontFamilySpec> {
    use style::values::computed::font::{GenericFontFamily, SingleFontFamily};

    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let first = primary.get_font().font_family.families.iter().next()?;
    let spec = match first {
        SingleFontFamily::FamilyName(name) => FontFamilySpec::Named(name.name.to_string()),
        SingleFontFamily::Generic(g) => {
            let kind = match g {
                GenericFontFamily::Serif => GenericFamilyKind::Serif,
                GenericFontFamily::SansSerif => GenericFamilyKind::SansSerif,
                GenericFontFamily::Monospace => GenericFamilyKind::Monospace,
                GenericFontFamily::Cursive => GenericFamilyKind::Cursive,
                GenericFontFamily::Fantasy => GenericFamilyKind::Fantasy,
                // None / SystemUi / other internal generics → sans-serif.
                _ => GenericFamilyKind::SansSerif,
            };
            FontFamilySpec::Generic(kind)
        },
    };
    Some(spec)
}

/// Read an element's cascaded numeric `font-weight` (400 normal, 700
/// bold). `None` when the cascade hasn't run.
fn font_weight_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_weight.value())
}

/// Whether an element's cascaded `font-style` is non-normal
/// (italic / oblique). `None` when the cascade hasn't run.
fn font_italic_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::computed::font::FontStyle;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_style != FontStyle::NORMAL)
}
