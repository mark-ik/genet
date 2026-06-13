/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The tile-tree surface (V5): splits + tab-stacks of live documents.
//!
//! V2's two-root compositing, scaled from "one strip + one document" to "one tile
//! frame + N documents." The [`pelt_core::tile::TileTree`] is mapped to xilem-serval
//! flex DOM (the *frame*: splits become flex rows/columns, tab-stacks become a tab bar
//! over a content-area placeholder), and each active tile's [`LoadedDocument`] is
//! composited into its content-area's laid-out rect (`fragments().rect_of`). That
//! placeholder is the external-texture-element idea in miniature: a hole the host
//! fills (a document scene here; an actor texture in V6).
//!
//! Tab clicks queue [`TileEvent`]s the surface applies through the reducer
//! ([`TileTree::apply`]). This module is the GPU-free foundation (the flex view, the
//! surface, the frame scene + content rects, tab activation); the windowed compositing
//! of the frame + the N document layers is the integration step on top.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use layout_dom_api::{LayoutDom, LocalName, Namespace};
use netrender::Scene;
use pelt_core::tile::{
    ContentSource, DocumentRef, DropTarget, SplitAxis, TabStack, TileEvent, TileId, TilePath,
    TileTree,
};
use serval_layout::IncrementalLayout;
use serval_render::{scene_from_session_dom, ContentReport};
use serval_scripted_dom::{NodeId, ScriptedDom};
use xilem_serval::{
    el, on_click, AnyView, DomHandle, PointerClick, ServalAppRunner, ServalCtx, ServalElement,
};

use crate::document::{LoadedDocument, LocalFetcher};

/// The erased tile-frame view type.
pub type TileView = Box<dyn AnyView<TileState, (), ServalCtx, ServalElement>>;
type TileLogic = fn(&TileState) -> TileView;

/// The surface's app state: the authoritative tile tree and the queue of tile events
/// the view handlers raise (tab clicks), drained by [`TileSurface::pump`].
pub struct TileState {
    tree: TileTree,
    pending: Vec<TileEvent>,
}

/// Map the tile tree to flex DOM. Splits become `display:flex` row/column with each
/// child sized by `flex-grow: fraction`; tab-stacks become a tab bar over a
/// content-area placeholder marked with the active tile's id.
fn tile_view(state: &TileState) -> TileView {
    render_node(&state.tree, &[])
}

/// Encode a split path (`[0, 1]`) as a DOM-attr string (`"0.1"`); the empty path (the
/// root split) is `""`.
fn encode_path(path: &[usize]) -> String {
    path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(".")
}

/// Decode the `data-divider` attr back to a [`TilePath`].
fn decode_path(s: &str) -> TilePath {
    if s.is_empty() {
        TilePath(Vec::new())
    } else {
        TilePath(s.split('.').filter_map(|p| p.parse().ok()).collect())
    }
}

fn render_node(node: &TileTree, path: &[usize]) -> TileView {
    match node {
        TileTree::Split { axis, children } => {
            let dir = match axis {
                SplitAxis::Row => "row",
                SplitAxis::Column => "column",
            };
            let path_attr = encode_path(path);
            // Interleave a draggable divider between adjacent children. Each divider
            // carries its split's path + the boundary index, so the host can resolve a
            // drag to a `DividerMoved`.
            let mut items: Vec<TileView> = Vec::new();
            for (j, branch) in children.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(j);
                let inner = render_node(&branch.tree, &child_path);
                let style = format!(
                    "flex: {frac} {frac} 0; min-width: 0; min-height: 0;",
                    frac = branch.fraction
                );
                items.push(Box::new(
                    el::<_, TileState, ()>("div", inner)
                        .attr("class", "tile-branch")
                        .attr("style", style),
                ) as TileView);
                if j + 1 < children.len() {
                    items.push(Box::new(
                        el::<_, TileState, ()>("div", ())
                            .attr("class", "tile-divider")
                            .attr("data-divider", path_attr.clone())
                            .attr("data-dindex", j.to_string()),
                    ) as TileView);
                }
            }
            Box::new(
                el::<_, TileState, ()>("div", items)
                    .attr("class", "tile-split")
                    .attr("style", format!("display: flex; flex-direction: {dir};")),
            )
        }
        TileTree::Stack(stack) => render_stack(stack, path),
    }
}

fn render_stack(stack: &TabStack, path: &[usize]) -> TileView {
    // The tab bar: a clickable tab per tile, the active one highlighted. Each tab's
    // handler queues an `Activated` for its own id (a per-tab capturing closure).
    let tabs: Vec<TileView> = stack
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tile)| {
            let id = tile.id;
            let class = if i == stack.active { "tile-tab active" } else { "tile-tab" };
            // The label activates the tab; the close × removes it. The × stops
            // propagation so its click does not also reach the tab's activate handler.
            let label = el::<_, TileState, ()>("span", tile.title.clone()).attr("class", "tile-label");
            let close = on_click(
                el::<_, TileState, ()>("span", "\u{00d7}").attr("class", "tile-close"),
                move |s: &mut TileState, ev: PointerClick| {
                    ev.stop_propagation();
                    s.pending.push(TileEvent::Closed(id));
                },
            );
            Box::new(on_click(
                el::<_, TileState, ()>("div", (label, close))
                    .attr("class", class)
                    .attr("data-tabid", id.0.to_string()),
                move |s: &mut TileState, _: PointerClick| s.pending.push(TileEvent::Activated(id)),
            )) as TileView
        })
        .collect();
    // The tab bar carries its stack's path so a tab dropped here resolves to a
    // `DropTarget::Stack` (insert into this stack) rather than an edge split.
    let tab_bar = el::<_, TileState, ()>("div", tabs)
        .attr("class", "tile-tabbar")
        .attr("data-stack", encode_path(path));

    // The content-area placeholder for the active tile, marked with its id so the host
    // can find its laid-out rect and composite the tile's document there.
    let active_id = stack.tabs.get(stack.active).map(|t| t.id.0).unwrap_or(0);
    let content = el::<_, TileState, ()>("div", ())
        .attr("class", "tile-content")
        .attr("data-tile", active_id.to_string());

    Box::new(
        el::<_, TileState, ()>("div", (tab_bar, content))
            .attr("class", "tile-stack")
            .attr("style", "display: flex; flex-direction: column;"),
    )
}

/// The default tile-frame stylesheet (the structural + tab-bar styling; a theme layers
/// over it, like the chrome's).
const DEFAULT_TILE_CSS: &str = "\
    div { display: block; box-sizing: border-box; } \
    head, style, script, title, meta, link, base { display: none; } \
    .tile-split { width: 100%; height: 100%; } \
    .tile-branch { display: flex; } \
    .tile-stack { width: 100%; height: 100%; } \
    .tile-tabbar { display: flex; height: 28px; background: #33333a; } \
    .tile-tab { display: flex; align-items: center; padding: 5px 10px; color: #cccccc; background: #2a2a30; margin-right: 2px; } \
    .tile-tab.active { color: #ffffff; background: #4a4a55; } \
    .tile-close { margin-left: 8px; padding: 0 4px; color: #999999; } \
    .tile-content { flex: 1 1 0; min-height: 0; background: #ffffff; } \
    .tile-divider { flex: 0 0 8px; background: #1a1a1f; }";

/// A rendered tile-tree frame: the frame scene (tab bars + dividers) plus one layer
/// per active tile (its content-area rect + its document's scene) for the host to
/// composite over the frame.
pub struct TileFrame {
    pub frame_scene: Scene,
    pub tiles: Vec<TileLayer>,
}

/// One tile's content layer: which tile, where to composite (`(x, y, w, h)` in surface
/// px), and the document scene to composite there.
pub struct TileLayer {
    pub tile: TileId,
    pub rect: (f32, f32, f32, f32),
    pub scene: Scene,
}

/// A tile-tree surface: a [`ServalAppRunner`] over the frame view + the authoritative
/// tree, plus a live [`LoadedDocument`] per document-lane tile.
pub struct TileSurface {
    runner: ServalAppRunner<TileState, TileLogic, TileView, ()>,
    docs: HashMap<TileId, LoadedDocument>,
    sheets: Vec<String>,
}

impl TileSurface {
    /// Build the surface for `tree`, loading a document for each document-lane tile.
    pub fn new(tree: TileTree) -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner =
            ServalAppRunner::new(dom, tile_view as TileLogic, TileState { tree, pending: Vec::new() });
        let mut surface = Self {
            runner,
            docs: HashMap::new(),
            sheets: vec![DEFAULT_TILE_CSS.to_string()],
        };
        surface.load_docs();
        surface
    }

    /// Ensure a [`LoadedDocument`] exists for every document-lane tile currently in the
    /// tree (and drop docs for tiles that are gone). Lazily loads new tiles.
    fn load_docs(&mut self) {
        let mut wanted: Vec<(TileId, String)> = Vec::new();
        for tile in self.runner.state().tree.tiles() {
            if let ContentSource::Document(DocumentRef(url)) = &tile.content {
                wanted.push((tile.id, url.clone()));
            }
        }
        let live: std::collections::HashSet<TileId> = wanted.iter().map(|(id, _)| *id).collect();
        self.docs.retain(|id, _| live.contains(id));
        for (id, url) in wanted {
            if !self.docs.contains_key(&id) {
                if let Ok(doc) = LoadedDocument::load(&LocalFetcher, &url) {
                    self.docs.insert(id, doc);
                }
            }
        }
    }

    /// Drain the queued tile events (tab clicks) and apply each through the reducer,
    /// then reconcile the live document set. Returns whether the tree changed.
    pub fn pump(&mut self) -> bool {
        let mut events = Vec::new();
        self.runner.update(|s| events = std::mem::take(&mut s.pending));
        if events.is_empty() {
            return false;
        }
        let mut changed = false;
        for event in &events {
            self.runner.update(|s| {
                if s.tree.apply(event) {
                    changed = true;
                }
            });
        }
        self.load_docs();
        changed
    }

    /// Render the frame at `width`×`height`: the frame scene plus a content layer per
    /// active tile (its rect + its document's scene). The host composites the frame,
    /// then each tile layer over its rect.
    pub fn frame(&mut self, width: u32, height: u32) -> TileFrame {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        // Lay the frame out once for both the scene and the content-area rects.
        let (frame_scene, areas) = {
            let dom = self.runner.dom();
            let dom = dom.borrow();
            let session =
                IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
            let scene = scene_from_session_dom(&session, &*dom, width.max(1), height.max(1));
            let areas = content_area_rects(&dom, &session);
            (scene, areas)
        };
        // Render each active tile's document into its content rect.
        let mut tiles = Vec::new();
        for (tile_id, rect) in areas {
            if let Some(doc) = self.docs.get_mut(&tile_id) {
                let (w, h) = (rect.2.max(1.0) as u32, rect.3.max(1.0) as u32);
                let scene = doc.frame(w, h);
                tiles.push(TileLayer { tile: tile_id, rect, scene });
            }
        }
        TileFrame { frame_scene, tiles }
    }

    /// The shared frame DOM handle (for the host's hit-testing of tab bars / dividers).
    pub fn dom(&self) -> DomHandle {
        self.runner.dom()
    }

    /// Dispatch a click that hit frame node `target` (a tab) — routes it to the tab's
    /// handler (queuing a tile event).
    pub fn dispatch_click(&mut self, target: NodeId, event: PointerClick) {
        self.runner.dispatch_click(target, event);
    }

    /// Hit-test the frame DOM at `(x, y)` (laid out at the surface size), so the host
    /// can resolve a click on a tab / divider to a frame node.
    pub fn hit_test_frame(&self, x: f32, y: f32, width: u32, height: u32) -> Option<NodeId> {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session =
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
        session.hit_test(&*dom, x, y, &serval_layout::ScrollOffsets::default())
    }

    /// Scroll the document in tile `id` by a device-px wheel delta; returns whether it
    /// moved (a no-op for a tile with no document, or at a scroll edge).
    pub fn scroll_tile(&mut self, id: TileId, dx: f32, dy: f32) -> bool {
        self.docs.get_mut(&id).is_some_and(|doc| doc.scroll_by(dx, dy))
    }

    /// Handle a click at tile-local `(x, y)` in tile `id`'s document (in-page link
    /// navigation); returns whether the document scrolled.
    pub fn click_tile(&mut self, id: TileId, x: f32, y: f32) -> bool {
        self.docs.get_mut(&id).is_some_and(|doc| doc.click_at(x, y))
    }

    /// If `(x, y)` is on a divider, the split it resizes: its path, the boundary index,
    /// whether the split is horizontal (a Row), and the split's pixel extent along its
    /// axis (so the host can convert a drag delta to a fraction).
    pub fn divider_at(&self, x: f32, y: f32, width: u32, height: u32) -> Option<DividerHit> {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session =
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
        let mut node = session.hit_test(&*dom, x, y, &serval_layout::ScrollOffsets::default())?;
        let ns = Namespace::default();
        loop {
            if let Some(path_str) = dom.attribute(node, &ns, &LocalName::from("data-divider")) {
                let index: usize = dom
                    .attribute(node, &ns, &LocalName::from("data-dindex"))?
                    .parse()
                    .ok()?;
                let path = decode_path(path_str);
                // The divider's parent is the split container; its extent sets the
                // pixels-per-fraction for the drag.
                let split_node = dom.parent(node)?;
                let split_rect = absolute_rect(&dom, &session, split_node)?;
                let horizontal =
                    matches!(self.runner.state().tree.axis_at(&path), Some(SplitAxis::Row));
                let extent = if horizontal { split_rect.2 } else { split_rect.3 };
                return Some(DividerHit { path, index, horizontal, extent });
            }
            node = dom.parent(node)?;
        }
    }

    /// The tile id of the tab at `(x, y)`, if a tab is there and the press is not on its
    /// close × (which the host dispatches as a click instead). Lets the host start a
    /// tab drag from the press.
    pub fn tab_at(&self, x: f32, y: f32, width: u32, height: u32) -> Option<TileId> {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session =
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
        let mut node = session.hit_test(&*dom, x, y, &serval_layout::ScrollOffsets::default())?;
        let ns = Namespace::default();
        let class = LocalName::from("class");
        // A press on the close × is a close, not a drag.
        if dom
            .attribute(node, &ns, &class)
            .is_some_and(|c| c.split_whitespace().any(|w| w == "tile-close"))
        {
            return None;
        }
        let tabid = LocalName::from("data-tabid");
        loop {
            if let Some(id) = dom.attribute(node, &ns, &tabid).and_then(|s| s.parse::<u64>().ok()) {
                return Some(TileId(id));
            }
            node = dom.parent(node)?;
        }
    }

    /// If `(x, y)` is over a stack's tab bar, that stack's path plus the tab index a
    /// drop would insert at (counting the tabs whose horizontal centre sits left of the
    /// cursor). Lets the host resolve a tab drop onto a tab bar to a `DropTarget::Stack`
    /// — merging the dragged tile into that stack rather than splitting a pane.
    pub fn tabbar_at(&self, x: f32, y: f32, width: u32, height: u32) -> Option<(TilePath, usize)> {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        let session =
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
        let mut node = session.hit_test(&*dom, x, y, &serval_layout::ScrollOffsets::default())?;
        let ns = Namespace::default();
        let stack_attr = LocalName::from("data-stack");
        // Walk up to the tab bar carrying its stack path.
        let bar = loop {
            if dom.attribute(node, &ns, &stack_attr).is_some() {
                break node;
            }
            node = dom.parent(node)?;
        };
        let path = decode_path(dom.attribute(bar, &ns, &stack_attr)?);
        // Insertion index: how many tab centres are left of the cursor.
        let tabid = LocalName::from("data-tabid");
        let mut centres = Vec::new();
        let mut stack = vec![bar];
        while let Some(n) = stack.pop() {
            if dom.attribute(n, &ns, &tabid).is_some() {
                if let Some(r) = absolute_rect(&dom, &session, n) {
                    centres.push(r.0 + r.2 / 2.0);
                }
            }
            for child in dom.dom_children(n) {
                stack.push(child);
            }
        }
        let index = centres.iter().filter(|&&c| c < x).count();
        Some((path, index))
    }

    /// Move `tile` onto `to` (a tab drag), applied through the reducer, then reconcile
    /// the live document set.
    pub fn drag_tile(&mut self, tile: TileId, to: DropTarget) {
        let event = TileEvent::Dragged { tile, to };
        self.runner.update(|s| {
            s.tree.apply(&event);
        });
        self.load_docs();
    }

    /// The child fractions of the split at `path` (for the host's divider drag).
    pub fn fractions_at(&self, path: &TilePath) -> Option<Vec<f32>> {
        self.runner.state().tree.fractions_at(path)
    }

    /// Set the child fractions of the split at `path` (a divider drag), applied through
    /// the reducer.
    pub fn set_divider_fractions(&mut self, path: &TilePath, fractions: Vec<f32>) {
        let event = TileEvent::DividerMoved { split: path.clone(), fractions };
        self.runner.update(|s| {
            s.tree.apply(&event);
        });
    }

    /// The current tile tree (read-only).
    pub fn tree(&self) -> &TileTree {
        &self.runner.state().tree
    }

    /// A structural [`ContentReport`] of tile `id`'s document (the inspector's read
    /// model — "inspect tile"). `None` for a tile with no document.
    pub fn inspect_tile(&self, id: TileId) -> Option<ContentReport> {
        self.docs.get(&id).map(|doc| doc.inspect())
    }
}

/// A divider the host can drag to resize a split (the result of [`TileSurface::divider_at`]).
pub struct DividerHit {
    pub path: TilePath,
    /// The boundary index: the divider sits between split children `index` and `index+1`.
    pub index: usize,
    /// Whether the split is a Row (horizontal drag) vs a Column (vertical drag).
    pub horizontal: bool,
    /// The split's pixel extent along its axis (drag delta / extent = fraction delta).
    pub extent: f32,
}

/// Walk the frame DOM for content-area placeholders (carrying `data-tile=<id>`) and
/// pair each tile id with its laid-out rect.
fn content_area_rects(
    dom: &ScriptedDom,
    session: &IncrementalLayout<NodeId>,
) -> Vec<(TileId, (f32, f32, f32, f32))> {
    let attr = LocalName::from("data-tile");
    let ns = Namespace::default();
    let mut out = Vec::new();
    let mut stack = vec![dom.document()];
    while let Some(node) = stack.pop() {
        if let Some(id) = dom.attribute(node, &ns, &attr).and_then(|s| s.parse::<u64>().ok()) {
            if let Some(rect) = absolute_rect(dom, session, node) {
                out.push((TileId(id), rect));
            }
        }
        for child in dom.dom_children(node) {
            stack.push(child);
        }
    }
    out
}

/// A node's rect in surface coordinates: `rect_of` is relative to the containing block
/// (a flex child reads x=0 within its stack), so we sum the locations up the ancestor
/// chain to get the absolute position. Size comes from the node's own fragment.
fn absolute_rect(
    dom: &ScriptedDom,
    session: &IncrementalLayout<NodeId>,
    node: NodeId,
) -> Option<(f32, f32, f32, f32)> {
    let layout = session.fragments().rect_of(node)?;
    let (w, h) = (layout.size.width, layout.size.height);
    let (mut x, mut y) = (layout.location.x, layout.location.y);
    let mut current = dom.parent(node);
    while let Some(parent) = current {
        if let Some(parent_layout) = session.fragments().rect_of(parent) {
            x += parent_layout.location.x;
            y += parent_layout.location.y;
        }
        current = dom.parent(parent);
    }
    Some((x, y, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pelt_core::tile::{Edge, Tile, TileBranch};

    fn doc_tile(id: u64, html: &str) -> Tile {
        Tile {
            id: TileId(id),
            title: format!("tab{id}"),
            content: ContentSource::Document(DocumentRef(format!("data:text/html,{html}"))),
        }
    }

    /// A single-tile surface renders a frame scene and one content layer with a
    /// non-degenerate rect and the document's scene.
    #[test]
    fn single_tile_renders_frame_and_content() {
        let mut surface = TileSurface::new(TileTree::single(doc_tile(1, "<h1>Hello</h1>")));
        let frame = surface.frame(800, 600);
        assert!(!frame.frame_scene.ops.is_empty(), "the frame paints (the tab bar)");
        assert_eq!(frame.tiles.len(), 1, "one active tile composited");
        let layer = &frame.tiles[0];
        assert!(layer.rect.2 > 1.0 && layer.rect.3 > 1.0, "content rect is non-degenerate: {:?}", layer.rect);
        assert!(
            layer.scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the tile's document paints text",
        );
    }

    /// A row split lays out two tiles side by side: two content layers, the second to
    /// the right of the first.
    #[test]
    fn row_split_lays_out_two_tiles() {
        let tree = TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::single(doc_tile(1, "<p>left</p>"))),
                TileBranch::new(0.5, TileTree::single(doc_tile(2, "<p>right</p>"))),
            ],
        );
        let mut surface = TileSurface::new(tree);
        let frame = surface.frame(800, 600);
        assert_eq!(frame.tiles.len(), 2, "two active tiles");
        let xs: Vec<f32> = frame.tiles.iter().map(|t| t.rect.0).collect();
        assert!(xs.iter().any(|&x| x < 10.0) && xs.iter().any(|&x| x > 100.0), "tiles are side by side: {xs:?}");
    }

    /// Clicking a tab queues an Activated event, and `pump` applies it through the
    /// reducer — the active tab switches.
    #[test]
    fn tab_click_activates() {
        let stack = TileTree::stack(
            vec![doc_tile(1, "<p>one</p>"), doc_tile(2, "<p>two</p>")],
            0,
        );
        let mut surface = TileSurface::new(stack);
        // First frame builds the DOM; find the second tab and click it.
        let _ = surface.frame(800, 600);
        let tab = find_tab(&surface, "tab2").expect("tab2 exists");
        surface.dispatch_click(tab, PointerClick::at((0.0, 0.0)));
        assert!(surface.pump(), "the tab click changed the tree");
        if let TileTree::Stack(s) = surface.tree() {
            assert_eq!(s.active, 1, "tab2 is now active");
        } else {
            panic!("expected a stack");
        }
    }

    /// A divider sits at the boundary between split children; resizing it rewrites the
    /// fractions.
    #[test]
    fn divider_resizes_split() {
        let tree = TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::single(doc_tile(1, "<p>l</p>"))),
                TileBranch::new(0.5, TileTree::single(doc_tile(2, "<p>r</p>"))),
            ],
        );
        let mut surface = TileSurface::new(tree);
        let _ = surface.frame(800, 600);
        // The divider sits at the center of an 800px row split.
        let hit = surface
            .divider_at(400.0, 300.0, 800, 600)
            .expect("a divider at the center boundary");
        assert_eq!(hit.index, 0);
        assert!(hit.horizontal, "a Row split drags horizontally");
        assert!(hit.extent > 700.0, "extent is ~ the window width: {}", hit.extent);
        surface.set_divider_fractions(&hit.path, vec![0.7, 0.3]);
        let fracs = surface.fractions_at(&hit.path).expect("split fractions");
        assert!(
            (fracs[0] - 0.7).abs() < 1e-5 && (fracs[1] - 0.3).abs() < 1e-5,
            "fractions updated: {fracs:?}",
        );
    }

    /// A tab can be located by point, and dragging it onto another tile's edge splits
    /// there: the tile leaves its stack and lands beside the drop target.
    #[test]
    fn tab_drag_onto_edge_splits() {
        let tree = TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(
                    0.5,
                    TileTree::stack(vec![doc_tile(1, "<p>1</p>"), doc_tile(2, "<p>2</p>")], 0),
                ),
                TileBranch::new(0.5, TileTree::single(doc_tile(3, "<p>3</p>"))),
            ],
        );
        let mut surface = TileSurface::new(tree);
        let _ = surface.frame(800, 600);
        // A tab sits in the left stack's tab bar.
        assert!(surface.tab_at(20.0, 14.0, 800, 600).is_some(), "a tab is hit at the tab bar");
        // Drag tile 1 onto tile 3's right edge.
        surface.drag_tile(TileId(1), DropTarget::Edge { tile: TileId(3), edge: Edge::Right });
        // Left stack lost tile 1 ([2]); the right became a split [3 | 1].
        let ids: Vec<u64> = surface.tree().tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![2, 3, 1], "tile 1 moved beside tile 3: {ids:?}");
    }

    /// Closing a tab via its × removes it from the stack and reconciles the docs.
    #[test]
    fn close_button_removes_tab() {
        let stack = TileTree::stack(
            vec![doc_tile(1, "<p>one</p>"), doc_tile(2, "<p>two</p>")],
            0,
        );
        let mut surface = TileSurface::new(stack);
        let _ = surface.frame(800, 600);
        // Click the × inside the tab labelled "tab2".
        let close = find_close(&surface, "tab2").expect("tab2 close button");
        surface.dispatch_click(close, PointerClick::at((0.0, 0.0)));
        assert!(surface.pump(), "closing changed the tree");
        // The stack collapsed to a single remaining tile (tab1).
        let ids: Vec<u64> = surface.tree().tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![1], "tab2 was removed: {ids:?}");
    }

    /// The full descendant text of `node`, in document order.
    fn node_text(dom: &ScriptedDom, node: NodeId) -> String {
        let mut out = String::new();
        collect_text(dom, node, &mut out);
        out
    }

    fn collect_text(dom: &ScriptedDom, node: NodeId, out: &mut String) {
        if let Some(t) = dom.text(node) {
            out.push_str(t);
        }
        for child in dom.dom_children(node) {
            collect_text(dom, child, out);
        }
    }

    fn has_class(dom: &ScriptedDom, node: NodeId, class: &str) -> bool {
        dom.attribute(node, &Namespace::default(), &LocalName::from("class"))
            .is_some_and(|c| c.split_whitespace().any(|w| w == class))
    }

    /// Find a tab `<div class="tile-tab">` whose label text contains `label`.
    fn find_tab(surface: &TileSurface, label: &str) -> Option<NodeId> {
        let dom = surface.dom();
        let dom = dom.borrow();
        let mut stack = vec![dom.document()];
        while let Some(node) = stack.pop() {
            if has_class(&dom, node, "tile-tab") && node_text(&dom, node).contains(label) {
                return Some(node);
            }
            for child in dom.dom_children(node) {
                stack.push(child);
            }
        }
        None
    }

    /// Find the `.tile-close` span inside the tab labelled `label`.
    fn find_close(surface: &TileSurface, label: &str) -> Option<NodeId> {
        let tab = find_tab(surface, label)?;
        let dom = surface.dom();
        let dom = dom.borrow();
        let mut stack = vec![tab];
        while let Some(node) = stack.pop() {
            if has_class(&dom, node, "tile-close") {
                return Some(node);
            }
            for child in dom.dom_children(node) {
                stack.push(child);
            }
        }
        None
    }
}
