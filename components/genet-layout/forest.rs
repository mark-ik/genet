/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Forest DOM: one document, N window-root subtrees, each a retained layout
//! session at its own viewport.
//!
//! The forest-dom design (mere `2026-07-08_forest_dom_plan.md`): the web
//! `moveBefore` standard is **intra-document and throws across documents**, so
//! moving a live tile between windows *with its DOM node, scroll, focus, and
//! animation intact* requires the windows to be one document with distinct
//! window-root elements. The [F0 spike](crate::subtree::tests) proved the
//! mechanism (two subtree sessions relayout independently; a cross-root move
//! preserves identity); this is the reusable **session manager** over it,
//! packaging the plan's three capabilities as one host-neutral primitive:
//!
//! - **F1 (runner mounts at a node)**: a window is a window-root element the
//!   host builds content under; each window is laid out from its own root, not
//!   the document root ([`ForestDom::window_root`] + [`crate::layout_subtree`]).
//! - **F2 (per-subtree layout at (root, viewport, sheet))**: one
//!   [`IncrementalLayout`] per window at its own size / sheet.
//! - **F3 (mutation routing by root containment)**: [`ForestDom::relayout_for`]
//!   finds which window-root owns a mutated node and relayouts *only* that
//!   window; the others' retained layouts are untouched.
//!
//! [`ForestDom::move_to_window`] is the **tear-out primitive**: an in-document
//! `move_before` that reparents a tile subtree under another window-root,
//! preserving node identity, then relayouts the two affected windows.
//!
//! Coarse-but-correct: each relayout rebuilds a window's session from its
//! subtree. The incremental [`IncrementalLayout::apply`] path is a later
//! optimization; correctness (isolation + identity) does not depend on it.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};

use crate::{IncrementalLayout, layout_subtree, subtree::SubtreeView};

fn qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

/// The class marking a window-root element in the shared document.
pub const WINDOW_ROOT_CLASS: &str = "window-root";

/// A stable per-window handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowRootId(pub u64);

struct WindowRoot<Id: Copy + Eq + Hash> {
    id: WindowRootId,
    root: Id,
    layout: IncrementalLayout<Id>,
    width: f32,
    height: f32,
    sheets: Vec<String>,
}

/// One document, N window-root subtrees, each a retained layout session. The
/// host owns the shared DOM through this manager: it mints windows, builds
/// content under each window-root (via [`Self::dom_mut`]), relayouts the
/// window a mutation touched, and tears a tile out to another window.
pub struct ForestDom<D: LayoutDomMut>
where
    D::NodeId: Copy + Eq + Hash,
{
    dom: D,
    windows: Vec<WindowRoot<D::NodeId>>,
    next: u64,
}

impl<D> ForestDom<D>
where
    D: LayoutDomMut,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
{
    /// Wrap a document (typically freshly `ScriptedDom::new()`); no windows yet.
    pub fn new(dom: D) -> Self {
        Self {
            dom,
            windows: Vec::new(),
            next: 0,
        }
    }

    /// The shared document (read) — the host builds content under a
    /// window-root, hit-tests, and paints from here.
    pub fn dom(&self) -> &D {
        &self.dom
    }

    /// The shared document (mutate) — the host appends/moves/edits tile
    /// content under a window-root, then calls [`Self::relayout_for`].
    pub fn dom_mut(&mut self) -> &mut D {
        &mut self.dom
    }

    /// Mint a new window: a window-root element under the document, plus its
    /// retained layout session at `(width, height)` under `sheets`.
    pub fn add_window(&mut self, sheets: &[&str], width: f32, height: f32) -> WindowRootId {
        let root = self.dom.create_element(qual("div"));
        self.dom.set_attribute(root, qual("class"), WINDOW_ROOT_CLASS);
        let doc = self.dom.document();
        self.dom.append_child(doc, root);
        let id = WindowRootId(self.next);
        self.next += 1;
        let layout = layout_subtree(&self.dom, root, sheets, width, height);
        self.windows.push(WindowRoot {
            id,
            root,
            layout,
            width,
            height,
            sheets: sheets.iter().map(|s| s.to_string()).collect(),
        });
        id
    }

    /// The window-root element for `id`.
    pub fn window_root(&self, id: WindowRootId) -> Option<D::NodeId> {
        self.find(id).map(|w| w.root)
    }

    /// The retained layout session for `id` (paint / hit-test off this).
    pub fn layout(&self, id: WindowRootId) -> Option<&IncrementalLayout<D::NodeId>> {
        self.find(id).map(|w| &w.layout)
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowRootId> + '_ {
        self.windows.iter().map(|w| w.id)
    }

    /// The laid-out rect of `node` in the window that owns it, or `None` if no
    /// window's session lays it out (the fragment plane is the oracle).
    pub fn rect_of(&self, node: D::NodeId) -> Option<(f32, f32, f32, f32)> {
        for w in &self.windows {
            if w.layout.fragments().rect_of(node).is_some() {
                let view = SubtreeView::new(&self.dom, w.root);
                return w.layout.absolute_rect(&view, node);
            }
        }
        None
    }

    /// Which window's subtree currently contains `node` (walks the DOM to a
    /// window-root). The F3 routing key.
    pub fn window_of(&self, node: D::NodeId) -> Option<WindowRootId> {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(w) = self.windows.iter().find(|w| w.root == n) {
                return Some(w.id);
            }
            cur = self.dom.parent(n);
        }
        None
    }

    /// Resize a window and relayout it.
    pub fn resize(&mut self, id: WindowRootId, width: f32, height: f32) {
        if let Some(i) = self.index(id) {
            self.windows[i].width = width;
            self.windows[i].height = height;
            self.relayout_index(i);
        }
    }

    /// Rebuild one window's layout after mutations to its subtree.
    pub fn relayout(&mut self, id: WindowRootId) {
        if let Some(i) = self.index(id) {
            self.relayout_index(i);
        }
    }

    /// F3: relayout only the window whose subtree contains `node` (a mutation
    /// there needs no other window to recompute). Returns the window, if found.
    pub fn relayout_for(&mut self, node: D::NodeId) -> Option<WindowRootId> {
        let id = self.window_of(node)?;
        self.relayout(id);
        Some(id)
    }

    /// The tear-out primitive: move the `node` subtree under window `to`'s
    /// root, preserving node identity (`move_before`, one document), then
    /// relayout the source and target windows. Returns `false` if `to` is
    /// unknown.
    pub fn move_to_window(&mut self, node: D::NodeId, to: WindowRootId) -> bool {
        let Some(to_root) = self.window_root(to) else {
            return false;
        };
        let from = self.window_of(node);
        self.dom.move_before(to_root, node, None);
        if let Some(from) = from {
            self.relayout(from);
        }
        self.relayout(to);
        true
    }

    fn relayout_index(&mut self, i: usize) {
        let root = self.windows[i].root;
        let (w, h) = (self.windows[i].width, self.windows[i].height);
        let sheets: Vec<&str> = self.windows[i].sheets.iter().map(String::as_str).collect();
        self.windows[i].layout = layout_subtree(&self.dom, root, &sheets, w, h);
    }

    fn find(&self, id: WindowRootId) -> Option<&WindowRoot<D::NodeId>> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn index(&self, id: WindowRootId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }
}

#[cfg(test)]
mod tests {
    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::LayoutDomMut;

    use super::*;

    const SHEET: &str = "div { display: block; width: 100px; height: 20px; }";

    fn tile(dom: &mut ScriptedDom, parent: NodeId, text: &str) -> NodeId {
        let el = dom.create_element(qual("div"));
        let t = dom.create_text(text);
        dom.append_child(el, t);
        dom.append_child(parent, el);
        el
    }

    /// The F0 spike promoted to the manager API: two windows at distinct
    /// viewports lay out independently; a mutation routes to (and relayouts)
    /// only its window; a tear-out move preserves identity across both.
    #[test]
    fn forest_dom_routes_relayout_and_tears_out_a_tile() {
        let mut forest = ForestDom::new(ScriptedDom::new());
        let win_a = forest.add_window(&[SHEET], 400.0, 600.0);
        let win_b = forest.add_window(&[SHEET], 800.0, 300.0);
        assert_eq!(forest.window_count(), 2);

        let root_a = forest.window_root(win_a).unwrap();
        let root_b = forest.window_root(win_b).unwrap();
        let a1 = tile(forest.dom_mut(), root_a, "alpha-1");
        let b1 = tile(forest.dom_mut(), root_b, "beta-1");
        // Content added after add_window needs a relayout of its window.
        forest.relayout(win_a);
        forest.relayout(win_b);

        // Each tile lays out in its own window; routing attributes it correctly.
        assert!(forest.rect_of(a1).is_some());
        assert_eq!(forest.window_of(a1), Some(win_a));
        assert_eq!(forest.window_of(b1), Some(win_b));

        // F3: a mutation in A relayouts only A. Capture B's rect, mutate A,
        // relayout via routing, and B is byte-identical.
        let b1_before = forest.rect_of(b1).unwrap();
        let a2 = tile(forest.dom_mut(), root_a, "alpha-2");
        assert_eq!(forest.relayout_for(a2), Some(win_a), "the mutation routed to A");
        assert_eq!(forest.rect_of(b1).unwrap(), b1_before, "B untouched by A's mutation");

        // Tear-out: move a2 from A to B, identity preserved, both windows see it.
        assert!(forest.move_to_window(a2, win_b));
        assert_eq!(forest.window_of(a2), Some(win_b), "the tile now lives in B");
        assert!(forest.rect_of(a2).is_some(), "and lays out under B's root");
        // A no longer lays it out.
        let view_a = SubtreeView::new(forest.dom(), root_a);
        assert!(
            forest.layout(win_a).unwrap().absolute_rect(&view_a, a2).is_none(),
            "A dropped the torn-out tile"
        );
    }
}
