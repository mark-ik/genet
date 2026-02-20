/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Graph data structures for the spatial browser.
//!
//! Core structures:
//! - `Graph`: Main graph container backed by petgraph::StableGraph
//! - `Node`: Webpage node with position, velocity, and metadata
//! - `EdgeType`: Connection type between nodes (hyperlink, history, user-grouped)

use euclid::default::{Point2D, Vector2D};
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use petgraph::{Directed, Direction};
use std::collections::HashMap;
use uuid::Uuid;

use crate::persistence::types::{
    GraphSnapshot, PersistedEdge, PersistedEdgeType, PersistedNode, PersistedNodeSessionState,
};

pub mod egui_adapter;

/// Stable node handle (petgraph NodeIndex — survives other deletions)
pub type NodeKey = NodeIndex;

/// Stable edge handle (petgraph EdgeIndex)
pub type EdgeKey = EdgeIndex;

/// A webpage node in the graph
#[derive(Debug, Clone)]
pub struct Node {
    /// Stable node identity.
    pub id: Uuid,

    /// Full URL of the webpage
    pub url: String,

    /// Page title (or URL if no title)
    pub title: String,

    /// Position in graph space
    pub position: Point2D<f32>,

    /// Velocity for physics simulation
    pub velocity: Vector2D<f32>,

    /// Whether this node's position is pinned (doesn't move with physics)
    pub is_pinned: bool,

    /// Timestamp of last visit
    pub last_visited: std::time::SystemTime,

    /// Navigation history seen for this node's mapped webview.
    pub history_entries: Vec<String>,

    /// Current index in `history_entries`.
    pub history_index: usize,

    /// Optional thumbnail bytes (PNG), persisted in snapshots.
    pub thumbnail_png: Option<Vec<u8>>,

    /// Thumbnail width in pixels (valid when `thumbnail_png` is `Some`).
    pub thumbnail_width: u32,

    /// Thumbnail height in pixels (valid when `thumbnail_png` is `Some`).
    pub thumbnail_height: u32,

    /// Optional favicon pixel data (RGBA8), persisted in snapshots.
    pub favicon_rgba: Option<Vec<u8>>,

    /// Favicon width in pixels (valid when `favicon_rgba` is `Some`).
    pub favicon_width: u32,

    /// Favicon height in pixels (valid when `favicon_rgba` is `Some`).
    pub favicon_height: u32,

    /// Last known scroll offset for higher-fidelity cold restore.
    pub session_scroll: Option<(f32, f32)>,

    /// Optional best-effort form draft payload (feature-guarded by caller policy).
    pub session_form_draft: Option<String>,

    /// Webview lifecycle state
    pub lifecycle: NodeLifecycle,
}

/// Lifecycle state for webview management
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLifecycle {
    /// Active webview (visible, rendering)
    Active,

    /// Warm webview (kept alive in memory but not currently visible in a pane)
    Warm,

    /// Cold (metadata only, no process)
    Cold,
}

/// Type of edge connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeType {
    /// Hyperlink from one page to another
    Hyperlink,

    /// Browser history traversal
    History,

    /// Explicit user grouping association
    UserGrouped,
}

/// Read-only view of an edge (built from petgraph edge references)
#[derive(Debug, Clone, Copy)]
pub struct EdgeView {
    pub from: NodeKey,
    pub to: NodeKey,
    pub edge_type: EdgeType,
}

/// Main graph structure backed by petgraph::StableGraph
#[derive(Clone)]
pub struct Graph {
    /// The underlying petgraph stable graph
    pub(crate) inner: StableGraph<Node, EdgeType, Directed>,

    /// URL to node mapping for lookup (supports duplicate URLs).
    url_to_nodes: HashMap<String, Vec<NodeKey>>,

    /// Stable UUID to node mapping.
    id_to_node: HashMap<Uuid, NodeKey>,
}

impl Graph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            inner: StableGraph::new(),
            url_to_nodes: HashMap::new(),
            id_to_node: HashMap::new(),
        }
    }

    /// Add a new node to the graph
    pub fn add_node(&mut self, url: String, position: Point2D<f32>) -> NodeKey {
        self.add_node_with_id(Uuid::new_v4(), url, position)
    }

    /// Add a node with a pre-existing UUID.
    pub fn add_node_with_id(&mut self, id: Uuid, url: String, position: Point2D<f32>) -> NodeKey {
        let now = std::time::SystemTime::now();
        let key = self.inner.add_node(Node {
            id,
            title: url.clone(),
            url: url.clone(),
            position,
            velocity: Vector2D::zero(),
            is_pinned: false,
            last_visited: now,
            history_entries: Vec::new(),
            history_index: 0,
            thumbnail_png: None,
            thumbnail_width: 0,
            thumbnail_height: 0,
            favicon_rgba: None,
            favicon_width: 0,
            favicon_height: 0,
            session_scroll: None,
            session_form_draft: None,
            lifecycle: NodeLifecycle::Cold,
        });

        self.url_to_nodes.entry(url).or_default().push(key);
        self.id_to_node.insert(id, key);
        key
    }

    /// Remove a node and all its connected edges
    pub fn remove_node(&mut self, key: NodeKey) -> bool {
        if let Some(node) = self.inner.remove_node(key) {
            self.id_to_node.remove(&node.id);
            self.remove_url_mapping(&node.url, key);
            true
        } else {
            false
        }
    }

    /// Update a node's URL, maintaining the url_to_node index.
    /// Returns the old URL, or None if the node doesn't exist.
    pub fn update_node_url(&mut self, key: NodeKey, new_url: String) -> Option<String> {
        let node = self.inner.node_weight_mut(key)?;
        let old_url = std::mem::replace(&mut node.url, new_url.clone());
        self.remove_url_mapping(&old_url, key);
        self.url_to_nodes.entry(new_url).or_default().push(key);
        Some(old_url)
    }

    /// Add an edge between two nodes
    pub fn add_edge(&mut self, from: NodeKey, to: NodeKey, edge_type: EdgeType) -> Option<EdgeKey> {
        if !self.inner.contains_node(from) || !self.inner.contains_node(to) {
            return None;
        }
        Some(self.inner.add_edge(from, to, edge_type))
    }

    /// Remove all directed edges from `from` to `to` with the given type.
    /// Returns how many edges were removed.
    pub fn remove_edges(&mut self, from: NodeKey, to: NodeKey, edge_type: EdgeType) -> usize {
        let edge_ids: Vec<EdgeKey> = self
            .inner
            .edge_references()
            .filter(|edge| {
                edge.source() == from && edge.target() == to && *edge.weight() == edge_type
            })
            .map(|edge| edge.id())
            .collect();

        let mut removed = 0;
        for edge_id in edge_ids {
            if self.inner.remove_edge(edge_id).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// Get a node by key
    pub fn get_node(&self, key: NodeKey) -> Option<&Node> {
        self.inner.node_weight(key)
    }

    /// Get a mutable node by key
    pub fn get_node_mut(&mut self, key: NodeKey) -> Option<&mut Node> {
        self.inner.node_weight_mut(key)
    }

    /// Get a node and its key by URL
    pub fn get_node_by_url(&self, url: &str) -> Option<(NodeKey, &Node)> {
        let key = self.url_to_nodes.get(url)?.last().copied()?;
        Some((key, self.inner.node_weight(key)?))
    }

    /// Get all node keys currently mapped to a URL.
    pub fn get_nodes_by_url(&self, url: &str) -> Vec<NodeKey> {
        self.url_to_nodes.get(url).cloned().unwrap_or_default()
    }

    /// Get a node by UUID.
    pub fn get_node_by_id(&self, id: Uuid) -> Option<(NodeKey, &Node)> {
        let key = *self.id_to_node.get(&id)?;
        Some((key, self.inner.node_weight(key)?))
    }

    /// Get node key by UUID.
    pub fn get_node_key_by_id(&self, id: Uuid) -> Option<NodeKey> {
        self.id_to_node.get(&id).copied()
    }

    /// Iterate over all nodes as (key, node) pairs
    pub fn nodes(&self) -> impl Iterator<Item = (NodeKey, &Node)> {
        self.inner
            .node_indices()
            .map(move |idx| (idx, &self.inner[idx]))
    }

    /// Iterate over all edges as EdgeView
    pub fn edges(&self) -> impl Iterator<Item = EdgeView> + '_ {
        self.inner.edge_references().map(|e| EdgeView {
            from: e.source(),
            to: e.target(),
            edge_type: *e.weight(),
        })
    }

    /// Iterate outgoing neighbor keys for a node
    pub fn out_neighbors(&self, key: NodeKey) -> impl Iterator<Item = NodeKey> + '_ {
        self.inner.neighbors_directed(key, Direction::Outgoing)
    }

    /// Iterate incoming neighbor keys for a node
    pub fn in_neighbors(&self, key: NodeKey) -> impl Iterator<Item = NodeKey> + '_ {
        self.inner.neighbors_directed(key, Direction::Incoming)
    }

    /// Check if a directed edge exists from `from` to `to`
    pub fn has_edge_between(&self, from: NodeKey, to: NodeKey) -> bool {
        self.inner.find_edge(from, to).is_some()
    }

    /// Count of nodes in the graph
    pub fn node_count(&self) -> usize {
        self.inner.node_count()
    }

    /// Count of edges in the graph
    pub fn edge_count(&self) -> usize {
        self.inner.edge_count()
    }

    /// Serialize the graph to a persistable snapshot
    pub fn to_snapshot(&self) -> GraphSnapshot {
        let nodes = self
            .nodes()
            .map(|(_, node)| PersistedNode {
                node_id: node.id.to_string(),
                url: node.url.clone(),
                title: node.title.clone(),
                position_x: node.position.x,
                position_y: node.position.y,
                is_pinned: node.is_pinned,
                history_entries: node.history_entries.clone(),
                history_index: node.history_index,
                thumbnail_png: node.thumbnail_png.clone(),
                thumbnail_width: node.thumbnail_width,
                thumbnail_height: node.thumbnail_height,
                favicon_rgba: node.favicon_rgba.clone(),
                favicon_width: node.favicon_width,
                favicon_height: node.favicon_height,
                session_state: Some(PersistedNodeSessionState {
                    history_entries: node.history_entries.clone(),
                    history_index: node.history_index,
                    scroll_x: node.session_scroll.map(|(x, _)| x),
                    scroll_y: node.session_scroll.map(|(_, y)| y),
                    form_draft: node.session_form_draft.clone(),
                }),
            })
            .collect();

        let edges = self
            .edges()
            .map(|edge| {
                let from_node_id = self
                    .get_node(edge.from)
                    .map(|n| n.id.to_string())
                    .unwrap_or_default();
                let to_node_id = self
                    .get_node(edge.to)
                    .map(|n| n.id.to_string())
                    .unwrap_or_default();
                PersistedEdge {
                    from_node_id,
                    to_node_id,
                    edge_type: match edge.edge_type {
                        EdgeType::Hyperlink => PersistedEdgeType::Hyperlink,
                        EdgeType::History => PersistedEdgeType::History,
                        EdgeType::UserGrouped => PersistedEdgeType::UserGrouped,
                    },
                }
            })
            .collect();

        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        GraphSnapshot {
            nodes,
            edges,
            timestamp_secs,
        }
    }

    /// Rebuild a graph from a persisted snapshot
    pub fn from_snapshot(snapshot: &GraphSnapshot) -> Self {
        let mut graph = Graph::new();

        for pnode in &snapshot.nodes {
            let Ok(node_id) = Uuid::parse_str(&pnode.node_id) else {
                continue;
            };
            let key = graph.add_node_with_id(
                node_id,
                pnode.url.clone(),
                Point2D::new(pnode.position_x, pnode.position_y),
            );
            let mut restore_url_from_session: Option<String> = None;
            if let Some(node) = graph.get_node_mut(key) {
                node.title = pnode.title.clone();
                node.is_pinned = pnode.is_pinned;
                node.history_entries = pnode.history_entries.clone();
                node.history_index = pnode
                    .history_index
                    .min(node.history_entries.len().saturating_sub(1));
                node.thumbnail_png = pnode.thumbnail_png.clone();
                node.thumbnail_width = pnode.thumbnail_width;
                node.thumbnail_height = pnode.thumbnail_height;
                node.favicon_rgba = pnode.favicon_rgba.clone();
                node.favicon_width = pnode.favicon_width;
                node.favicon_height = pnode.favicon_height;
                if let Some(session) = &pnode.session_state {
                    node.history_entries = session.history_entries.clone();
                    node.history_index = session
                        .history_index
                        .min(node.history_entries.len().saturating_sub(1));
                    restore_url_from_session =
                        node.history_entries.get(node.history_index).cloned();
                    node.session_scroll = session.scroll_x.zip(session.scroll_y);
                    node.session_form_draft = session.form_draft.clone();
                }
            }
            if let Some(current_url) = restore_url_from_session
                && !current_url.is_empty()
            {
                let _ = graph.update_node_url(key, current_url);
            }
        }

        for pedge in &snapshot.edges {
            let from_key = Uuid::parse_str(&pedge.from_node_id)
                .ok()
                .and_then(|id| graph.get_node_key_by_id(id));
            let to_key = Uuid::parse_str(&pedge.to_node_id)
                .ok()
                .and_then(|id| graph.get_node_key_by_id(id));
            if let (Some(from), Some(to)) = (from_key, to_key) {
                let edge_type = match pedge.edge_type {
                    PersistedEdgeType::Hyperlink => EdgeType::Hyperlink,
                    PersistedEdgeType::History => EdgeType::History,
                    PersistedEdgeType::UserGrouped => EdgeType::UserGrouped,
                };
                graph.add_edge(from, to, edge_type);
            }
        }

        graph
    }

    fn remove_url_mapping(&mut self, url: &str, key: NodeKey) {
        if let Some(keys) = self.url_to_nodes.get_mut(url) {
            keys.retain(|candidate| *candidate != key);
            if keys.is_empty() {
                self.url_to_nodes.remove(url);
            }
        }
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_new() {
        let graph = Graph::new();
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut graph = Graph::new();
        let pos = Point2D::new(100.0, 200.0);
        let key = graph.add_node("https://example.com".to_string(), pos);

        let node = graph.get_node(key).unwrap();
        assert_eq!(node.url, "https://example.com");
        assert_eq!(node.title, "https://example.com");
        assert_eq!(node.position.x, 100.0);
        assert_eq!(node.position.y, 200.0);
        assert_eq!(node.velocity.x, 0.0);
        assert_eq!(node.velocity.y, 0.0);
        assert!(!node.is_pinned);
        assert_eq!(node.lifecycle, NodeLifecycle::Cold);
    }

    #[test]
    fn test_add_multiple_nodes() {
        let mut graph = Graph::new();
        let key1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let key2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        let key3 = graph.add_node("https://c.com".to_string(), Point2D::new(2.0, 2.0));

        assert_eq!(graph.node_count(), 3);
        assert!(graph.get_node(key1).is_some());
        assert!(graph.get_node(key2).is_some());
        assert!(graph.get_node(key3).is_some());
    }

    #[test]
    fn test_duplicate_url_nodes_have_distinct_ids() {
        let mut graph = Graph::new();
        let key1 = graph.add_node("https://same.com".to_string(), Point2D::new(0.0, 0.0));
        let key2 = graph.add_node("https://same.com".to_string(), Point2D::new(10.0, 10.0));

        assert_ne!(key1, key2);
        let node1 = graph.get_node(key1).unwrap();
        let node2 = graph.get_node(key2).unwrap();
        assert_ne!(node1.id, node2.id);
        assert_eq!(graph.get_nodes_by_url("https://same.com").len(), 2);
    }

    #[test]
    fn test_get_node_by_url() {
        let mut graph = Graph::new();
        graph.add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));

        let (_, node) = graph.get_node_by_url("https://example.com").unwrap();
        assert_eq!(node.url, "https://example.com");

        assert!(graph.get_node_by_url("https://notfound.com").is_none());
    }

    #[test]
    fn test_get_node_mut() {
        let mut graph = Graph::new();
        let key = graph.add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));

        {
            let node = graph.get_node_mut(key).unwrap();
            node.position = Point2D::new(100.0, 200.0);
            node.is_pinned = true;
        }

        let node = graph.get_node(key).unwrap();
        assert_eq!(node.position.x, 100.0);
        assert_eq!(node.position.y, 200.0);
        assert!(node.is_pinned);
    }

    #[test]
    fn test_add_edge() {
        let mut graph = Graph::new();
        let node1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let node2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));

        graph.add_edge(node1, node2, EdgeType::Hyperlink).unwrap();

        // Check adjacency via graph methods
        assert!(graph.has_edge_between(node1, node2));
        assert!(!graph.has_edge_between(node2, node1));
        assert_eq!(graph.out_neighbors(node1).count(), 1);
        assert_eq!(graph.in_neighbors(node2).count(), 1);
    }

    #[test]
    fn test_add_edge_invalid_nodes() {
        let mut graph = Graph::new();
        let node1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));

        let invalid_key = NodeIndex::new(999);

        assert!(
            graph
                .add_edge(invalid_key, node1, EdgeType::Hyperlink)
                .is_none()
        );
        assert!(
            graph
                .add_edge(node1, invalid_key, EdgeType::Hyperlink)
                .is_none()
        );
    }

    #[test]
    fn test_add_multiple_edges() {
        let mut graph = Graph::new();
        let node1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let node2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        let node3 = graph.add_node("https://c.com".to_string(), Point2D::new(2.0, 2.0));

        graph.add_edge(node1, node2, EdgeType::Hyperlink).unwrap();
        graph.add_edge(node1, node3, EdgeType::Hyperlink).unwrap();
        graph.add_edge(node2, node3, EdgeType::Hyperlink).unwrap();

        assert_eq!(graph.edge_count(), 3);

        // Check node1 has 2 outgoing neighbors
        assert_eq!(graph.out_neighbors(node1).count(), 2);

        // Check node3 has 2 incoming neighbors
        assert_eq!(graph.in_neighbors(node3).count(), 2);
    }

    #[test]
    fn test_remove_edges_by_type_between_nodes() {
        let mut graph = Graph::new();
        let a = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let b = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));

        graph.add_edge(a, b, EdgeType::Hyperlink).unwrap();
        graph.add_edge(a, b, EdgeType::UserGrouped).unwrap();
        graph.add_edge(a, b, EdgeType::UserGrouped).unwrap();

        let removed = graph.remove_edges(a, b, EdgeType::UserGrouped);
        assert_eq!(removed, 2);
        assert_eq!(graph.edge_count(), 1);
        assert!(
            graph
                .edges()
                .all(|edge| edge.edge_type == EdgeType::Hyperlink)
        );
    }

    #[test]
    fn test_remove_node() {
        let mut graph = Graph::new();
        let n1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let n2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        graph.add_edge(n1, n2, EdgeType::Hyperlink);

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);

        assert!(graph.remove_node(n1));
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0); // edge auto-removed
        assert!(graph.get_node(n1).is_none());
        assert!(graph.get_node_by_url("https://a.com").is_none());

        // n2 still exists
        assert!(graph.get_node(n2).is_some());
    }

    #[test]
    fn test_remove_nonexistent_node() {
        let mut graph = Graph::new();
        assert!(!graph.remove_node(NodeIndex::new(999)));
    }

    #[test]
    fn test_nodes_iterator() {
        let mut graph = Graph::new();
        graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        graph.add_node("https://c.com".to_string(), Point2D::new(2.0, 2.0));

        let urls: Vec<String> = graph.nodes().map(|(_, n)| n.url.clone()).collect();
        assert_eq!(urls.len(), 3);
        assert!(urls.contains(&"https://a.com".to_string()));
        assert!(urls.contains(&"https://b.com".to_string()));
        assert!(urls.contains(&"https://c.com".to_string()));
    }

    #[test]
    fn test_edges_iterator() {
        let mut graph = Graph::new();
        let node1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let node2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        let node3 = graph.add_node("https://c.com".to_string(), Point2D::new(2.0, 2.0));

        graph.add_edge(node1, node2, EdgeType::Hyperlink);
        graph.add_edge(node1, node3, EdgeType::Hyperlink);

        let edge_count = graph.edges().count();
        assert_eq!(edge_count, 2);

        let edge_types: Vec<EdgeType> = graph.edges().map(|e| e.edge_type).collect();
        assert!(edge_types.iter().all(|&t| t == EdgeType::Hyperlink));
    }

    #[test]
    fn test_node_lifecycle_default() {
        let mut graph = Graph::new();
        let key = graph.add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));

        let node = graph.get_node(key).unwrap();
        assert_eq!(node.lifecycle, NodeLifecycle::Cold);
    }

    #[test]
    fn test_empty_graph_operations() {
        let graph = Graph::new();

        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
        assert!(graph.get_node_by_url("https://example.com").is_none());

        let invalid_key = NodeIndex::new(999);
        assert!(graph.get_node(invalid_key).is_none());
    }

    #[test]
    fn test_node_count() {
        let mut graph = Graph::new();
        assert_eq!(graph.node_count(), 0);

        graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        assert_eq!(graph.node_count(), 1);

        graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));
        assert_eq!(graph.node_count(), 2);
    }

    #[test]
    fn test_edge_count() {
        let mut graph = Graph::new();
        let node1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let node2 = graph.add_node("https://b.com".to_string(), Point2D::new(1.0, 1.0));

        assert_eq!(graph.edge_count(), 0);

        graph.add_edge(node1, node2, EdgeType::Hyperlink);
        assert_eq!(graph.edge_count(), 1);

        graph.add_edge(node2, node1, EdgeType::Hyperlink);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let mut graph = Graph::new();
        let n1 = graph.add_node("https://a.com".to_string(), Point2D::new(10.0, 20.0));
        let n2 = graph.add_node("https://b.com".to_string(), Point2D::new(30.0, 40.0));
        graph.add_edge(n1, n2, EdgeType::Hyperlink);

        graph.get_node_mut(n1).unwrap().title = "Site A".to_string();
        graph.get_node_mut(n2).unwrap().is_pinned = true;

        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);

        assert_eq!(restored.node_count(), 2);
        assert_eq!(restored.edge_count(), 1);

        let (_, ra) = restored.get_node_by_url("https://a.com").unwrap();
        assert_eq!(ra.title, "Site A");
        assert_eq!(ra.position.x, 10.0);
        assert_eq!(ra.position.y, 20.0);

        let (_, rb) = restored.get_node_by_url("https://b.com").unwrap();
        assert!(rb.is_pinned);
        assert_eq!(rb.position.x, 30.0);
    }

    #[test]
    fn test_snapshot_empty_graph() {
        let graph = Graph::new();
        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);

        assert_eq!(restored.node_count(), 0);
        assert_eq!(restored.edge_count(), 0);
    }

    #[test]
    fn test_snapshot_preserves_edge_types() {
        let mut graph = Graph::new();
        let n1 = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let n2 = graph.add_node("https://b.com".to_string(), Point2D::new(100.0, 0.0));
        let n3 = graph.add_node("https://c.com".to_string(), Point2D::new(200.0, 0.0));
        graph.add_edge(n1, n2, EdgeType::Hyperlink);
        graph.add_edge(n2, n1, EdgeType::History);
        graph.add_edge(n1, n3, EdgeType::UserGrouped);

        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);

        assert_eq!(restored.edge_count(), 3);

        let edges: Vec<_> = restored.edges().collect();
        let has_hyperlink = edges.iter().any(|e| e.edge_type == EdgeType::Hyperlink);
        let has_history = edges.iter().any(|e| e.edge_type == EdgeType::History);
        let has_user_grouped = edges.iter().any(|e| e.edge_type == EdgeType::UserGrouped);
        assert!(has_hyperlink);
        assert!(has_history);
        assert!(has_user_grouped);
    }

    #[test]
    fn test_snapshot_preserves_favicon_data() {
        let mut graph = Graph::new();
        let key = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let favicon = vec![255, 0, 0, 255];
        if let Some(node) = graph.get_node_mut(key) {
            node.favicon_rgba = Some(favicon.clone());
            node.favicon_width = 1;
            node.favicon_height = 1;
        }

        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);
        let (_, restored_node) = restored.get_node_by_url("https://a.com").unwrap();
        assert_eq!(restored_node.favicon_rgba.as_ref(), Some(&favicon));
        assert_eq!(restored_node.favicon_width, 1);
        assert_eq!(restored_node.favicon_height, 1);
    }

    #[test]
    fn test_snapshot_preserves_thumbnail_data() {
        let mut graph = Graph::new();
        let key = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let thumbnail = vec![137, 80, 78, 71];
        if let Some(node) = graph.get_node_mut(key) {
            node.thumbnail_png = Some(thumbnail.clone());
            node.thumbnail_width = 64;
            node.thumbnail_height = 48;
        }

        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);
        let (_, restored_node) = restored.get_node_by_url("https://a.com").unwrap();
        assert_eq!(restored_node.thumbnail_png.as_ref(), Some(&thumbnail));
        assert_eq!(restored_node.thumbnail_width, 64);
        assert_eq!(restored_node.thumbnail_height, 48);
    }

    #[test]
    fn test_snapshot_preserves_uuid_identity() {
        let mut graph = Graph::new();
        let key = graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = graph.get_node(key).unwrap().id;

        let snapshot = graph.to_snapshot();
        let restored = Graph::from_snapshot(&snapshot);
        let (_, restored_node) = restored.get_node_by_id(node_id).unwrap();
        assert_eq!(restored_node.url, "https://a.com");
    }

    // --- TEST-3: from_snapshot edge cases ---

    #[test]
    fn test_snapshot_edge_with_missing_url_is_dropped() {
        use crate::persistence::types::{
            GraphSnapshot, PersistedEdge, PersistedEdgeType, PersistedNode,
        };

        let snapshot = GraphSnapshot {
            nodes: vec![PersistedNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://a.com".to_string(),
                title: String::new(),
                position_x: 0.0,
                position_y: 0.0,
                is_pinned: false,
                history_entries: vec![],
                history_index: 0,
                thumbnail_png: None,
                thumbnail_width: 0,
                thumbnail_height: 0,
                favicon_rgba: None,
                favicon_width: 0,
                favicon_height: 0,
                session_state: None,
            }],
            edges: vec![PersistedEdge {
                from_node_id: Uuid::new_v4().to_string(),
                to_node_id: Uuid::new_v4().to_string(),
                edge_type: PersistedEdgeType::Hyperlink,
            }],
            timestamp_secs: 0,
        };

        let graph = Graph::from_snapshot(&snapshot);

        // Node should be restored, edge should be silently dropped
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_snapshot_duplicate_urls_last_wins() {
        use crate::persistence::types::{GraphSnapshot, PersistedNode};

        let snapshot = GraphSnapshot {
            nodes: vec![
                PersistedNode {
                    node_id: Uuid::new_v4().to_string(),
                    url: "https://same.com".to_string(),
                    title: "First".to_string(),
                    position_x: 0.0,
                    position_y: 0.0,
                    is_pinned: false,
                    history_entries: vec![],
                    history_index: 0,
                    thumbnail_png: None,
                    thumbnail_width: 0,
                    thumbnail_height: 0,
                    favicon_rgba: None,
                    favicon_width: 0,
                    favicon_height: 0,
                    session_state: None,
                },
                PersistedNode {
                    node_id: Uuid::new_v4().to_string(),
                    url: "https://same.com".to_string(),
                    title: "Second".to_string(),
                    position_x: 100.0,
                    position_y: 100.0,
                    is_pinned: false,
                    history_entries: vec![],
                    history_index: 0,
                    thumbnail_png: None,
                    thumbnail_width: 0,
                    thumbnail_height: 0,
                    favicon_rgba: None,
                    favicon_width: 0,
                    favicon_height: 0,
                    session_state: None,
                },
            ],
            edges: vec![],
            timestamp_secs: 0,
        };

        let graph = Graph::from_snapshot(&snapshot);

        // Both nodes are created and lookup keeps last inserted semantics.
        assert_eq!(graph.node_count(), 2);
        let (_, node) = graph.get_node_by_url("https://same.com").unwrap();
        assert_eq!(node.title, "Second");
    }

    #[test]
    fn test_update_node_url() {
        let mut graph = Graph::new();
        let key = graph.add_node("old".to_string(), Point2D::new(0.0, 0.0));

        let old = graph.update_node_url(key, "new".to_string());

        assert_eq!(old, Some("old".to_string()));
        assert_eq!(graph.get_node(key).unwrap().url, "new");
        assert!(graph.get_node_by_url("new").is_some());
        assert!(graph.get_node_by_url("old").is_none());
    }

    #[test]
    fn test_update_node_url_nonexistent() {
        let mut graph = Graph::new();
        let fake_key = NodeKey::new(999);

        assert_eq!(graph.update_node_url(fake_key, "x".to_string()), None);
    }

    #[test]
    fn test_cold_restore_reapplies_history_index() {
        use crate::persistence::types::{GraphSnapshot, PersistedNode, PersistedNodeSessionState};

        let node_id = Uuid::new_v4();
        let snapshot = GraphSnapshot {
            nodes: vec![PersistedNode {
                node_id: node_id.to_string(),
                url: "https://fallback.example".to_string(),
                title: "Node".to_string(),
                position_x: 0.0,
                position_y: 0.0,
                is_pinned: false,
                history_entries: vec!["https://legacy.example".to_string()],
                history_index: 0,
                thumbnail_png: None,
                thumbnail_width: 0,
                thumbnail_height: 0,
                favicon_rgba: None,
                favicon_width: 0,
                favicon_height: 0,
                session_state: Some(PersistedNodeSessionState {
                    history_entries: vec![
                        "https://example.com/one".to_string(),
                        "https://example.com/two".to_string(),
                        "https://example.com/three".to_string(),
                    ],
                    history_index: 2,
                    scroll_x: Some(4.0),
                    scroll_y: Some(120.0),
                    form_draft: None,
                }),
            }],
            edges: vec![],
            timestamp_secs: 0,
        };

        let restored = Graph::from_snapshot(&snapshot);
        let (_, node) = restored.get_node_by_id(node_id).unwrap();
        assert_eq!(node.history_entries.len(), 3);
        assert_eq!(node.history_index, 2);
    }

    #[test]
    fn test_cold_restore_reapplies_scroll_offset() {
        use crate::persistence::types::{GraphSnapshot, PersistedNode, PersistedNodeSessionState};

        let snapshot = GraphSnapshot {
            nodes: vec![PersistedNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://example.com".to_string(),
                title: "Node".to_string(),
                position_x: 0.0,
                position_y: 0.0,
                is_pinned: false,
                history_entries: vec![],
                history_index: 0,
                thumbnail_png: None,
                thumbnail_width: 0,
                thumbnail_height: 0,
                favicon_rgba: None,
                favicon_width: 0,
                favicon_height: 0,
                session_state: Some(PersistedNodeSessionState {
                    history_entries: vec!["https://example.com".to_string()],
                    history_index: 0,
                    scroll_x: Some(20.0),
                    scroll_y: Some(640.0),
                    form_draft: None,
                }),
            }],
            edges: vec![],
            timestamp_secs: 0,
        };

        let restored = Graph::from_snapshot(&snapshot);
        let (_, node) = restored.get_node_by_url("https://example.com").unwrap();
        assert_eq!(node.session_scroll, Some((20.0, 640.0)));
    }

    #[test]
    fn test_restore_fallback_without_session_state() {
        use crate::persistence::types::{GraphSnapshot, PersistedNode};

        let snapshot = GraphSnapshot {
            nodes: vec![PersistedNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://fallback.example".to_string(),
                title: "Node".to_string(),
                position_x: 0.0,
                position_y: 0.0,
                is_pinned: false,
                history_entries: vec!["https://legacy-one.example".to_string()],
                history_index: 0,
                thumbnail_png: None,
                thumbnail_width: 0,
                thumbnail_height: 0,
                favicon_rgba: None,
                favicon_width: 0,
                favicon_height: 0,
                session_state: None,
            }],
            edges: vec![],
            timestamp_secs: 0,
        };

        let restored = Graph::from_snapshot(&snapshot);
        let (_, node) = restored
            .get_node_by_url("https://fallback.example")
            .unwrap();
        assert_eq!(
            node.history_entries,
            vec!["https://legacy-one.example".to_string()]
        );
        assert_eq!(node.history_index, 0);
        assert_eq!(node.session_scroll, None);
    }
}
