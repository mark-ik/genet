/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The host-agnostic tile interaction layer: the input state machine over a
//! [`TileSurface`].
//!
//! "Shed the host loop" applied to *input*. The press / move / release / wheel state
//! machine (cursor tracking, divider drags, tab drags, drop resolution) lives here, not
//! welded to winit. The windowed shell ([`crate::tile_viewer`]) feeds it winit events;
//! a headless driver (tests / automation / assistive tech) feeds it synthetic events
//! and reads the resulting [`TileTree`] / frame back. One input brain, many drivers —
//! the same seam the lib-first plan applies to rendering, applied to interaction, so a
//! gesture can be verified without a human at the screen.

use pelt_core::tile::{DropTarget, Edge, TileEvent, TileId, TilePath, TileTree};
use serval_render::ContentReport;
use xilem_serval::PointerClick;

use crate::tile_surface::{GhostLayer, TileFrame, TileSurface};

type Rect = (f32, f32, f32, f32);

/// An in-progress divider drag (see [`TileShell::pointer_down`]).
struct DividerDrag {
    path: TilePath,
    index: usize,
    horizontal: bool,
    extent: f32,
    start: (f32, f32),
    init_first: f32,
    pair_total: f32,
}

/// An in-progress tab drag.
struct TabDrag {
    tile: TileId,
    start: (f32, f32),
    moved: bool,
}

/// A tile surface plus its live interaction state, driven by semantic pointer/wheel
/// events. The shell owns the cursor, so `pointer_down` / `pointer_up` / `wheel` act at
/// the position set by the preceding `pointer_move`.
pub struct TileShell {
    surface: TileSurface,
    width: u32,
    height: u32,
    ui_scale: f32,
    cursor: (f32, f32),
    /// The last frame's tile content rects, for routing a click/scroll/drop to the tile
    /// under the cursor.
    tile_rects: Vec<(TileId, Rect)>,
    divider_drag: Option<DividerDrag>,
    tab_drag: Option<TabDrag>,
    /// When set, a tab-drag drop and a divider resize are *reported* as
    /// [`TileEvent`]s through [`take_events`](Self::take_events) for an embedding host
    /// to apply to its own arrangement, instead of being applied to the surface tree
    /// here. The host re-projects via [`set_tree`](Self::set_tree), so the surface
    /// stays a view, never a second authority. Default `false` keeps the standalone
    /// (apply-locally) behaviour the windowed [`crate::tile_viewer`] relies on.
    host_authoritative: bool,
}

impl TileShell {
    /// A shell over `tree`, at a default size (set the real size with [`resize`]).
    /// Gestures apply to the surface tree locally (standalone pelt).
    pub fn new(tree: TileTree) -> Self {
        Self::with_authority(tree, false)
    }

    /// A shell whose tab-drag and divider gestures are *reported* through
    /// [`take_events`](Self::take_events) for an embedding host to apply (meerkat's
    /// `Workbench`), rather than mutating the surface tree. Pair with
    /// [`set_tree`](Self::set_tree): the host applies each gesture to its arrangement
    /// and re-projects. (Host-authority mode.)
    pub fn new_host_authoritative(tree: TileTree) -> Self {
        Self::with_authority(tree, true)
    }

    fn with_authority(tree: TileTree, host_authoritative: bool) -> Self {
        Self {
            surface: TileSurface::new(tree),
            width: 800,
            height: 600,
            ui_scale: 1.0,
            cursor: (0.0, 0.0),
            tile_rects: Vec::new(),
            divider_drag: None,
            tab_drag: None,
            host_authoritative,
        }
    }

    /// Replace the tile tree — the host re-projects its arrangement here after applying
    /// the gestures from [`take_events`](Self::take_events). Mirrors
    /// [`TileSurface::set_tree`].
    pub fn set_tree(&mut self, tree: TileTree) {
        self.surface.set_tree(tree);
    }

    /// Drain the queued tile gestures: tab activate / close always, plus (in
    /// host-authority mode) the tab-drag [`Dragged`](TileEvent::Dragged) and divider
    /// [`DividerMoved`](TileEvent::DividerMoved). The host maps each onto its
    /// arrangement and re-projects via [`set_tree`](Self::set_tree).
    pub fn take_events(&mut self) -> Vec<TileEvent> {
        self.surface.take_events()
    }

    /// Layer the host's theme CSS over the surface's structural default.
    pub fn set_theme(&mut self, css: impl Into<String>) {
        self.surface.set_theme(css);
    }

    /// Set the shell's UI scale so transient drag visuals match the host's chrome scale.
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(0.5, 4.0);
    }

    /// Set the surface size (the next [`frame`](Self::frame) lays out at it).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }

    /// Render the frame at the current size, caching the tile content rects for input
    /// routing. While a tab drag is moving, the frame carries a ghost of the dragged tab
    /// at the cursor. The host composites the returned [`TileFrame`].
    pub fn frame(&mut self) -> TileFrame {
        let mut frame = self.surface.frame(self.width, self.height);
        // Cache every tile's content rect for input routing (click / scroll / drop).
        // A host-driven tile (meerkat) composites its own texture, so it has no document
        // layer in `frame.tiles` but does report an `external_tiles` rect — include both,
        // or drop resolution (`resolve_drop` -> `tile_at`) never finds an external tile
        // and an Edge (split) drop can't resolve.
        self.tile_rects = frame
            .tiles
            .iter()
            .map(|t| (t.tile, t.rect))
            .chain(
                frame
                    .external_tiles
                    .iter()
                    .map(|(tile, rect, _)| (*tile, *rect)),
            )
            .collect();
        if let Some(drag) = self.tab_drag.as_ref() {
            if drag.moved {
                if let Some(title) = self.surface.tile_title(drag.tile) {
                    let gw = (210.0 * self.ui_scale).round().max(1.0) as u32;
                    let gh = (40.0 * self.ui_scale).round().max(1.0) as u32;
                    let scene = self.surface.ghost_scene(&title, gw, gh);
                    // Offset so the ghost trails just below-right of the cursor hotspot.
                    let rect = (
                        self.cursor.0 - (14.0 * self.ui_scale),
                        self.cursor.1 - (16.0 * self.ui_scale),
                        gw as f32,
                        gh as f32,
                    );
                    frame.ghost = Some(GhostLayer { rect, scene });
                }
            }
        }
        frame
    }

    /// The current tile tree (for the host / a driver to observe).
    pub fn tree(&self) -> &TileTree {
        self.surface.tree()
    }

    /// The underlying surface (the observe surface: its DOM, tree, hit-tests — the
    /// substrate a headless driver / inspector queries).
    pub fn surface(&self) -> &TileSurface {
        &self.surface
    }

    /// A structural [`ContentReport`] of tile `id`'s document ("inspect tile") — the
    /// observe surface a driver, an inspector pane, or a test queries instead of
    /// reading pixels.
    pub fn inspect_tile(&self, id: TileId) -> Option<ContentReport> {
        self.surface.inspect_tile(id)
    }

    /// The current cursor position.
    pub fn cursor(&self) -> (f32, f32) {
        self.cursor
    }

    /// Move the pointer to `(x, y)`. Advances a live divider drag (resizing the split)
    /// and arms a tab drag once the cursor leaves the press point. Returns whether a
    /// redraw is needed.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        self.cursor = (x, y);
        let mut redraw = false;
        let drag = self.divider_drag.as_ref().map(|d| {
            (
                d.path.clone(),
                d.index,
                d.horizontal,
                d.extent,
                d.start,
                d.init_first,
                d.pair_total,
            )
        });
        if let Some((path, index, horizontal, extent, start, init_first, total)) = drag {
            let delta = if horizontal {
                self.cursor.0 - start.0
            } else {
                self.cursor.1 - start.1
            };
            let frac_delta = if extent > 0.0 { delta / extent } else { 0.0 };
            let new_first = (init_first + frac_delta).clamp(0.05 * total, 0.95 * total);
            if let Some(mut fracs) = self.surface.fractions_at(&path) {
                if index + 1 < fracs.len() {
                    fracs[index] = new_first;
                    fracs[index + 1] = total - new_first;
                    if self.host_authoritative {
                        // Report the resize; the host applies it to its arrangement
                        // and re-projects. Do not mutate the surface tree here.
                        self.surface.queue_event(TileEvent::DividerMoved {
                            split: path,
                            fractions: fracs,
                        });
                    } else {
                        self.surface.set_divider_fractions(&path, fracs);
                    }
                    redraw = true;
                }
            }
        }
        if let Some(drag) = self.tab_drag.as_mut() {
            if (self.cursor.0 - drag.start.0).abs() + (self.cursor.1 - drag.start.1).abs() > 6.0 {
                drag.moved = true;
            }
            // Repaint on every move of an armed tab drag so the ghost follows the
            // cursor (the frame adds the ghost from this drag state).
            if drag.moved {
                redraw = true;
            }
        }
        redraw
    }

    /// Press the pointer at the current cursor. Resolves to: starting a divider drag, a
    /// content click (routed to the tile's document), arming a tab drag, or a frame
    /// click (a close ×). Returns whether a redraw is needed.
    pub fn pointer_down(&mut self) -> bool {
        let (x, y) = self.cursor;
        let (w, h) = (self.width, self.height);
        if let Some(hit) = self.surface.divider_at(x, y, w, h) {
            if let Some(fracs) = self.surface.fractions_at(&hit.path) {
                if hit.index + 1 < fracs.len() {
                    self.divider_drag = Some(DividerDrag {
                        path: hit.path,
                        index: hit.index,
                        horizontal: hit.horizontal,
                        extent: hit.extent,
                        start: self.cursor,
                        init_first: fracs[hit.index],
                        pair_total: fracs[hit.index] + fracs[hit.index + 1],
                    });
                }
            }
            return false;
        }
        if let Some((tile, local)) = self.tile_at(self.cursor) {
            return self.surface.click_tile(tile, local.0, local.1);
        }
        if let Some(tile) = self.surface.tab_at(x, y, w, h) {
            self.tab_drag = Some(TabDrag {
                tile,
                start: self.cursor,
                moved: false,
            });
            return false;
        }
        if let Some(node) = self.surface.hit_test_frame(x, y, w, h) {
            self.surface
                .dispatch_click(node, PointerClick::at(self.cursor));
            // The click queued a gesture (e.g. a close ×). Standalone pelt applies it
            // here; a host-authoritative shell leaves it for `take_events` so the host
            // applies it to its arrangement and re-projects.
            if !self.host_authoritative {
                self.surface.pump();
            }
        }
        true
    }

    /// Release the pointer. Ends a divider drag; a moved tab drag drops (splitting the
    /// target pane on its nearest edge, merging into a tab bar, or reporting an outside
    /// drop in host-authority mode), an unmoved one activates the tab. Returns whether
    /// a redraw is needed.
    pub fn pointer_up(&mut self) -> bool {
        self.divider_drag = None;
        if let Some(drag) = self.tab_drag.take() {
            if drag.moved {
                if let Some(to) = self.resolve_drop(drag.tile) {
                    if self.host_authoritative {
                        // Report the drop; the host applies it to its arrangement and
                        // re-projects. Do not mutate the surface tree here.
                        self.surface.queue_event(TileEvent::Dragged {
                            tile: drag.tile,
                            to,
                        });
                    } else {
                        self.surface.drag_tile(drag.tile, to);
                    }
                }
            } else {
                let (w, h) = (self.width, self.height);
                if let Some(node) = self
                    .surface
                    .hit_test_frame(drag.start.0, drag.start.1, w, h)
                {
                    self.surface
                        .dispatch_click(node, PointerClick::at(drag.start));
                    // An unmoved press is a tab activate. Standalone pelt applies it; a
                    // host-authoritative shell leaves it for `take_events`.
                    if !self.host_authoritative {
                        self.surface.pump();
                    }
                }
            }
            return true;
        }
        false
    }

    /// Wheel by `(dx, dy)` over the tile under the cursor (scrolls just that document).
    /// Returns whether it moved. Routes to the nearest `overflow: scroll/auto` container
    /// under the pointer (tile-local), falling through to the tile's document viewport.
    pub fn wheel(&mut self, dx: f32, dy: f32) -> bool {
        if let Some((tile, local)) = self.tile_at(self.cursor) {
            return self.surface.scroll_tile_at(tile, local.0, local.1, dx, dy);
        }
        false
    }

    /// The tile whose content rect contains `p`, with the point in tile-local coords.
    fn tile_at(&self, p: (f32, f32)) -> Option<(TileId, (f32, f32))> {
        self.tile_rects
            .iter()
            .find(|(_, r)| in_rect(p, *r))
            .map(|(id, r)| (*id, (p.0 - r.0, p.1 - r.1)))
    }

    /// Resolve a tab drop at the cursor. Over a tab bar, the dragged tile merges into
    /// that stack (`DropTarget::Stack`); over another tile's content, it splits that
    /// pane on the nearest edge (`DropTarget::Edge`). In host-authority mode, a drop
    /// over no tile resolves to `DropTarget::Outside` so the embedding host can tear
    /// the tile out; standalone pelt ignores that case. `None` over the dragged tile's
    /// own content.
    fn resolve_drop(&self, dragged: TileId) -> Option<DropTarget> {
        let (x, y) = self.cursor;
        let (w, h) = (self.width, self.height);
        if let Some((stack, index)) = self.surface.tabbar_at(x, y, w, h) {
            return Some(DropTarget::Stack { stack, index });
        }
        let Some((tile, rect)) = self
            .tile_rects
            .iter()
            .find(|(_, r)| in_rect(self.cursor, *r))
        else {
            return self.host_authoritative.then_some(DropTarget::Outside);
        };
        if *tile == dragged {
            return None;
        }
        Some(DropTarget::Edge {
            tile: *tile,
            edge: nearest_edge(self.cursor, *rect),
        })
    }
}

fn in_rect(p: (f32, f32), r: Rect) -> bool {
    p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3
}

/// The side of rect `r` nearest to point `p` — the edge a tab dropped there splits on.
fn nearest_edge(p: (f32, f32), r: Rect) -> Edge {
    let rx = if r.2 > 0.0 { (p.0 - r.0) / r.2 } else { 0.5 };
    let ry = if r.3 > 0.0 { (p.1 - r.1) / r.3 } else { 0.5 };
    let (left, right, top, bottom) = (rx, 1.0 - rx, ry, 1.0 - ry);
    let m = left.min(right).min(top).min(bottom);
    if m == left {
        Edge::Left
    } else if m == right {
        Edge::Right
    } else if m == top {
        Edge::Top
    } else {
        Edge::Bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pelt_core::tile::{ContentSource, DocumentRef, SplitAxis, Tile, TileBranch};

    fn doc_tile(id: u64, html: &str) -> Tile {
        Tile {
            id: TileId(id),
            title: format!("tab{id}"),
            content: ContentSource::Document(DocumentRef(format!("data:text/html,{html}"))),
            accent: None,
        }
    }

    /// A driven tab drag, headless: press the left stack's first tab, move to the right
    /// pane near its right edge, release. The full press→move→release flow runs through
    /// the same state machine the windowed shell uses, and the tree splits — no window,
    /// no human. This is the test that would have caught a too-small-threshold bug.
    #[test]
    fn driven_tab_drag_splits_headless() {
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
        let mut shell = TileShell::new(tree);
        shell.resize(800, 600);
        let _ = shell.frame(); // lay out + cache rects

        // Press the first tab in the left tab bar, drag to the right pane near its right
        // edge, release.
        shell.pointer_move(20.0, 14.0);
        shell.pointer_down();
        shell.pointer_move(770.0, 300.0);
        shell.pointer_up();

        // Tile 1 left the stack and split the right pane: left=[2], right=[3 | 1].
        let ids: Vec<u64> = shell.tree().tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(
            ids,
            vec![2, 3, 1],
            "the drag built a new split layout: {ids:?}"
        );
    }

    /// In host-authority mode the same driven tab-drag and divider-drag are *reported*
    /// through `take_events` and the surface tree is left UNCHANGED — the embedding host
    /// (meerkat's Workbench) applies them and re-projects via `set_tree`. This is the
    /// counterpart to `driven_tab_drag_splits_headless`, which applies locally.
    #[test]
    fn host_authoritative_reports_gestures_without_applying() {
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
        let mut shell = TileShell::new_host_authoritative(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        let before: Vec<u64> = shell.tree().tiles().iter().map(|t| t.id.0).collect();

        // Drag tab 1 onto the right pane: a Dragged event, surface tree unchanged.
        shell.pointer_move(20.0, 14.0);
        shell.pointer_down();
        shell.pointer_move(770.0, 300.0);
        shell.pointer_up();
        let after: Vec<u64> = shell.tree().tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(
            before, after,
            "host-authority mode must not mutate the surface tree"
        );
        let events = shell.take_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                TileEvent::Dragged {
                    tile: TileId(1),
                    ..
                }
            )),
            "the drag is reported as a Dragged event: {events:?}"
        );

        // Drag the central divider: a DividerMoved event, fractions unchanged on the
        // surface (the host owns them). The tree is still the original 50/50 row.
        let _ = shell.frame();
        shell.pointer_move(400.0, 300.0);
        shell.pointer_down();
        let moved = shell.pointer_move(420.0, 300.0);
        assert!(moved, "the divider drag asks for a redraw");
        let events = shell.take_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TileEvent::DividerMoved { .. })),
            "the divider drag is reported as DividerMoved: {events:?}"
        );
        shell.pointer_up();
    }

    /// In host-authority mode an unmoved tab click is reported as Activated via
    /// take_events *without* applying it locally — the host (meerkat's Workbench) owns
    /// the active tab and re-projects. (The click path also went through `pump` before;
    /// this guards that it now queues for the host instead.)
    #[test]
    fn host_authoritative_reports_tab_activate_without_applying() {
        let tree = TileTree::stack(vec![doc_tile(1, "<p>1</p>"), doc_tile(2, "<p>2</p>")], 0);
        let mut shell = TileShell::new_host_authoritative(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        let x = (0..400)
            .map(|x| x as f32)
            .find(|&x| shell.surface().tab_at(x, 14.0, 800, 600) == Some(TileId(2)))
            .expect("tab 2 is in the tab bar");
        shell.pointer_move(x, 14.0);
        shell.pointer_down();
        shell.pointer_up();
        if let TileTree::Stack(s) = shell.tree() {
            assert_eq!(
                s.active, 0,
                "host-authority mode does not change the active tab locally"
            );
        } else {
            panic!("expected a stack");
        }
        let events = shell.take_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TileEvent::Activated(TileId(2)))),
            "the unmoved tab click is reported as Activated: {events:?}"
        );
    }

    /// A host-driven tile composites its own texture (meerkat's lane), so it has no
    /// document layer — its content rect lives only in `frame.external_tiles`. The shell
    /// must still find it for drop resolution: dragging a tab onto an external tile's
    /// content resolves an Edge (split) drop. Guards the regression where `tile_rects`
    /// drew only from `frame.tiles` and every meerkat drop silently failed.
    #[test]
    fn host_authority_edge_drop_resolves_external_tile() {
        fn ext_tile(id: u64) -> Tile {
            Tile {
                id: TileId(id),
                title: format!("ext{id}"),
                content: ContentSource::ExternalTexture(pelt_core::tile::TextureKey(id)),
                accent: None,
            }
        }
        let tree = TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::stack(vec![ext_tile(1), ext_tile(2)], 0)),
                TileBranch::new(0.5, TileTree::single(ext_tile(3))),
            ],
        );
        let mut shell = TileShell::new_host_authoritative(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        // Drag tab 1 from the left stack onto the right (external) pane's content.
        shell.pointer_move(20.0, 14.0);
        shell.pointer_down();
        shell.pointer_move(770.0, 300.0);
        shell.pointer_up();
        let events = shell.take_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                TileEvent::Dragged {
                    tile: TileId(1),
                    to: DropTarget::Edge {
                        tile: TileId(3),
                        ..
                    }
                }
            )),
            "dragging onto an external tile resolves an Edge drop: {events:?}"
        );
    }

    /// In host-authority mode, dragging a tab past the workbench surface reports an
    /// outside drop instead of silently vanishing, so the embedding host can tear the
    /// tile into its own window.
    #[test]
    fn host_authority_drag_outside_reports_outside() {
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
        let mut shell = TileShell::new_host_authoritative(tree);
        shell.resize(800, 600);
        let _ = shell.frame();

        shell.pointer_move(20.0, 14.0);
        shell.pointer_down();
        shell.pointer_move(980.0, 300.0);
        shell.pointer_up();

        let events = shell.take_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                TileEvent::Dragged {
                    tile: TileId(1),
                    to: DropTarget::Outside,
                }
            )),
            "dragging past the surface reports Outside: {events:?}"
        );
    }

    /// A tab dropped onto another stack's tab bar merges into that stack instead of
    /// splitting the pane: the same press→move→release flow, but released over the tab
    /// bar (top strip) rather than the content area.
    #[test]
    fn driven_tab_drag_onto_tabbar_merges_headless() {
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
        let mut shell = TileShell::new(tree);
        shell.resize(800, 600);
        let _ = shell.frame();

        // Press tab 1 in the left bar, drag to the RIGHT stack's tab bar (top strip,
        // right half), release.
        shell.pointer_move(20.0, 14.0);
        shell.pointer_down();
        shell.pointer_move(600.0, 14.0);
        shell.pointer_up();

        // Tile 1 merged into the right stack: left collapses to [2], right is [3, 1] —
        // still a 2-way split, no new split from the drop.
        let ids: Vec<u64> = shell.tree().tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(
            ids,
            vec![2, 3, 1],
            "tile 1 merged into the right stack: {ids:?}"
        );
        match shell.tree() {
            TileTree::Split { children, .. } => {
                assert_eq!(children.len(), 2, "still a 2-way split, not a fresh split");
                match &children[1].tree {
                    TileTree::Stack(s) => {
                        assert_eq!(s.tabs.len(), 2, "the right stack now holds two tabs");
                    },
                    _ => panic!("the right child should be a stack"),
                }
            },
            _ => panic!("expected the row split to survive"),
        }
    }

    /// A press that does not move past the threshold activates the tab instead of
    /// dragging — the click/drag discrimination, driven.
    #[test]
    fn driven_unmoved_press_activates() {
        let tree = TileTree::stack(vec![doc_tile(1, "<p>1</p>"), doc_tile(2, "<p>2</p>")], 0);
        let mut shell = TileShell::new(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        // Locate tab 2 in the tab bar via the surface, then press + release without
        // moving (a click, not a drag).
        let x = (0..400)
            .map(|x| x as f32)
            .find(|&x| shell.surface().tab_at(x, 14.0, 800, 600) == Some(TileId(2)))
            .expect("tab 2 is somewhere in the tab bar");
        shell.pointer_move(x, 14.0);
        shell.pointer_down();
        shell.pointer_up();
        if let TileTree::Stack(s) = shell.tree() {
            assert_eq!(s.active, 1, "the unmoved press activated tab 2");
        } else {
            panic!("expected a stack");
        }
    }

    /// A moving tab drag carries a ghost in the frame; at rest and after release there
    /// is none. The ghost is host input state (it tracks the live drag), so it is
    /// verifiable headless without reading pixels.
    #[test]
    fn drag_carries_ghost_then_clears() {
        let tree = TileTree::stack(vec![doc_tile(1, "<p>1</p>"), doc_tile(2, "<p>2</p>")], 0);
        let mut shell = TileShell::new(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        assert!(shell.frame().ghost.is_none(), "no ghost at rest");
        // Press tab 1, then move past the drag threshold: the frame carries a ghost.
        let x = (0..400)
            .map(|x| x as f32)
            .find(|&x| shell.surface().tab_at(x, 14.0, 800, 600) == Some(TileId(1)))
            .expect("tab 1 is in the tab bar");
        shell.pointer_move(x, 14.0);
        shell.pointer_down();
        // The move past the threshold must signal a redraw, or the windowed shell never
        // re-frames and the ghost never paints (the bug this guards).
        assert!(
            shell.pointer_move(x + 60.0, 200.0),
            "a moving tab drag asks for a redraw"
        );
        assert!(
            shell.frame().ghost.is_some(),
            "a moving tab drag shows a ghost"
        );
        // Release ends the drag; the ghost clears.
        shell.pointer_up();
        assert!(
            shell.frame().ghost.is_none(),
            "the ghost clears after release"
        );
    }

    /// Inspecting a tile returns its content's structural report — the observe surface
    /// reaching the addressed document, asserted semantically (title / headings /
    /// links) rather than by pixels.
    #[test]
    fn inspect_tile_reports_content() {
        let tree = TileTree::single(doc_tile(
            1,
            "<title>Demo</title><h1>Head</h1><a href=\"/x\">link</a>",
        ));
        let mut shell = TileShell::new(tree);
        shell.resize(800, 600);
        let _ = shell.frame();
        let report = shell
            .inspect_tile(TileId(1))
            .expect("tile 1 has a document");
        assert_eq!(report.title.as_deref(), Some("Demo"));
        assert_eq!(report.headings, vec!["Head"]);
        assert_eq!(report.links, vec!["/x"]);
    }
}
