/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The tile-tree contract (V5): the presentation-grade arrangement vocabulary the
//! pelt surface renders and the host drives.
//!
//! A [`TileTree`] is a tree of **splits** (a row/column of children, each with a
//! fractional share) and **tab-stacks** (a set of tiles, one active). The surface lib
//! renders a tree and emits [`TileEvent`]s (activate / close / drag / divider move);
//! the host owns the authoritative tree, applies the events, and feeds back the next
//! tree. Standalone pelt populates it from its own simple state; mere projects forme
//! onto it through platen's `tree_projection` — a *projection*, not a second authority.
//!
//! **Presentation vocabulary only.** This contract names splits, tabs, fractions, and
//! the two content lanes — nothing about graphs, sessions, lineage, or arrangement
//! relations. If it ever starts wanting those, it is drifting toward forme (the
//! arrangement truth on the mere side) and should stop. forme maps *onto* this; this
//! never grows toward forme.

/// A host-assigned identity for a tile, stable across the renders of one running
/// surface. Opaque to the contract (the host mints + interprets it).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TileId(pub u64);

/// How a split lays its children out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitAxis {
    /// Children side by side, divided by vertical dividers (a row).
    Row,
    /// Children stacked top to bottom, divided by horizontal dividers (a column).
    Column,
}

/// The arrangement the surface renders: either a split of children, or a leaf
/// tab-stack. The host holds the authoritative value and rebuilds it as events apply.
#[derive(Clone, Debug, PartialEq)]
pub enum TileTree {
    /// A row/column of child subtrees, each taking a fraction of the axis.
    Split {
        axis: SplitAxis,
        children: Vec<TileBranch>,
    },
    /// A leaf: a stack of tabbed tiles, one shown.
    Stack(TabStack),
}

/// A child of a split: a subtree plus its fractional share of the split axis. The
/// shares across a split's children are maintained by the host (conventionally summing
/// to 1.0); the surface renders them and a [`TileEvent::DividerMoved`] updates them.
#[derive(Clone, Debug, PartialEq)]
pub struct TileBranch {
    pub fraction: f32,
    pub tree: TileTree,
}

/// A leaf stack of tabbed tiles. `active` indexes the shown tab (the host keeps it in
/// range as tabs are added / closed).
#[derive(Clone, Debug, PartialEq)]
pub struct TabStack {
    pub tabs: Vec<Tile>,
    pub active: usize,
}

/// One tile: a titled handle onto a content source. The tile is the *tab* (the handle);
/// its content is rendered into the tile's body by the host-resolved lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: TileId,
    pub title: String,
    pub content: ContentSource,
    /// An optional tab tint the host owns. `None` (the default) leaves the tab to the
    /// surface's tab styling; `Some` colors *this* tab so it can carry a host meaning of
    /// its own — mere tints each tab to match its graph node's state + selection, so a
    /// tab reads as its node. The surface renders it inline (overriding the tab CSS).
    pub accent: Option<TabAccent>,
}

/// A host-owned tab tint: opaque sRGB `background` + `foreground` (label) bytes the
/// surface paints inline on a tab, overriding the theme's tab colors. The host decides
/// the meaning (mere: the node's activation/selection color); the contract just carries
/// the two colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabAccent {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
}

/// Where a tile's content comes from — the two lanes the surface composites. The
/// contract names the lane only; the host resolves the actual content. This is the
/// `serval content-root subtree` vs `external_texture(key)` distinction V6 routes on.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentSource {
    /// A serval document. Standalone pelt carries the document URL here; mere carries
    /// an (opaque) handle to a content-root subtree it renders into the tile.
    Document(DocumentRef),
    /// An externally-composited texture, addressed by key (V6: constellation actor
    /// textures, scrying WebViews). Standalone pelt produces none — the lane is named
    /// here so the contract is complete before mere needs it.
    ExternalTexture(TextureKey),
    /// A settings page, rendered by the host's settings-lane provider. The lane is
    /// **multi-provider**: the [`SettingsRef`] namespaces which provider + page (pelt's
    /// own settings; a moot's permissioned settings pages; any namespace that
    /// implements the settings-lane protocol), and each provider resolves its own refs
    /// to a permission-gated page. The contract names the lane and carries the opaque
    /// ref; the settings protocol itself (page schema, permission model) is the
    /// provider's concern, not the tile contract's.
    Settings(SettingsRef),
}

/// An opaque reference to a document the host resolves. Standalone pelt stores the
/// document URL; the contract does not interpret it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DocumentRef(pub String);

/// An opaque key for an externally-composited texture the host supplies.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextureKey(pub u64);

/// An opaque, namespaced reference to a settings page (e.g. `"pelt/appearance"`,
/// `"moot:<id>/permissions"`) that the settings-lane provider for that namespace
/// resolves to a rendered, permission-gated page.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SettingsRef(pub String);

/// A path from the tree root to a split node: the child index taken at each split on
/// the way down. The empty path is the root. Used to address the split a divider belongs
/// to (and the destination stack of a drag).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TilePath(pub Vec<usize>);

/// A gesture the surface emits for the host to apply to its authoritative tree. The
/// surface never mutates the tree itself — it reports intent; the host is the single
/// writer (so standalone pelt and mere apply the same events to different state).
#[derive(Clone, Debug, PartialEq)]
pub enum TileEvent {
    /// A tab was activated (selected) within its stack.
    Activated(TileId),
    /// A tab's close affordance was used.
    Closed(TileId),
    /// A tab was dragged onto a drop target (another stack, or a tile's edge to split).
    Dragged { tile: TileId, to: DropTarget },
    /// A split divider moved: the new fractional shares for the split addressed by
    /// `split` (same length + order as that split's children).
    DividerMoved {
        split: TilePath,
        fractions: Vec<f32>,
    },
}

/// Where a dragged tile was dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Into a stack (the leaf addressed by `stack`), inserted at `index`.
    Stack { stack: TilePath, index: usize },
    /// Onto an edge of an existing tile, creating a new split that places the dragged
    /// tile on that side of the target.
    Edge { tile: TileId, edge: Edge },
    /// Outside the tile surface entirely. Standalone pelt leaves the tree unchanged;
    /// an embedding host may interpret it as "tear this tile out".
    Outside,
}

/// Which edge of a tile a drag landed on (the side the dropped tile takes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl TileBranch {
    /// A split child taking `fraction` of the axis.
    pub fn new(fraction: f32, tree: TileTree) -> Self {
        Self { fraction, tree }
    }
}

impl TileTree {
    /// A tree of a single tile (a one-tab stack) — the surface's initial state.
    pub fn single(tile: Tile) -> Self {
        TileTree::Stack(TabStack {
            tabs: vec![tile],
            active: 0,
        })
    }

    /// A leaf stack of `tabs` with `active` shown.
    pub fn stack(tabs: Vec<Tile>, active: usize) -> Self {
        TileTree::Stack(TabStack { tabs, active })
    }

    /// A split of `children` along `axis`.
    pub fn split(axis: SplitAxis, children: Vec<TileBranch>) -> Self {
        TileTree::Split { axis, children }
    }

    /// Visit every tile in the tree in document order (splits left-to-right /
    /// top-to-bottom, then each stack's tabs in order).
    pub fn tiles(&self) -> Vec<&Tile> {
        let mut out = Vec::new();
        self.collect_tiles(&mut out);
        out
    }

    fn collect_tiles<'a>(&'a self, out: &mut Vec<&'a Tile>) {
        match self {
            TileTree::Split { children, .. } => {
                for branch in children {
                    branch.tree.collect_tiles(out);
                }
            },
            TileTree::Stack(stack) => out.extend(stack.tabs.iter()),
        }
    }

    /// Find a tile by id anywhere in the tree.
    pub fn find(&self, id: TileId) -> Option<&Tile> {
        self.tiles().into_iter().find(|t| t.id == id)
    }

    /// Find a tile by id anywhere in the tree, mutably — for the host to retarget a
    /// tile's content + title when a followed link navigates it (not an event: the
    /// content source changing is a host state edit, outside the gesture reducer).
    pub fn tile_mut(&mut self, id: TileId) -> Option<&mut Tile> {
        match self {
            TileTree::Stack(stack) => stack.tabs.iter_mut().find(|t| t.id == id),
            TileTree::Split { children, .. } => {
                children.iter_mut().find_map(|b| b.tree.tile_mut(id))
            },
        }
    }

    /// The child fractions of the split addressed by `path`, or `None` if the path
    /// does not resolve to a split. (For a host driving divider resize.)
    pub fn fractions_at(&self, path: &TilePath) -> Option<Vec<f32>> {
        match self.node_at(path)? {
            TileTree::Split { children, .. } => Some(children.iter().map(|b| b.fraction).collect()),
            TileTree::Stack(_) => None,
        }
    }

    /// The axis of the split addressed by `path`, or `None` if it is not a split.
    pub fn axis_at(&self, path: &TilePath) -> Option<SplitAxis> {
        match self.node_at(path)? {
            TileTree::Split { axis, .. } => Some(*axis),
            TileTree::Stack(_) => None,
        }
    }

    /// Apply a [`TileEvent`] to the tree, returning whether it changed. This is the
    /// **reference reducer standalone pelt uses** — the tree is pelt's whole arrangement
    /// state, so it applies events here. mere does *not* use it: mere applies the same
    /// events to forme (its authority) and re-projects, so the contract stays a
    /// projection target, not a second writer. The tree is kept canonical: a tab-stack
    /// emptied by a close/drag is removed from its split, and a split left with one
    /// child flattens into that child.
    pub fn apply(&mut self, event: &TileEvent) -> bool {
        match event {
            TileEvent::Activated(id) => self.activate(*id),
            TileEvent::Closed(id) => {
                let removed = self.remove_tile(*id).is_some();
                if removed {
                    self.collapse();
                }
                removed
            },
            TileEvent::DividerMoved { split, fractions } => self.set_fractions(split, fractions),
            TileEvent::Dragged { tile, to } => self.drag(*tile, to),
        }
    }

    /// Set the active tab in whichever stack holds `id`.
    fn activate(&mut self, id: TileId) -> bool {
        match self {
            TileTree::Stack(stack) => match stack.tabs.iter().position(|t| t.id == id) {
                Some(i) if stack.active != i => {
                    stack.active = i;
                    true
                },
                _ => false,
            },
            TileTree::Split { children, .. } => children.iter_mut().any(|b| b.tree.activate(id)),
        }
    }

    /// Remove the tile `id` from its stack (keeping the stack's `active` in range),
    /// returning it. Does *not* collapse — callers collapse once after the structural
    /// change, so paths/ids resolved beforehand stay valid across the removal.
    fn remove_tile(&mut self, id: TileId) -> Option<Tile> {
        match self {
            TileTree::Stack(stack) => {
                let i = stack.tabs.iter().position(|t| t.id == id)?;
                let tile = stack.tabs.remove(i);
                if stack.active >= stack.tabs.len() {
                    stack.active = stack.tabs.len().saturating_sub(1);
                }
                Some(tile)
            },
            TileTree::Split { children, .. } => {
                children.iter_mut().find_map(|b| b.tree.remove_tile(id))
            },
        }
    }

    /// Canonicalize: drop empty tab-stacks from splits, renormalize the surviving
    /// fractions, and flatten a split that is left with a single child into that child.
    fn collapse(&mut self) {
        if let TileTree::Split { children, .. } = self {
            for branch in children.iter_mut() {
                branch.tree.collapse();
            }
            children.retain(|b| !b.tree.is_empty_stack());
            normalize(children);
            if children.len() == 1 {
                *self = children.remove(0).tree;
            }
        }
    }

    fn is_empty_stack(&self) -> bool {
        matches!(self, TileTree::Stack(s) if s.tabs.is_empty())
    }

    /// Navigate to the node at `path` (child index at each split), immutably.
    fn node_at(&self, path: &TilePath) -> Option<&TileTree> {
        let mut node = self;
        for &idx in &path.0 {
            match node {
                TileTree::Split { children, .. } => node = &children.get(idx)?.tree,
                TileTree::Stack(_) => return None,
            }
        }
        Some(node)
    }

    /// Navigate to the node at `path`, mutably.
    fn node_at_mut(&mut self, path: &TilePath) -> Option<&mut TileTree> {
        let mut node = self;
        for &idx in &path.0 {
            match node {
                TileTree::Split { children, .. } => node = &mut children.get_mut(idx)?.tree,
                TileTree::Stack(_) => return None,
            }
        }
        Some(node)
    }

    /// Set the fractional shares of the split addressed by `path` (length must match).
    fn set_fractions(&mut self, path: &TilePath, fractions: &[f32]) -> bool {
        match self.node_at_mut(path) {
            Some(TileTree::Split { children, .. }) if children.len() == fractions.len() => {
                let mut changed = false;
                for (branch, f) in children.iter_mut().zip(fractions) {
                    if branch.fraction != *f {
                        branch.fraction = *f;
                        changed = true;
                    }
                }
                changed
            },
            _ => false,
        }
    }

    /// Move `id` onto `to`. The target is validated first so a failed drag never loses
    /// the tile; the tile is then removed (structure unchanged — only a tab leaves its
    /// stack) and inserted, and the tree collapses once at the end.
    fn drag(&mut self, id: TileId, to: &DropTarget) -> bool {
        let target_ok = match to {
            DropTarget::Stack { stack, .. } => {
                matches!(self.node_at(stack), Some(TileTree::Stack(_)))
            },
            DropTarget::Edge { tile, .. } => self.find(*tile).is_some(),
            DropTarget::Outside => false,
        };
        if !target_ok {
            return false;
        }
        let Some(tile) = self.remove_tile(id) else {
            return false;
        };
        match to {
            DropTarget::Stack { stack, index } => {
                if let Some(TileTree::Stack(s)) = self.node_at_mut(stack) {
                    let i = (*index).min(s.tabs.len());
                    s.tabs.insert(i, tile);
                    s.active = i;
                }
            },
            DropTarget::Edge { tile: target, edge } => {
                self.split_at_tile(*target, *edge, tile);
            },
            DropTarget::Outside => return false,
        }
        self.collapse();
        true
    }

    /// Wrap the stack holding `target` in a new split, placing `tile` on `edge`'s side.
    fn split_at_tile(&mut self, target: TileId, edge: Edge, tile: Tile) -> bool {
        match self {
            TileTree::Stack(stack) if stack.tabs.iter().any(|t| t.id == target) => {
                let axis = match edge {
                    Edge::Left | Edge::Right => SplitAxis::Row,
                    Edge::Top | Edge::Bottom => SplitAxis::Column,
                };
                let placeholder = TileTree::Stack(TabStack {
                    tabs: Vec::new(),
                    active: 0,
                });
                let target_tree = std::mem::replace(self, placeholder);
                let new_tree = TileTree::single(tile);
                let (first, second) = match edge {
                    Edge::Left | Edge::Top => (new_tree, target_tree),
                    Edge::Right | Edge::Bottom => (target_tree, new_tree),
                };
                *self = TileTree::split(
                    axis,
                    vec![TileBranch::new(0.5, first), TileBranch::new(0.5, second)],
                );
                true
            },
            TileTree::Stack(_) => false,
            TileTree::Split { children, .. } => children
                .iter_mut()
                .any(|b| b.tree.split_at_tile(target, edge, tile.clone())),
        }
    }
}

/// Renormalize a split's child fractions to sum to 1.0 (after a child is removed). If
/// the shares are degenerate (sum ~0), fall back to equal shares.
fn normalize(children: &mut [TileBranch]) {
    let sum: f32 = children.iter().map(|b| b.fraction).sum();
    if sum > f32::EPSILON {
        for branch in children.iter_mut() {
            branch.fraction /= sum;
        }
    } else if !children.is_empty() {
        let equal = 1.0 / children.len() as f32;
        for branch in children.iter_mut() {
            branch.fraction = equal;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_tile(id: u64, url: &str) -> Tile {
        Tile {
            id: TileId(id),
            title: url.to_string(),
            content: ContentSource::Document(DocumentRef(url.to_string())),
            accent: None,
        }
    }

    /// A single-tile tree has one tile, found by id.
    #[test]
    fn single_tile_tree() {
        let tree = TileTree::single(doc_tile(1, "a.html"));
        assert_eq!(tree.tiles().len(), 1);
        assert_eq!(
            tree.find(TileId(1)).map(|t| t.title.as_str()),
            Some("a.html")
        );
        assert!(tree.find(TileId(2)).is_none());
    }

    /// A split of two stacks visits every tile in order.
    #[test]
    fn split_visits_all_tiles_in_order() {
        let left = TileTree::Stack(TabStack {
            tabs: vec![doc_tile(1, "a.html"), doc_tile(2, "b.html")],
            active: 0,
        });
        let right = TileTree::single(doc_tile(3, "c.html"));
        let tree = TileTree::Split {
            axis: SplitAxis::Row,
            children: vec![
                TileBranch {
                    fraction: 0.5,
                    tree: left,
                },
                TileBranch {
                    fraction: 0.5,
                    tree: right,
                },
            ],
        };
        let ids: Vec<u64> = tree.tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    /// Recursive, asymmetric, mixed-axis splitting is expressible: a Row split whose
    /// left child is a 2-tile horizontal split and whose right child is a quad (a
    /// Column of two Row-splits) — the exact arrangement the V5 contract must support,
    /// to arbitrary depth and fraction.
    #[test]
    fn recursive_asymmetric_quad_and_split() {
        let stack = |id| TileTree::single(doc_tile(id, "x.html"));
        // Left half: two tiles split top/bottom.
        let left = TileTree::split(
            SplitAxis::Column,
            vec![
                TileBranch::new(0.5, stack(1)),
                TileBranch::new(0.5, stack(2)),
            ],
        );
        // Right half: a quad — two rows, each split into two.
        let row = |a, b| {
            TileTree::split(
                SplitAxis::Row,
                vec![
                    TileBranch::new(0.5, stack(a)),
                    TileBranch::new(0.5, stack(b)),
                ],
            )
        };
        let right = TileTree::split(
            SplitAxis::Column,
            vec![
                TileBranch::new(0.5, row(3, 4)),
                TileBranch::new(0.5, row(5, 6)),
            ],
        );
        let tree = TileTree::split(
            SplitAxis::Row,
            vec![TileBranch::new(0.5, left), TileBranch::new(0.5, right)],
        );
        // Six tile-stacks, arbitrarily nested, visited in order.
        let ids: Vec<u64> = tree.tiles().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
    }

    /// N-ary splits express even shares directly: three columns at 1/3 each.
    #[test]
    fn n_ary_even_split() {
        let third = |id| TileBranch::new(1.0 / 3.0, TileTree::single(doc_tile(id, "x.html")));
        let tree = TileTree::split(SplitAxis::Column, vec![third(1), third(2), third(3)]);
        assert_eq!(tree.tiles().len(), 3);
    }

    /// The settings lane carries a namespaced page ref a provider resolves.
    #[test]
    fn settings_lane() {
        let tile = Tile {
            id: TileId(9),
            title: "Settings".into(),
            content: ContentSource::Settings(SettingsRef("pelt/appearance".into())),
            accent: None,
        };
        assert!(matches!(tile.content, ContentSource::Settings(_)));
    }

    /// A two-stack row used by several reducer tests.
    fn row_of(a: u64, b: u64) -> TileTree {
        TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::single(doc_tile(a, "a"))),
                TileBranch::new(0.5, TileTree::single(doc_tile(b, "b"))),
            ],
        )
    }

    /// Activating a tab selects it within its stack.
    #[test]
    fn apply_activate() {
        let mut tree = TileTree::stack(vec![doc_tile(1, "a"), doc_tile(2, "b")], 0);
        assert!(tree.apply(&TileEvent::Activated(TileId(2))));
        if let TileTree::Stack(s) = &tree {
            assert_eq!(s.active, 1);
        } else {
            panic!("stack");
        }
        // Re-activating the active tab is a no-op.
        assert!(!tree.apply(&TileEvent::Activated(TileId(2))));
    }

    /// Closing the last tile of one side of a split collapses the split into the
    /// surviving side (canonicalization).
    #[test]
    fn apply_close_collapses_split() {
        let mut tree = row_of(1, 2);
        assert!(tree.apply(&TileEvent::Closed(TileId(1))));
        // The split flattened to the remaining single stack holding tile 2.
        assert!(matches!(&tree, TileTree::Stack(_)));
        assert_eq!(
            tree.tiles().iter().map(|t| t.id.0).collect::<Vec<_>>(),
            vec![2]
        );
    }

    /// A divider move rewrites the addressed split's fractions.
    #[test]
    fn apply_divider_move() {
        let mut tree = row_of(1, 2);
        assert!(tree.apply(&TileEvent::DividerMoved {
            split: TilePath(vec![]),
            fractions: vec![0.7, 0.3],
        }));
        if let TileTree::Split { children, .. } = &tree {
            assert!((children[0].fraction - 0.7).abs() < 1e-6);
            assert!((children[1].fraction - 0.3).abs() < 1e-6);
        } else {
            panic!("split");
        }
    }

    /// Dragging a tile into another stack moves it there and collapses the emptied
    /// source side.
    #[test]
    fn apply_drag_into_stack() {
        // Left stack has tiles 1 and 2; right stack has tile 3. Drag 1 into the right.
        let mut tree = TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(
                    0.5,
                    TileTree::stack(vec![doc_tile(1, "a"), doc_tile(2, "b")], 0),
                ),
                TileBranch::new(0.5, TileTree::single(doc_tile(3, "c"))),
            ],
        );
        assert!(tree.apply(&TileEvent::Dragged {
            tile: TileId(1),
            to: DropTarget::Stack {
                stack: TilePath(vec![1]),
                index: 1
            },
        }));
        // Order preserved: left now [2], right now [3, 1].
        assert_eq!(
            tree.tiles().iter().map(|t| t.id.0).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    /// Dragging a tile onto a tile's edge creates a new split with the dragged tile on
    /// that side.
    #[test]
    fn apply_drag_onto_edge_splits() {
        let mut tree = TileTree::single(doc_tile(1, "a"));
        // Add a second tile to the same stack so removing one leaves a target.
        if let TileTree::Stack(s) = &mut tree {
            s.tabs.push(doc_tile(2, "b"));
        }
        // Drag tile 2 onto the right edge of tile 1: Row split [stack(1), stack(2)].
        assert!(tree.apply(&TileEvent::Dragged {
            tile: TileId(2),
            to: DropTarget::Edge {
                tile: TileId(1),
                edge: Edge::Right
            },
        }));
        match &tree {
            TileTree::Split { axis, children } => {
                assert_eq!(*axis, SplitAxis::Row);
                assert_eq!(children.len(), 2);
                // Right edge → target (1) first, dragged (2) second.
                assert_eq!(children[0].tree.tiles()[0].id.0, 1);
                assert_eq!(children[1].tree.tiles()[0].id.0, 2);
            },
            _ => panic!("expected a split"),
        }
    }

    /// A drag onto a vanished target is a no-op that does not lose the tile.
    #[test]
    fn apply_drag_bad_target_preserves_tile() {
        let mut tree = TileTree::stack(vec![doc_tile(1, "a"), doc_tile(2, "b")], 0);
        assert!(!tree.apply(&TileEvent::Dragged {
            tile: TileId(1),
            to: DropTarget::Edge {
                tile: TileId(99),
                edge: Edge::Top
            },
        }));
        // Both tiles still present.
        assert_eq!(tree.tiles().len(), 2);
    }

    /// An outside drop is for the embedding host to interpret; the reference reducer
    /// leaves the tree alone.
    #[test]
    fn apply_drag_outside_preserves_tile() {
        let mut tree = TileTree::stack(vec![doc_tile(1, "a"), doc_tile(2, "b")], 0);
        let before = tree.clone();
        assert!(!tree.apply(&TileEvent::Dragged {
            tile: TileId(1),
            to: DropTarget::Outside,
        }));
        assert_eq!(tree, before);
    }
}
