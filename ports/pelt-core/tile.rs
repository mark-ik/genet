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
    Split { axis: SplitAxis, children: Vec<TileBranch> },
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
    DividerMoved { split: TilePath, fractions: Vec<f32> },
}

/// Where a dragged tile was dropped.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    /// Into a stack (the leaf addressed by `stack`), inserted at `index`.
    Stack { stack: TilePath, index: usize },
    /// Onto an edge of an existing tile, creating a new split that places the dragged
    /// tile on that side of the target.
    Edge { tile: TileId, edge: Edge },
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
        TileTree::Stack(TabStack { tabs: vec![tile], active: 0 })
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
            }
            TileTree::Stack(stack) => out.extend(stack.tabs.iter()),
        }
    }

    /// Find a tile by id anywhere in the tree.
    pub fn find(&self, id: TileId) -> Option<&Tile> {
        self.tiles().into_iter().find(|t| t.id == id)
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
        }
    }

    /// A single-tile tree has one tile, found by id.
    #[test]
    fn single_tile_tree() {
        let tree = TileTree::single(doc_tile(1, "a.html"));
        assert_eq!(tree.tiles().len(), 1);
        assert_eq!(tree.find(TileId(1)).map(|t| t.title.as_str()), Some("a.html"));
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
                TileBranch { fraction: 0.5, tree: left },
                TileBranch { fraction: 0.5, tree: right },
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
            vec![TileBranch::new(0.5, stack(1)), TileBranch::new(0.5, stack(2))],
        );
        // Right half: a quad — two rows, each split into two.
        let row = |a, b| {
            TileTree::split(
                SplitAxis::Row,
                vec![TileBranch::new(0.5, stack(a)), TileBranch::new(0.5, stack(b))],
            )
        };
        let right = TileTree::split(
            SplitAxis::Column,
            vec![TileBranch::new(0.5, row(3, 4)), TileBranch::new(0.5, row(5, 6))],
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
        };
        assert!(matches!(tile.content, ContentSource::Settings(_)));
    }
}
