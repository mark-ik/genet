/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Spatial index for graph-node hit-testing.
//!
//! Nodes are indexed by their canvas (world) space position so lasso
//! selection can use an efficient R*-tree range query instead of a
//! full O(n) node scan.

use egui::Pos2;
use rstar::{AABB, RTree, RTreeObject};

use crate::graph::NodeKey;

/// A graph node entry stored in the R*-tree.
struct IndexedNode {
    envelope: AABB<[f32; 2]>,
    key: NodeKey,
}

impl RTreeObject for IndexedNode {
    type Envelope = AABB<[f32; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Spatial index mapping canvas-space positions to `NodeKey`s.
///
/// Built from an iterator of `(NodeKey, canvas_pos)` pairs. Queries
/// operate in canvas space; callers are responsible for converting
/// screen-space coordinates via `MetadataFrame::screen_to_canvas_pos`
/// before calling [`nodes_in_canvas_rect`].
pub(crate) struct NodeSpatialIndex {
    tree: RTree<IndexedNode>,
}

impl NodeSpatialIndex {
    /// Build the index from an iterator of `(key, canvas_position, radius)` tuples.
    pub fn build(nodes: impl Iterator<Item = (NodeKey, Pos2, f32)>) -> Self {
        let entries: Vec<_> = nodes
            .map(|(key, pos, radius)| IndexedNode {
                envelope: AABB::from_corners(
                    [pos.x - radius, pos.y - radius],
                    [pos.x + radius, pos.y + radius],
                ),
                key,
            })
            .collect();
        Self {
            tree: RTree::bulk_load(entries),
        }
    }

    /// Return all node keys whose canvas position falls inside `rect`.
    pub fn nodes_in_canvas_rect(&self, rect: egui::Rect) -> Vec<NodeKey> {
        let aabb = AABB::from_corners([rect.min.x, rect.min.y], [rect.max.x, rect.max.y]);
        self.tree
            .locate_in_envelope_intersecting(&aabb)
            .map(|n| n.key)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};
    use std::time::Instant;

    fn make_key(raw: u32) -> NodeKey {
        petgraph::graph::NodeIndex::new(raw as usize)
    }

    #[test]
    fn test_nodes_in_canvas_rect_finds_contained_nodes() {
        let index = NodeSpatialIndex::build(
            [
                (make_key(0), Pos2::new(10.0, 10.0), 8.0),
                (make_key(1), Pos2::new(50.0, 50.0), 8.0),
                (make_key(2), Pos2::new(200.0, 200.0), 8.0),
            ]
            .into_iter(),
        );
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let mut found = index.nodes_in_canvas_rect(rect);
        found.sort_by_key(|k| k.index());
        assert_eq!(found, vec![make_key(0), make_key(1)]);
    }

    #[test]
    fn test_nodes_in_canvas_rect_excludes_outside_nodes() {
        let index =
            NodeSpatialIndex::build([(make_key(0), Pos2::new(500.0, 500.0), 8.0)].into_iter());
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let found = index.nodes_in_canvas_rect(rect);
        assert!(found.is_empty());
    }

    #[test]
    fn test_nodes_in_canvas_rect_includes_radius_overlap() {
        let index =
            NodeSpatialIndex::build([(make_key(0), Pos2::new(110.0, 50.0), 12.0)].into_iter());
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let found = index.nodes_in_canvas_rect(rect);
        assert_eq!(found, vec![make_key(0)]);
    }

    #[test]
    fn test_empty_graph_returns_empty() {
        let index = NodeSpatialIndex::build(std::iter::empty());
        let rect = Rect::from_min_max(Pos2::new(-1000.0, -1000.0), Pos2::new(1000.0, 1000.0));
        assert!(index.nodes_in_canvas_rect(rect).is_empty());
    }

    #[test]
    #[ignore]
    fn perf_nodes_in_canvas_rect_10k_under_budget() {
        let node_count = 10_000u32;
        let radius = 8.0f32;
        let nodes = (0..node_count).map(|i| {
            let x = (i % 100) as f32 * 20.0;
            let y = (i / 100) as f32 * 20.0;
            (make_key(i), Pos2::new(x, y), radius)
        });
        let build_start = Instant::now();
        let index = NodeSpatialIndex::build(nodes);
        let build_elapsed = build_start.elapsed();

        let query_rect = Rect::from_min_max(Pos2::new(400.0, 400.0), Pos2::new(1200.0, 1200.0));
        let query_start = Instant::now();
        let found = index.nodes_in_canvas_rect(query_rect);
        let query_elapsed = query_start.elapsed();

        assert!(!found.is_empty());
        assert!(
            build_elapsed.as_millis() < 100,
            "build took {:?}, expected < 100ms",
            build_elapsed
        );
        assert!(
            query_elapsed.as_millis() < 10,
            "query took {:?}, expected < 10ms",
            query_elapsed
        );
    }
}
