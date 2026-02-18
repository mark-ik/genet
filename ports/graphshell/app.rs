/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Application state management for the graph browser.

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::graph::egui_adapter::EguiGraphState;
use crate::graph::{EdgeType, Graph, NodeKey};
use crate::persistence::GraphStore;
use crate::persistence::types::{LogEntry, PersistedEdgeType};
use egui_graphs::FruchtermanReingoldState;
use euclid::default::Point2D;
use log::warn;
use servo::WebViewId;

/// Camera state for zoom bounds enforcement
pub struct Camera {
    pub zoom_min: f32,
    pub zoom_max: f32,
    pub current_zoom: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            zoom_min: 0.1,
            zoom_max: 10.0,
            current_zoom: 1.0,
        }
    }

    /// Clamp a zoom value to the allowed range
    pub fn clamp(&self, zoom: f32) -> f32 {
        zoom.clamp(self.zoom_min, self.zoom_max)
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonical node-selection state.
///
/// This wraps the selected-node set with explicit metadata so consumers can
/// reason about selection changes deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectionState {
    nodes: HashSet<NodeKey>,
    primary: Option<NodeKey>,
    revision: u64,
}

#[derive(Debug, Clone)]
pub struct NodeCrashState {
    pub reason: String,
    pub has_backtrace: bool,
    pub crashed_at: SystemTime,
}

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Monotonic revision incremented whenever the selection changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Primary selected node (most recently selected).
    pub fn primary(&self) -> Option<NodeKey> {
        self.primary
    }

    pub fn select(&mut self, key: NodeKey, multi_select: bool) {
        if multi_select {
            if self.nodes.insert(key) {
                self.primary = Some(key);
                self.revision = self.revision.saturating_add(1);
            }
            return;
        }

        if self.nodes.len() == 1 && self.nodes.contains(&key) && self.primary == Some(key) {
            return;
        }

        self.nodes.clear();
        self.nodes.insert(key);
        self.primary = Some(key);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn clear(&mut self) {
        if self.nodes.is_empty() && self.primary.is_none() {
            return;
        }
        self.nodes.clear();
        self.primary = None;
        self.revision = self.revision.saturating_add(1);
    }
}

impl Deref for SelectionState {
    type Target = HashSet<NodeKey>;

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

/// Deterministic mutation intent boundary for graph state updates.
#[derive(Debug, Clone)]
pub enum GraphIntent {
    TogglePhysics,
    RequestFitToScreen,
    TogglePhysicsPanel,
    ToggleHelpPanel,
    CreateNodeNearCenter,
    CreateNodeAtUrl {
        url: String,
        position: Point2D<f32>,
    },
    RemoveSelectedNodes,
    ClearGraph,
    SelectNode {
        key: NodeKey,
        multi_select: bool,
    },
    SetInteracting {
        interacting: bool,
    },
    SetNodePosition {
        key: NodeKey,
        position: Point2D<f32>,
    },
    SetZoom {
        zoom: f32,
    },
    SetNodeUrl {
        key: NodeKey,
        new_url: String,
    },
    CreateUserGroupedEdge {
        from: NodeKey,
        to: NodeKey,
    },
    RemoveEdge {
        from: NodeKey,
        to: NodeKey,
        edge_type: EdgeType,
    },
    PromoteNodeToActive {
        key: NodeKey,
    },
    DemoteNodeToCold {
        key: NodeKey,
    },
    MapWebviewToNode {
        webview_id: WebViewId,
        key: NodeKey,
    },
    UnmapWebview {
        webview_id: WebViewId,
    },
    WebViewCreated {
        parent_webview_id: WebViewId,
        child_webview_id: WebViewId,
        initial_url: Option<String>,
    },
    WebViewUrlChanged {
        webview_id: WebViewId,
        new_url: String,
    },
    WebViewHistoryChanged {
        webview_id: WebViewId,
        entries: Vec<String>,
        current: usize,
    },
    WebViewTitleChanged {
        webview_id: WebViewId,
        title: Option<String>,
    },
    WebViewCrashed {
        webview_id: WebViewId,
        reason: String,
        has_backtrace: bool,
    },
    SetNodeThumbnail {
        key: NodeKey,
        png_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    SetNodeFavicon {
        key: NodeKey,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// Main application state
pub struct GraphBrowserApp {
    /// The graph data structure
    pub graph: Graph,

    /// Force-directed layout state owned by app/runtime UI controls.
    pub physics: FruchtermanReingoldState,

    /// Physics running state before user drag/pan interaction began.
    physics_running_before_interaction: Option<bool>,

    /// Currently selected nodes (can be multiple)
    pub selected_nodes: SelectionState,

    /// Bidirectional mapping between browser tabs and graph nodes
    webview_to_node: HashMap<WebViewId, NodeKey>,
    node_to_webview: HashMap<NodeKey, WebViewId>,
    /// Runtime-only crash metadata keyed by graph node.
    node_crash_state: HashMap<NodeKey, NodeCrashState>,

    /// Nodes that had webviews before switching to graph view (for restoration).
    /// Managed by the webview_controller module.
    pub(crate) active_webview_nodes: Vec<NodeKey>,

    /// Counter for unique placeholder URLs (about:blank#1, about:blank#2, ...).
    /// Prevents `url_to_node` clobbering when pressing N multiple times.
    next_placeholder_id: u32,

    /// True while the user is actively interacting (drag/pan) with the graph
    pub(crate) is_interacting: bool,

    /// Whether the physics config panel is open
    pub show_physics_panel: bool,

    /// Whether the keyboard shortcut help panel is open
    pub show_help_panel: bool,

    /// One-shot flag: fit graph to screen on next frame (triggered by 'C' key)
    pub fit_to_screen_requested: bool,

    /// Camera state (zoom bounds)
    pub camera: Camera,

    /// Persistent graph store (fjall log + redb snapshots)
    persistence: Option<GraphStore>,

    /// Cached egui_graphs state (persists across frames for drag/interaction)
    pub egui_state: Option<EguiGraphState>,

    /// Flag: egui_state needs rebuild (set when graph structure changes)
    pub egui_state_dirty: bool,
}

impl GraphBrowserApp {
    pub fn default_physics_state() -> FruchtermanReingoldState {
        let mut state = FruchtermanReingoldState::default();
        // Tighter defaults: avoid explosive node drift on resume while
        // preserving enough movement for layout convergence.
        state.c_repulse = 0.75;
        state.c_attract = 0.08;
        state.k_scale = 0.65;
        state.max_step = 10.0;
        state.damping = 0.92;
        state
    }

    /// Create a new graph browser application
    pub fn new() -> Self {
        Self::new_from_dir(GraphStore::default_data_dir())
    }

    /// Create a new graph browser application using a specific persistence directory.
    pub fn new_from_dir(data_dir: PathBuf) -> Self {
        // Try to open persistence store and recover graph
        let (graph, persistence) = match GraphStore::open(data_dir) {
            Ok(store) => {
                let graph = store.recover().unwrap_or_else(Graph::new);
                (graph, Some(store))
            },
            Err(e) => {
                warn!("Failed to open graph store: {e}");
                (Graph::new(), None)
            },
        };

        // Scan recovered graph for existing placeholder IDs to avoid collisions
        let next_placeholder_id = Self::scan_max_placeholder_id(&graph);

        Self {
            graph,
            physics: Self::default_physics_state(),
            physics_running_before_interaction: None,
            selected_nodes: SelectionState::new(),
            webview_to_node: HashMap::new(),
            node_to_webview: HashMap::new(),
            node_crash_state: HashMap::new(),
            active_webview_nodes: Vec::new(),
            next_placeholder_id,
            is_interacting: false,
            show_physics_panel: false,
            show_help_panel: false,
            fit_to_screen_requested: false,
            camera: Camera::new(),
            persistence,
            egui_state: None,
            egui_state_dirty: true,
        }
    }

    /// Create a new graph browser application without persistence (for tests)
    #[cfg(test)]
    pub fn new_for_testing() -> Self {
        Self {
            graph: Graph::new(),
            physics: Self::default_physics_state(),
            physics_running_before_interaction: None,
            selected_nodes: SelectionState::new(),
            webview_to_node: HashMap::new(),
            node_to_webview: HashMap::new(),
            node_crash_state: HashMap::new(),
            active_webview_nodes: Vec::new(),
            next_placeholder_id: 0,
            is_interacting: false,
            show_physics_panel: false,
            show_help_panel: false,
            fit_to_screen_requested: false,
            camera: Camera::new(),
            persistence: None,
            egui_state: None,
            egui_state_dirty: true,
        }
    }

    /// Whether the graph was recovered from persistence (has nodes on startup)
    pub fn has_recovered_graph(&self) -> bool {
        self.graph.node_count() > 0
    }

    /// Select a node
    pub fn select_node(&mut self, key: NodeKey, multi_select: bool) {
        // Ignore stale keys.
        if self.graph.get_node(key).is_none() {
            return;
        }

        self.selected_nodes.select(key, multi_select);

        // Selection changes require egui_graphs state refresh.
        self.egui_state_dirty = true;
    }

    /// Request fit-to-screen on next render frame (one-shot)
    pub fn request_fit_to_screen(&mut self) {
        self.fit_to_screen_requested = true;
    }

    /// Set whether the user is actively interacting with the graph
    pub fn set_interacting(&mut self, interacting: bool) {
        if self.is_interacting == interacting {
            return;
        }
        self.is_interacting = interacting;

        if interacting {
            self.physics_running_before_interaction = Some(self.physics.is_running);
            self.physics.is_running = false;
        } else if let Some(was_running) = self.physics_running_before_interaction.take() {
            self.physics.is_running = was_running;
        }
    }

    /// Apply a batch of intents deterministically in insertion order.
    pub fn apply_intents<I>(&mut self, intents: I)
    where
        I: IntoIterator<Item = GraphIntent>,
    {
        for intent in intents {
            self.apply_intent(intent);
        }
    }

    fn apply_intent(&mut self, intent: GraphIntent) {
        match intent {
            GraphIntent::TogglePhysics => self.toggle_physics(),
            GraphIntent::RequestFitToScreen => self.request_fit_to_screen(),
            GraphIntent::TogglePhysicsPanel => self.toggle_physics_panel(),
            GraphIntent::ToggleHelpPanel => self.toggle_help_panel(),
            GraphIntent::CreateNodeNearCenter => {
                self.create_new_node_near_center();
            },
            GraphIntent::CreateNodeAtUrl { url, position } => {
                let key = self.add_node_and_sync(url, position);
                self.select_node(key, false);
            },
            GraphIntent::RemoveSelectedNodes => self.remove_selected_nodes(),
            GraphIntent::ClearGraph => self.clear_graph(),
            GraphIntent::SelectNode { key, multi_select } => self.select_node(key, multi_select),
            GraphIntent::SetInteracting { interacting } => self.set_interacting(interacting),
            GraphIntent::SetNodePosition { key, position } => {
                if let Some(node) = self.graph.get_node_mut(key) {
                    node.position = position;
                }
            },
            GraphIntent::SetZoom { zoom } => {
                self.camera.current_zoom = self.camera.clamp(zoom);
            },
            GraphIntent::SetNodeUrl { key, new_url } => {
                let _ = self.update_node_url_and_log(key, new_url);
            },
            GraphIntent::CreateUserGroupedEdge { from, to } => {
                self.add_user_grouped_edge_if_missing(from, to);
            },
            GraphIntent::RemoveEdge {
                from,
                to,
                edge_type,
            } => {
                let _ = self.remove_edges_and_log(from, to, edge_type);
            },
            GraphIntent::PromoteNodeToActive { key } => {
                self.promote_node_to_active(key);
            },
            GraphIntent::DemoteNodeToCold { key } => {
                self.demote_node_to_cold(key);
            },
            GraphIntent::MapWebviewToNode { webview_id, key } => {
                self.map_webview_to_node(webview_id, key);
            },
            GraphIntent::UnmapWebview { webview_id } => {
                let _ = self.unmap_webview(webview_id);
            },
            GraphIntent::WebViewCreated {
                parent_webview_id,
                child_webview_id,
                initial_url,
            } => {
                let parent_node = self.get_node_for_webview(parent_webview_id);
                let position = if let Some(parent_key) = parent_node {
                    self.graph
                        .get_node(parent_key)
                        .map(|node| Point2D::new(node.position.x + 140.0, node.position.y + 80.0))
                        .unwrap_or_else(|| Point2D::new(400.0, 300.0))
                } else {
                    Point2D::new(400.0, 300.0)
                };
                let node_url = initial_url
                    .filter(|url| !url.is_empty() && url != "about:blank")
                    .unwrap_or_else(|| self.next_placeholder_url());
                let child_node = self.add_node_and_sync(node_url, position);
                self.apply_intent(GraphIntent::MapWebviewToNode {
                    webview_id: child_webview_id,
                    key: child_node,
                });
                self.apply_intent(GraphIntent::PromoteNodeToActive { key: child_node });
                if let Some(parent_key) = parent_node {
                    let _ = self.add_edge_and_sync(parent_key, child_node, EdgeType::Hyperlink);
                }
                self.select_node(child_node, false);
            },
            GraphIntent::WebViewUrlChanged {
                webview_id,
                new_url,
            } => {
                if new_url.is_empty() {
                    return;
                }
                let Some(node_key) = self.get_node_for_webview(webview_id) else {
                    // URL change should update an existing tab/node, not create a new node.
                    return;
                };
                if let Some(node) = self.graph.get_node_mut(node_key) {
                    node.last_visited = std::time::SystemTime::now();
                }
                if self
                    .graph
                    .get_node(node_key)
                    .map(|n| n.url != new_url)
                    .unwrap_or(false)
                {
                    let _ = self.update_node_url_and_log(node_key, new_url);
                }
            },
            GraphIntent::WebViewHistoryChanged {
                webview_id,
                entries,
                current,
            } => {
                // Delegate traces show traversal can change history index even when URL callbacks
                // remain on the latest route string. Treat history index/list as authoritative.
                let Some(node_key) = self.get_node_for_webview(webview_id) else {
                    return;
                };
                let (old_entries, old_index) = if let Some(node) = self.graph.get_node(node_key) {
                    (node.history_entries.clone(), node.history_index)
                } else {
                    return;
                };
                let new_index = if entries.is_empty() {
                    0
                } else {
                    current.min(entries.len() - 1)
                };
                self.maybe_add_history_traversal_edge(
                    node_key,
                    &old_entries,
                    old_index,
                    &entries,
                    new_index,
                );
                if let Some(node) = self.graph.get_node_mut(node_key) {
                    node.history_entries = entries;
                    node.history_index = new_index;
                }
            },
            GraphIntent::WebViewTitleChanged { webview_id, title } => {
                let Some(node_key) = self.get_node_for_webview(webview_id) else {
                    return;
                };
                let Some(title) = title else {
                    return;
                };
                if title.is_empty() {
                    return;
                }
                let mut changed = false;
                if let Some(node) = self.graph.get_node_mut(node_key) {
                    if node.title != title {
                        node.title = title;
                        changed = true;
                    }
                }
                if changed {
                    self.log_title_mutation(node_key);
                    self.egui_state_dirty = true;
                }
            },
            GraphIntent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace,
            } => {
                if let Some(node_key) = self.get_node_for_webview(webview_id) {
                    self.node_crash_state.insert(
                        node_key,
                        NodeCrashState {
                            reason: reason.clone(),
                            has_backtrace,
                            crashed_at: SystemTime::now(),
                        },
                    );
                    self.demote_node_to_cold(node_key);
                } else {
                    let _ = self.unmap_webview(webview_id);
                }
                warn!(
                    "WebView {:?} crashed: reason={} has_backtrace={}",
                    webview_id, reason, has_backtrace
                );
            },
            GraphIntent::SetNodeThumbnail {
                key,
                png_bytes,
                width,
                height,
            } => {
                if let Some(node) = self.graph.get_node_mut(key) {
                    node.thumbnail_png = Some(png_bytes);
                    node.thumbnail_width = width;
                    node.thumbnail_height = height;
                    self.egui_state_dirty = true;
                }
            },
            GraphIntent::SetNodeFavicon {
                key,
                rgba,
                width,
                height,
            } => {
                if let Some(node) = self.graph.get_node_mut(key) {
                    node.favicon_rgba = Some(rgba);
                    node.favicon_width = width;
                    node.favicon_height = height;
                    self.egui_state_dirty = true;
                }
            },
        }
    }

    /// Add a new node and mark render state as dirty.
    pub fn add_node_and_sync(
        &mut self,
        url: String,
        position: euclid::default::Point2D<f32>,
    ) -> NodeKey {
        let key = self.graph.add_node(url.clone(), position);
        if let Some(store) = &mut self.persistence
            && let Some(node) = self.graph.get_node(key)
        {
            store.log_mutation(&LogEntry::AddNode {
                node_id: node.id.to_string(),
                url,
                position_x: position.x,
                position_y: position.y,
            });
        }
        self.egui_state_dirty = true; // Graph structure changed
        key
    }

    /// Add a new edge with persistence logging.
    pub fn add_edge_and_sync(
        &mut self,
        from_key: NodeKey,
        to_key: NodeKey,
        edge_type: crate::graph::EdgeType,
    ) -> Option<crate::graph::EdgeKey> {
        let edge_key = self.graph.add_edge(from_key, to_key, edge_type);
        if edge_key.is_some() {
            self.log_edge_mutation(from_key, to_key, edge_type);
            self.egui_state_dirty = true; // Graph structure changed
        }
        edge_key
    }

    /// Remove directed edges of a specific type and log the mutation.
    /// Returns number of removed edges.
    pub fn remove_edges_and_log(
        &mut self,
        from_key: NodeKey,
        to_key: NodeKey,
        edge_type: crate::graph::EdgeType,
    ) -> usize {
        let removed = self.graph.remove_edges(from_key, to_key, edge_type);
        if removed > 0 {
            self.log_edge_removal_mutation(from_key, to_key, edge_type);
            self.egui_state_dirty = true;
        }
        removed
    }

    /// Log an edge addition to persistence
    pub fn log_edge_mutation(
        &mut self,
        from_key: NodeKey,
        to_key: NodeKey,
        edge_type: crate::graph::EdgeType,
    ) {
        if let Some(store) = &mut self.persistence {
            let from_id = self.graph.get_node(from_key).map(|n| n.id.to_string());
            let to_id = self.graph.get_node(to_key).map(|n| n.id.to_string());
            let (Some(from_node_id), Some(to_node_id)) = (from_id, to_id) else {
                return;
            };
            let persisted_type = match edge_type {
                crate::graph::EdgeType::Hyperlink => PersistedEdgeType::Hyperlink,
                crate::graph::EdgeType::History => PersistedEdgeType::History,
                crate::graph::EdgeType::UserGrouped => PersistedEdgeType::UserGrouped,
            };
            store.log_mutation(&LogEntry::AddEdge {
                from_node_id,
                to_node_id,
                edge_type: persisted_type,
            });
        }
    }

    /// Log an edge removal to persistence.
    pub fn log_edge_removal_mutation(
        &mut self,
        from_key: NodeKey,
        to_key: NodeKey,
        edge_type: crate::graph::EdgeType,
    ) {
        if let Some(store) = &mut self.persistence {
            let from_id = self.graph.get_node(from_key).map(|n| n.id.to_string());
            let to_id = self.graph.get_node(to_key).map(|n| n.id.to_string());
            let (Some(from_node_id), Some(to_node_id)) = (from_id, to_id) else {
                return;
            };
            let persisted_type = match edge_type {
                crate::graph::EdgeType::Hyperlink => PersistedEdgeType::Hyperlink,
                crate::graph::EdgeType::History => PersistedEdgeType::History,
                crate::graph::EdgeType::UserGrouped => PersistedEdgeType::UserGrouped,
            };
            store.log_mutation(&LogEntry::RemoveEdge {
                from_node_id,
                to_node_id,
                edge_type: persisted_type,
            });
        }
    }

    /// Log a title update to persistence
    pub fn log_title_mutation(&mut self, node_key: NodeKey) {
        if let Some(store) = &mut self.persistence {
            if let Some(node) = self.graph.get_node(node_key) {
                store.log_mutation(&LogEntry::UpdateNodeTitle {
                    node_id: node.id.to_string(),
                    title: node.title.clone(),
                });
            }
        }
    }

    /// Check if it's time for a periodic snapshot
    pub fn check_periodic_snapshot(&mut self) {
        if let Some(store) = &mut self.persistence {
            store.check_periodic_snapshot(&self.graph);
        }
    }

    /// Configure periodic persistence snapshot interval in seconds.
    pub fn set_snapshot_interval_secs(&mut self, secs: u64) -> Result<(), String> {
        let store = self
            .persistence
            .as_mut()
            .ok_or_else(|| "Persistence is not available".to_string())?;
        store
            .set_snapshot_interval_secs(secs)
            .map_err(|e| e.to_string())
    }

    /// Current periodic persistence snapshot interval in seconds, if persistence is enabled.
    pub fn snapshot_interval_secs(&self) -> Option<u64> {
        self.persistence
            .as_ref()
            .map(|store| store.snapshot_interval_secs())
    }

    /// Take an immediate snapshot (e.g., on shutdown)
    pub fn take_snapshot(&mut self) {
        if let Some(store) = &mut self.persistence {
            store.take_snapshot(&self.graph);
        }
    }

    /// Persist serialized tile layout JSON.
    pub fn save_tile_layout_json(&mut self, layout_json: &str) {
        if let Some(store) = &mut self.persistence
            && let Err(e) = store.save_tile_layout_json(layout_json)
        {
            warn!("Failed to save tile layout: {e}");
        }
    }

    /// Load serialized tile layout JSON from persistence.
    pub fn load_tile_layout_json(&self) -> Option<String> {
        self.persistence
            .as_ref()
            .and_then(|store| store.load_tile_layout_json())
    }

    /// Switch persistence backing store at runtime and reload graph state from it.
    pub fn switch_persistence_dir(&mut self, data_dir: PathBuf) -> Result<(), String> {
        let store = GraphStore::open(data_dir).map_err(|e| e.to_string())?;
        let graph = store.recover().unwrap_or_else(Graph::new);
        let next_placeholder_id = Self::scan_max_placeholder_id(&graph);

        self.graph = graph;
        self.persistence = Some(store);
        self.selected_nodes.clear();
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.node_crash_state.clear();
        self.active_webview_nodes.clear();
        self.next_placeholder_id = next_placeholder_id;
        self.egui_state = None;
        self.egui_state_dirty = true;
        self.is_interacting = false;
        self.physics_running_before_interaction = None;
        Ok(())
    }

    /// Add a bidirectional mapping between a webview and a node
    pub fn map_webview_to_node(&mut self, webview_id: WebViewId, node_key: NodeKey) {
        self.webview_to_node.insert(webview_id, node_key);
        self.node_to_webview.insert(node_key, webview_id);
    }

    /// Remove the mapping for a webview and its corresponding node
    pub fn unmap_webview(&mut self, webview_id: WebViewId) -> Option<NodeKey> {
        if let Some(node_key) = self.webview_to_node.remove(&webview_id) {
            self.node_to_webview.remove(&node_key);
            Some(node_key)
        } else {
            None
        }
    }

    /// Get the node key for a given webview
    pub fn get_node_for_webview(&self, webview_id: WebViewId) -> Option<NodeKey> {
        self.webview_to_node.get(&webview_id).copied()
    }

    pub fn get_node_crash_state(&self, node_key: NodeKey) -> Option<&NodeCrashState> {
        self.node_crash_state.get(&node_key)
    }

    /// Get the webview ID for a given node
    pub fn get_webview_for_node(&self, node_key: NodeKey) -> Option<WebViewId> {
        self.node_to_webview.get(&node_key).copied()
    }

    /// Get all webview-node mappings as an iterator
    pub fn webview_node_mappings(&self) -> impl Iterator<Item = (WebViewId, NodeKey)> + '_ {
        self.webview_to_node.iter().map(|(&wv, &nk)| (wv, nk))
    }

    /// Toggle force-directed layout simulation.
    pub fn toggle_physics(&mut self) {
        if self.is_interacting {
            let next = !self
                .physics_running_before_interaction
                .unwrap_or(self.physics.is_running);
            self.physics_running_before_interaction = Some(next);
            return;
        }
        self.physics.is_running = !self.physics.is_running;
    }

    /// Update force-directed layout configuration.
    pub fn update_physics_config(&mut self, config: FruchtermanReingoldState) {
        self.physics = config;
    }

    /// Toggle physics config panel visibility
    pub fn toggle_physics_panel(&mut self) {
        self.show_physics_panel = !self.show_physics_panel;
    }

    /// Toggle keyboard shortcut help panel visibility
    pub fn toggle_help_panel(&mut self) {
        self.show_help_panel = !self.show_help_panel;
    }

    /// Promote a node to Active lifecycle (mark as needing webview)
    pub fn promote_node_to_active(&mut self, node_key: NodeKey) {
        use crate::graph::NodeLifecycle;
        if let Some(node) = self.graph.get_node_mut(node_key) {
            node.lifecycle = NodeLifecycle::Active;
        }
        self.node_crash_state.remove(&node_key);
    }

    /// Demote a node to Cold lifecycle (mark as not needing webview)
    pub fn demote_node_to_cold(&mut self, node_key: NodeKey) {
        use crate::graph::NodeLifecycle;
        if let Some(node) = self.graph.get_node_mut(node_key) {
            node.lifecycle = NodeLifecycle::Cold;
        }
        // Also unmap webview association if it exists
        if let Some(webview_id) = self.node_to_webview.get(&node_key).copied() {
            self.webview_to_node.remove(&webview_id);
            self.node_to_webview.remove(&node_key);
        }
    }

    /// Scan graph for existing `about:blank#N` placeholder URLs and return
    /// the next available ID (max found + 1, or 0 if none exist).
    fn scan_max_placeholder_id(graph: &Graph) -> u32 {
        let mut max_id = 0u32;
        for (_, node) in graph.nodes() {
            if let Some(fragment) = node.url.strip_prefix("about:blank#") {
                if let Ok(id) = fragment.parse::<u32>() {
                    max_id = max_id.max(id + 1);
                }
            }
        }
        max_id
    }

    /// Generate a unique placeholder URL for a new node.
    fn next_placeholder_url(&mut self) -> String {
        let url = format!("about:blank#{}", self.next_placeholder_id);
        self.next_placeholder_id += 1;
        url
    }

    fn maybe_add_history_traversal_edge(
        &mut self,
        node_key: NodeKey,
        old_entries: &[String],
        old_index: usize,
        new_entries: &[String],
        new_index: usize,
    ) {
        let Some(old_url) = old_entries.get(old_index).filter(|url| !url.is_empty()) else {
            return;
        };
        let Some(new_url) = new_entries.get(new_index).filter(|url| !url.is_empty()) else {
            return;
        };
        if old_url == new_url {
            return;
        }

        let is_back = new_index < old_index;
        let is_forward_same_list = new_index > old_index && new_entries.len() == old_entries.len();
        if !is_back && !is_forward_same_list {
            return;
        }

        let from_key = self
            .graph
            .get_nodes_by_url(old_url)
            .into_iter()
            .find(|&key| key != node_key)
            .or(Some(node_key));
        let to_key = self
            .graph
            .get_nodes_by_url(new_url)
            .into_iter()
            .find(|&key| key != node_key)
            .or(Some(node_key));
        let (Some(from_key), Some(to_key)) = (from_key, to_key) else {
            return;
        };

        let has_history_edge = self.graph.edges().any(|edge| {
            edge.edge_type == EdgeType::History && edge.from == from_key && edge.to == to_key
        });
        if !has_history_edge {
            let _ = self.add_edge_and_sync(from_key, to_key, EdgeType::History);
        }
    }

    fn add_user_grouped_edge_if_missing(&mut self, from: NodeKey, to: NodeKey) {
        if from == to {
            return;
        }
        if self.graph.get_node(from).is_none() || self.graph.get_node(to).is_none() {
            return;
        }
        let already_grouped = self.graph.edges().any(|edge| {
            edge.edge_type == EdgeType::UserGrouped && edge.from == from && edge.to == to
        });
        if !already_grouped {
            let _ = self.add_edge_and_sync(from, to, EdgeType::UserGrouped);
        }
    }

    /// Create a new node near the center of the graph (or at origin if graph is empty)
    pub fn create_new_node_near_center(&mut self) -> NodeKey {
        use euclid::default::Point2D;
        use rand::Rng;

        // Calculate approximate center of existing nodes
        let (center_x, center_y) = if self.graph.node_count() > 0 {
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut count = 0;

            for (_, node) in self.graph.nodes() {
                sum_x += node.position.x;
                sum_y += node.position.y;
                count += 1;
            }

            (sum_x / count as f32, sum_y / count as f32)
        } else {
            (400.0, 300.0) // Default center if no nodes
        };

        // Add random offset to avoid stacking directly on center
        let mut rng = rand::thread_rng();
        let offset_x = rng.gen_range(-100.0..100.0);
        let offset_y = rng.gen_range(-100.0..100.0);

        let position = Point2D::new(center_x + offset_x, center_y + offset_y);
        let placeholder_url = self.next_placeholder_url();

        let key = self.add_node_and_sync(placeholder_url, position);

        // Select the newly created node
        self.select_node(key, false);

        key
    }

    /// Remove selected nodes and their associated webviews.
    /// Note: actual webview closure must be handled by the caller (gui.rs)
    /// since we don't hold a window reference.
    pub fn remove_selected_nodes(&mut self) {
        let nodes_to_remove: Vec<NodeKey> = self.selected_nodes.iter().copied().collect();

        for node_key in nodes_to_remove {
            // Log removal to persistence before removing from graph
            if let Some(store) = &mut self.persistence {
                if let Some(node) = self.graph.get_node(node_key) {
                    store.log_mutation(&LogEntry::RemoveNode {
                        node_id: node.id.to_string(),
                    });
                }
            }

            // Unmap webview if it exists
            if let Some(webview_id) = self.node_to_webview.get(&node_key).copied() {
                self.webview_to_node.remove(&webview_id);
                self.node_to_webview.remove(&node_key);
            }
            self.node_crash_state.remove(&node_key);

            // Remove from graph
            self.graph.remove_node(node_key);
            self.egui_state_dirty = true;
        }

        // Clear selection
        self.selected_nodes.clear();
    }

    /// Get the currently selected node (if exactly one is selected)
    pub fn get_single_selected_node(&self) -> Option<NodeKey> {
        if self.selected_nodes.len() == 1 {
            self.selected_nodes.primary()
        } else {
            None
        }
    }

    /// Clear the entire graph and all webview mappings.
    /// Webview closure must be handled by the caller (gui.rs) since we don't
    /// hold a reference to the window.
    pub fn clear_graph(&mut self) {
        if let Some(store) = &mut self.persistence {
            store.log_mutation(&LogEntry::ClearGraph);
        }
        self.graph = Graph::new();
        self.selected_nodes.clear();
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.node_crash_state.clear();
        self.egui_state_dirty = true;
    }

    /// Clear the graph in memory and wipe all persisted graph data.
    pub fn clear_graph_and_persistence(&mut self) {
        if let Some(store) = &mut self.persistence {
            if let Err(e) = store.clear_all() {
                warn!("Failed to clear persisted graph data: {e}");
            }
        }
        self.graph = Graph::new();
        self.selected_nodes.clear();
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.node_crash_state.clear();
        self.active_webview_nodes.clear();
        self.next_placeholder_id = 0;
        self.egui_state_dirty = true;
    }

    /// Update a node's URL and log to persistence.
    /// Returns the old URL, or None if the node doesn't exist.
    pub fn update_node_url_and_log(&mut self, key: NodeKey, new_url: String) -> Option<String> {
        let old_url = self.graph.update_node_url(key, new_url.clone())?;
        if let Some(store) = &mut self.persistence {
            if let Some(node) = self.graph.get_node(key) {
                store.log_mutation(&LogEntry::UpdateNodeUrl {
                    node_id: node.id.to_string(),
                    new_url,
                });
            }
        }
        self.egui_state_dirty = true;
        Some(old_url)
    }
}

impl Default for GraphBrowserApp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use euclid::default::Point2D;
    use tempfile::TempDir;
    use uuid::Uuid;

    /// Create a unique WebViewId for testing.
    /// Ensures the pipeline namespace is installed on the current thread.
    fn test_webview_id() -> servo::WebViewId {
        thread_local! {
            static NS_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        NS_INSTALLED.with(|cell| {
            if !cell.get() {
                base::id::PipelineNamespace::install(base::id::PipelineNamespaceId(42));
                cell.set(true);
            }
        });
        servo::WebViewId::new(base::id::PainterId::next())
    }

    #[test]
    fn test_select_node_marks_selection_state() {
        let mut app = GraphBrowserApp::new_for_testing();
        let node_key = app
            .graph
            .add_node("test".to_string(), Point2D::new(100.0, 100.0));

        app.select_node(node_key, false);

        // Node should be selected
        assert!(app.selected_nodes.contains(&node_key));
    }

    #[test]
    fn test_request_fit_to_screen() {
        let mut app = GraphBrowserApp::new_for_testing();

        // Initially false
        assert!(!app.fit_to_screen_requested);

        // Request fit to screen
        app.request_fit_to_screen();
        assert!(app.fit_to_screen_requested);

        // Reset (as render would do)
        app.fit_to_screen_requested = false;
        assert!(!app.fit_to_screen_requested);
    }

    #[test]
    fn test_select_node_single() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("test".to_string(), Point2D::new(0.0, 0.0));

        app.select_node(key, false);

        assert_eq!(app.selected_nodes.len(), 1);
        assert!(app.selected_nodes.contains(&key));
    }

    #[test]
    fn test_select_node_multi() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));

        app.select_node(key1, false);
        app.select_node(key2, true);

        assert_eq!(app.selected_nodes.len(), 2);
        assert!(app.selected_nodes.contains(&key1));
        assert!(app.selected_nodes.contains(&key2));
    }

    #[test]
    fn test_selection_revision_increments_on_change() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key2 = app.graph.add_node("b".to_string(), Point2D::new(1.0, 0.0));
        let rev0 = app.selected_nodes.revision();

        app.select_node(key1, false);
        let rev1 = app.selected_nodes.revision();
        assert!(rev1 > rev0);

        app.select_node(key1, false);
        let rev2 = app.selected_nodes.revision();
        assert_eq!(rev2, rev1);

        app.select_node(key2, true);
        let rev3 = app.selected_nodes.revision();
        assert!(rev3 > rev2);
    }

    #[test]
    fn test_intent_webview_created_links_parent_and_selects_child() {
        let mut app = GraphBrowserApp::new_for_testing();
        let parent = app
            .graph
            .add_node("https://parent.com".into(), Point2D::new(10.0, 20.0));
        let parent_wv = test_webview_id();
        let child_wv = test_webview_id();
        app.map_webview_to_node(parent_wv, parent);

        let edges_before = app.graph.edge_count();
        app.apply_intents([GraphIntent::WebViewCreated {
            parent_webview_id: parent_wv,
            child_webview_id: child_wv,
            initial_url: Some("https://child.com".into()),
        }]);

        assert_eq!(app.graph.edge_count(), edges_before + 1);
        let child = app.get_node_for_webview(child_wv).unwrap();
        assert_eq!(app.get_single_selected_node(), Some(child));
        assert_eq!(app.graph.get_node(child).unwrap().url, "https://child.com");
    }

    #[test]
    fn test_intent_webview_created_about_blank_uses_placeholder() {
        let mut app = GraphBrowserApp::new_for_testing();
        let child_wv = test_webview_id();

        app.apply_intents([GraphIntent::WebViewCreated {
            parent_webview_id: test_webview_id(),
            child_webview_id: child_wv,
            initial_url: Some("about:blank".into()),
        }]);

        let child = app.get_node_for_webview(child_wv).unwrap();
        assert!(
            app.graph
                .get_node(child)
                .unwrap()
                .url
                .starts_with("about:blank#")
        );
    }

    #[test]
    fn test_intent_webview_url_changed_updates_existing_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://before.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);

        app.apply_intents([GraphIntent::WebViewUrlChanged {
            webview_id: wv,
            new_url: "https://after.com".into(),
        }]);

        assert_eq!(app.graph.get_node(key).unwrap().url, "https://after.com");
        assert_eq!(app.get_node_for_webview(wv), Some(key));
    }

    #[test]
    fn test_intent_webview_url_changed_ignores_unmapped_webview() {
        let mut app = GraphBrowserApp::new_for_testing();
        let wv = test_webview_id();
        let before = app.graph.node_count();

        app.apply_intents([GraphIntent::WebViewUrlChanged {
            webview_id: wv,
            new_url: "https://ignored.com".into(),
        }]);

        assert_eq!(app.graph.node_count(), before);
        assert_eq!(app.get_node_for_webview(wv), None);
    }

    #[test]
    fn test_intent_webview_history_changed_clamps_index() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);

        app.apply_intents([GraphIntent::WebViewHistoryChanged {
            webview_id: wv,
            entries: vec!["https://a.com".into(), "https://b.com".into()],
            current: 99,
        }]);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.history_entries.len(), 2);
        assert_eq!(node.history_index, 1);
    }

    #[test]
    fn test_intent_webview_history_changed_adds_history_edge_on_back() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://b.com".into(), Point2D::new(100.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, to);
        if let Some(node) = app.graph.get_node_mut(to) {
            node.history_entries = vec!["https://a.com".into(), "https://b.com".into()];
            node.history_index = 1;
        }

        app.apply_intents([GraphIntent::WebViewHistoryChanged {
            webview_id: wv,
            entries: vec!["https://a.com".into(), "https://b.com".into()],
            current: 0,
        }]);

        let has_edge = app
            .graph
            .edges()
            .any(|e| e.edge_type == EdgeType::History && e.from == to && e.to == from);
        assert!(has_edge);
    }

    #[test]
    fn test_intent_webview_history_changed_does_not_add_edge_on_normal_navigation() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://b.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);
        if let Some(node) = app.graph.get_node_mut(key) {
            node.history_entries = vec!["https://a.com".into(), "https://b.com".into()];
            node.history_index = 1;
        }

        app.apply_intents([GraphIntent::WebViewHistoryChanged {
            webview_id: wv,
            entries: vec![
                "https://a.com".into(),
                "https://b.com".into(),
                "https://c.com".into(),
            ],
            current: 2,
        }]);

        let history_edge_count = app
            .graph
            .edges()
            .filter(|e| e.edge_type == EdgeType::History)
            .count();
        assert_eq!(history_edge_count, 0);
    }

    #[test]
    fn test_intent_webview_history_changed_adds_history_edge_on_forward_same_list() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://b.com".into(), Point2D::new(100.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, from);
        if let Some(node) = app.graph.get_node_mut(from) {
            node.history_entries = vec!["https://a.com".into(), "https://b.com".into()];
            node.history_index = 0;
        }

        app.apply_intents([GraphIntent::WebViewHistoryChanged {
            webview_id: wv,
            entries: vec!["https://a.com".into(), "https://b.com".into()],
            current: 1,
        }]);

        let has_edge = app
            .graph
            .edges()
            .any(|e| e.edge_type == EdgeType::History && e.from == from && e.to == to);
        assert!(has_edge);
    }

    #[test]
    fn test_intent_create_user_grouped_edge_adds_single_edge() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://to.com".into(), Point2D::new(10.0, 0.0));

        app.apply_intents([GraphIntent::CreateUserGroupedEdge { from, to }]);

        let count = app
            .graph
            .edges()
            .filter(|e| e.edge_type == EdgeType::UserGrouped && e.from == from && e.to == to)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_intent_create_user_grouped_edge_is_idempotent() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://to.com".into(), Point2D::new(10.0, 0.0));

        app.apply_intents([
            GraphIntent::CreateUserGroupedEdge { from, to },
            GraphIntent::CreateUserGroupedEdge { from, to },
        ]);

        let count = app
            .graph
            .edges()
            .filter(|e| e.edge_type == EdgeType::UserGrouped && e.from == from && e.to == to)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_intent_remove_edge_removes_matching_type_only() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app.add_node_and_sync("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app.add_node_and_sync("https://to.com".into(), Point2D::new(100.0, 0.0));

        let _ = app.add_edge_and_sync(from, to, EdgeType::Hyperlink);
        let _ = app.add_edge_and_sync(from, to, EdgeType::UserGrouped);

        app.apply_intents([GraphIntent::RemoveEdge {
            from,
            to,
            edge_type: EdgeType::UserGrouped,
        }]);

        let has_user_grouped = app
            .graph
            .edges()
            .any(|e| e.edge_type == EdgeType::UserGrouped && e.from == from && e.to == to);
        let has_hyperlink = app
            .graph
            .edges()
            .any(|e| e.edge_type == EdgeType::Hyperlink && e.from == from && e.to == to);
        assert!(!has_user_grouped);
        assert!(has_hyperlink);
    }

    #[test]
    fn test_remove_edges_and_log_reports_removed_count() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app.add_node_and_sync("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app.add_node_and_sync("https://to.com".into(), Point2D::new(100.0, 0.0));

        let _ = app.add_edge_and_sync(from, to, EdgeType::UserGrouped);
        let _ = app.add_edge_and_sync(from, to, EdgeType::UserGrouped);

        let removed = app.remove_edges_and_log(from, to, EdgeType::UserGrouped);
        assert_eq!(removed, 2);
        assert_eq!(
            app.graph
                .edges()
                .filter(|e| e.edge_type == EdgeType::UserGrouped)
                .count(),
            0
        );
    }

    #[test]
    fn test_history_changed_is_authoritative_when_url_callback_stays_latest() {
        let mut app = GraphBrowserApp::new_for_testing();
        let step1 = app.graph.add_node(
            "https://site.example/?step=1".into(),
            Point2D::new(0.0, 0.0),
        );
        let step2 = app.graph.add_node(
            "https://site.example/?step=2".into(),
            Point2D::new(10.0, 0.0),
        );
        let wv = test_webview_id();
        app.map_webview_to_node(wv, step2);
        if let Some(node) = app.graph.get_node_mut(step2) {
            node.history_entries = vec![
                "https://site.example/?step=0".into(),
                "https://site.example/?step=1".into(),
                "https://site.example/?step=2".into(),
            ];
            node.history_index = 2;
        }

        // Mirrors observed delegate behavior: URL callback can stay at the latest route
        // while history callback index moves backward.
        app.apply_intents([
            GraphIntent::WebViewUrlChanged {
                webview_id: wv,
                new_url: "https://site.example/?step=2".into(),
            },
            GraphIntent::WebViewHistoryChanged {
                webview_id: wv,
                entries: vec![
                    "https://site.example/?step=0".into(),
                    "https://site.example/?step=1".into(),
                    "https://site.example/?step=2".into(),
                ],
                current: 1,
            },
        ]);

        let node = app.graph.get_node(step2).unwrap();
        assert_eq!(node.history_index, 1);

        let has_edge = app
            .graph
            .edges()
            .any(|e| e.edge_type == EdgeType::History && e.from == step2 && e.to == step1);
        assert!(has_edge);
    }

    #[test]
    fn test_intent_webview_title_changed_updates_and_ignores_empty() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://title.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);
        let original_title = app.graph.get_node(key).unwrap().title.clone();

        app.apply_intents([GraphIntent::WebViewTitleChanged {
            webview_id: wv,
            title: Some("".into()),
        }]);
        assert_eq!(app.graph.get_node(key).unwrap().title, original_title);

        app.apply_intents([GraphIntent::WebViewTitleChanged {
            webview_id: wv,
            title: Some("Hello".into()),
        }]);
        assert_eq!(app.graph.get_node(key).unwrap().title, "Hello");
    }

    #[test]
    fn test_intent_thumbnail_and_favicon_update_node_metadata() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://assets.com".into(), Point2D::new(0.0, 0.0));

        app.apply_intents([
            GraphIntent::SetNodeThumbnail {
                key,
                png_bytes: vec![1, 2, 3],
                width: 10,
                height: 20,
            },
            GraphIntent::SetNodeFavicon {
                key,
                rgba: vec![255, 0, 0, 255],
                width: 1,
                height: 1,
            },
        ]);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.thumbnail_png.as_ref().unwrap().len(), 3);
        assert_eq!(node.thumbnail_width, 10);
        assert_eq!(node.thumbnail_height, 20);
        assert_eq!(node.favicon_rgba.as_ref().unwrap().len(), 4);
        assert_eq!(node.favicon_width, 1);
        assert_eq!(node.favicon_height, 1);
    }

    #[test]
    fn test_conflict_delete_dominates_title_update_any_order() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://conflict-a.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);
        app.select_node(key, false);
        app.apply_intents([
            GraphIntent::RemoveSelectedNodes,
            GraphIntent::WebViewTitleChanged {
                webview_id: wv,
                title: Some("updated".into()),
            },
        ]);
        assert!(app.graph.get_node(key).is_none());

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://conflict-b.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);
        app.select_node(key, false);
        app.apply_intents([
            GraphIntent::WebViewTitleChanged {
                webview_id: wv,
                title: Some("updated".into()),
            },
            GraphIntent::RemoveSelectedNodes,
        ]);
        assert!(app.graph.get_node(key).is_none());
    }

    #[test]
    fn test_conflict_delete_dominates_metadata_updates() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://conflict-meta.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);
        app.select_node(key, false);

        app.apply_intents([
            GraphIntent::RemoveSelectedNodes,
            GraphIntent::WebViewHistoryChanged {
                webview_id: wv,
                entries: vec!["https://x.com".into()],
                current: 0,
            },
            GraphIntent::SetNodeThumbnail {
                key,
                png_bytes: vec![1, 2, 3],
                width: 8,
                height: 8,
            },
            GraphIntent::SetNodeFavicon {
                key,
                rgba: vec![0, 0, 0, 255],
                width: 1,
                height: 1,
            },
            GraphIntent::SetNodeUrl {
                key,
                new_url: "https://should-not-apply.com".into(),
            },
        ]);

        assert!(app.graph.get_node(key).is_none());
    }

    #[test]
    fn test_conflict_last_writer_wins_for_url_updates() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://start.com".into(), Point2D::new(0.0, 0.0));
        app.apply_intents([
            GraphIntent::SetNodeUrl {
                key,
                new_url: "https://first.com".into(),
            },
            GraphIntent::SetNodeUrl {
                key,
                new_url: "https://second.com".into(),
            },
        ]);
        assert_eq!(app.graph.get_node(key).unwrap().url, "https://second.com");
    }

    #[test]
    #[ignore]
    fn perf_apply_intent_batch_10k_under_budget() {
        let mut app = GraphBrowserApp::new_for_testing();
        let mut intents = Vec::new();
        for i in 0..10_000 {
            intents.push(GraphIntent::CreateNodeAtUrl {
                url: format!("https://perf/{i}"),
                position: Point2D::new((i % 100) as f32, (i / 100) as f32),
            });
        }
        let start = std::time::Instant::now();
        app.apply_intents(intents);
        let elapsed = start.elapsed();
        assert_eq!(app.graph.node_count(), 10_000);
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "intent batch exceeded budget: {elapsed:?}"
        );
    }

    #[test]
    fn test_camera_defaults() {
        let cam = Camera::new();
        assert_eq!(cam.zoom_min, 0.1);
        assert_eq!(cam.zoom_max, 10.0);
        assert_eq!(cam.current_zoom, 1.0);
    }

    #[test]
    fn test_camera_clamp_within_range() {
        let cam = Camera::new();
        assert_eq!(cam.clamp(1.0), 1.0);
        assert_eq!(cam.clamp(5.0), 5.0);
        assert_eq!(cam.clamp(0.5), 0.5);
    }

    #[test]
    fn test_camera_clamp_below_min() {
        let cam = Camera::new();
        assert_eq!(cam.clamp(0.05), 0.1);
        assert_eq!(cam.clamp(0.0), 0.1);
        assert_eq!(cam.clamp(-1.0), 0.1);
    }

    #[test]
    fn test_camera_clamp_above_max() {
        let cam = Camera::new();
        assert_eq!(cam.clamp(15.0), 10.0);
        assert_eq!(cam.clamp(100.0), 10.0);
    }

    #[test]
    fn test_camera_clamp_at_boundaries() {
        let cam = Camera::new();
        assert_eq!(cam.clamp(0.1), 0.1);
        assert_eq!(cam.clamp(10.0), 10.0);
    }

    #[test]
    fn test_create_multiple_placeholder_nodes_unique_urls() {
        let mut app = GraphBrowserApp::new_for_testing();

        let k1 = app.create_new_node_near_center();
        let k2 = app.create_new_node_near_center();
        let k3 = app.create_new_node_near_center();

        // All three nodes must have distinct URLs
        let url1 = app.graph.get_node(k1).unwrap().url.clone();
        let url2 = app.graph.get_node(k2).unwrap().url.clone();
        let url3 = app.graph.get_node(k3).unwrap().url.clone();

        assert_ne!(url1, url2);
        assert_ne!(url2, url3);
        assert_ne!(url1, url3);

        // All URLs start with about:blank#
        assert!(url1.starts_with("about:blank#"));
        assert!(url2.starts_with("about:blank#"));
        assert!(url3.starts_with("about:blank#"));

        // url_to_node should have 3 distinct entries
        assert_eq!(app.graph.node_count(), 3);
        assert!(app.graph.get_node_by_url(&url1).is_some());
        assert!(app.graph.get_node_by_url(&url2).is_some());
        assert!(app.graph.get_node_by_url(&url3).is_some());
    }

    #[test]
    fn test_placeholder_id_scan_on_recovery() {
        let mut graph = Graph::new();
        graph.add_node("about:blank#5".to_string(), Point2D::new(0.0, 0.0));
        graph.add_node("about:blank#2".to_string(), Point2D::new(100.0, 0.0));
        graph.add_node("https://example.com".to_string(), Point2D::new(200.0, 0.0));

        let next_id = GraphBrowserApp::scan_max_placeholder_id(&graph);
        // Max is 5, so next should be 6
        assert_eq!(next_id, 6);
    }

    #[test]
    fn test_placeholder_id_scan_empty_graph() {
        let graph = Graph::new();
        assert_eq!(GraphBrowserApp::scan_max_placeholder_id(&graph), 0);
    }

    // --- TEST-1: remove_selected_nodes ---

    #[test]
    fn test_remove_selected_nodes_single() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let _k2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));

        app.select_node(k1, false);
        app.remove_selected_nodes();

        assert_eq!(app.graph.node_count(), 1);
        assert!(app.graph.get_node(k1).is_none());
        assert!(app.selected_nodes.is_empty());
    }

    #[test]
    fn test_remove_selected_nodes_multi() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let k2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));
        let k3 = app
            .graph
            .add_node("c".to_string(), Point2D::new(200.0, 0.0));

        app.select_node(k1, false);
        app.select_node(k2, true);
        app.remove_selected_nodes();

        assert_eq!(app.graph.node_count(), 1);
        assert!(app.graph.get_node(k3).is_some());
        assert!(app.selected_nodes.is_empty());
    }

    #[test]
    fn test_remove_selected_nodes_empty_selection() {
        let mut app = GraphBrowserApp::new_for_testing();
        app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));

        // No selection — should be a no-op
        app.remove_selected_nodes();
        assert_eq!(app.graph.node_count(), 1);
    }

    #[test]
    fn test_remove_selected_nodes_clears_webview_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));

        // Simulate a webview mapping
        let fake_wv_id = test_webview_id();
        app.map_webview_to_node(fake_wv_id, k1);
        assert!(app.get_node_for_webview(fake_wv_id).is_some());

        app.select_node(k1, false);
        app.remove_selected_nodes();

        // Mapping should be cleaned up
        assert!(app.get_node_for_webview(fake_wv_id).is_none());
        assert!(app.get_webview_for_node(k1).is_none());
    }

    // --- TEST-1: clear_graph ---

    #[test]
    fn test_clear_graph_resets_everything() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let k2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));

        app.select_node(k1, false);
        app.select_node(k2, false);

        let fake_wv_id = test_webview_id();
        app.map_webview_to_node(fake_wv_id, k1);

        app.clear_graph();

        assert_eq!(app.graph.node_count(), 0);
        assert!(app.selected_nodes.is_empty());
        assert!(app.get_node_for_webview(fake_wv_id).is_none());
    }

    // --- TEST-1: create_new_node_near_center ---

    #[test]
    fn test_create_new_node_near_center_empty_graph() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.create_new_node_near_center();

        assert_eq!(app.graph.node_count(), 1);
        assert!(app.selected_nodes.contains(&key));

        let node = app.graph.get_node(key).unwrap();
        assert!(node.url.starts_with("about:blank#"));
    }

    #[test]
    fn test_create_new_node_near_center_selects_node() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app
            .graph
            .add_node("existing".to_string(), Point2D::new(0.0, 0.0));
        app.select_node(k1, false);

        let k2 = app.create_new_node_near_center();

        // New node should be selected, old one deselected
        assert_eq!(app.selected_nodes.len(), 1);
        assert!(app.selected_nodes.contains(&k2));
    }

    // --- TEST-1: demote/promote lifecycle ---

    #[test]
    fn test_promote_and_demote_node_lifecycle() {
        use crate::graph::NodeLifecycle;
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));

        // Default lifecycle is Cold
        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        ));

        app.promote_node_to_active(key);
        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        ));

        app.demote_node_to_cold(key);
        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        ));
    }

    #[test]
    fn test_demote_clears_webview_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let fake_wv_id = test_webview_id();

        app.map_webview_to_node(fake_wv_id, key);
        assert!(app.get_webview_for_node(key).is_some());

        app.demote_node_to_cold(key);
        assert!(app.get_webview_for_node(key).is_none());
        assert!(app.get_node_for_webview(fake_wv_id).is_none());
    }

    #[test]
    fn test_webview_crashed_demotes_node_and_unmaps_webview() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();

        app.promote_node_to_active(key);
        app.map_webview_to_node(wv_id, key);
        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        ));

        app.apply_intents([GraphIntent::WebViewCrashed {
            webview_id: wv_id,
            reason: "gpu reset".to_string(),
            has_backtrace: false,
        }]);

        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        ));
        assert_eq!(
            app.get_node_crash_state(key)
                .map(|state| state.reason.as_str()),
            Some("gpu reset")
        );
        assert!(app.get_node_for_webview(wv_id).is_none());
        assert!(app.get_webview_for_node(key).is_none());

        app.apply_intents([GraphIntent::PromoteNodeToActive { key }]);
        assert!(matches!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        ));
        assert!(app.get_node_crash_state(key).is_none());
    }

    #[test]
    fn test_clear_graph_clears_runtime_crash_state() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();
        app.map_webview_to_node(wv_id, key);
        app.apply_intents([GraphIntent::WebViewCrashed {
            webview_id: wv_id,
            reason: "boom".to_string(),
            has_backtrace: true,
        }]);
        assert!(app.get_node_crash_state(key).is_some());

        app.clear_graph();
        assert!(app.get_node_crash_state(key).is_none());
    }

    // --- TEST-1: webview mapping ---

    #[test]
    fn test_webview_mapping_bidirectional() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();

        app.map_webview_to_node(wv_id, key);

        assert_eq!(app.get_node_for_webview(wv_id), Some(key));
        assert_eq!(app.get_webview_for_node(key), Some(wv_id));
    }

    #[test]
    fn test_unmap_webview() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();

        app.map_webview_to_node(wv_id, key);
        let unmapped_key = app.unmap_webview(wv_id);

        assert_eq!(unmapped_key, Some(key));
        assert!(app.get_node_for_webview(wv_id).is_none());
        assert!(app.get_webview_for_node(key).is_none());
    }

    #[test]
    fn test_unmap_nonexistent_webview() {
        let mut app = GraphBrowserApp::new_for_testing();
        let wv_id = test_webview_id();

        assert_eq!(app.unmap_webview(wv_id), None);
    }

    #[test]
    fn test_webview_node_mappings_iterator() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let k2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));
        let wv1 = test_webview_id();
        let wv2 = test_webview_id();

        app.map_webview_to_node(wv1, k1);
        app.map_webview_to_node(wv2, k2);

        let mappings: Vec<_> = app.webview_node_mappings().collect();
        assert_eq!(mappings.len(), 2);
    }

    // --- TEST-1: get_single_selected_node ---

    #[test]
    fn test_get_single_selected_node_one() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);

        assert_eq!(app.get_single_selected_node(), Some(key));
    }

    #[test]
    fn test_get_single_selected_node_none() {
        let app = GraphBrowserApp::new_for_testing();
        assert_eq!(app.get_single_selected_node(), None);
    }

    #[test]
    fn test_get_single_selected_node_multi() {
        let mut app = GraphBrowserApp::new_for_testing();
        let k1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let k2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));
        app.select_node(k1, false);
        app.select_node(k2, true);

        assert_eq!(app.get_single_selected_node(), None);
    }

    // --- TEST-1: update_node_url_and_log ---

    #[test]
    fn test_update_node_url_and_log() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("old-url".to_string(), Point2D::new(0.0, 0.0));

        let old = app.update_node_url_and_log(key, "new-url".to_string());

        assert_eq!(old, Some("old-url".to_string()));
        assert_eq!(app.graph.get_node(key).unwrap().url, "new-url");
        // url_to_node should be updated
        assert!(app.graph.get_node_by_url("new-url").is_some());
        assert!(app.graph.get_node_by_url("old-url").is_none());
    }

    #[test]
    fn test_update_node_url_nonexistent() {
        let mut app = GraphBrowserApp::new_for_testing();
        let fake_key = NodeKey::new(999);

        assert_eq!(app.update_node_url_and_log(fake_key, "x".to_string()), None);
    }

    #[test]
    fn test_new_from_dir_recovers_logged_graph() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            let id_a = Uuid::new_v4();
            let id_b = Uuid::new_v4();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_a.to_string(),
                url: "https://a.com".to_string(),
                position_x: 10.0,
                position_y: 20.0,
            });
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_b.to_string(),
                url: "https://b.com".to_string(),
                position_x: 30.0,
                position_y: 40.0,
            });
            store.log_mutation(&LogEntry::AddEdge {
                from_node_id: id_a.to_string(),
                to_node_id: id_b.to_string(),
                edge_type: PersistedEdgeType::Hyperlink,
            });
        }

        let app = GraphBrowserApp::new_from_dir(path);
        assert!(app.has_recovered_graph());
        assert_eq!(app.graph.node_count(), 2);
        assert_eq!(app.graph.edge_count(), 1);
        assert!(app.graph.get_node_by_url("https://a.com").is_some());
        assert!(app.graph.get_node_by_url("https://b.com").is_some());
    }

    #[test]
    fn test_new_from_dir_scans_placeholder_ids_from_recovery() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            let id = Uuid::new_v4();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id.to_string(),
                url: "about:blank#5".to_string(),
                position_x: 0.0,
                position_y: 0.0,
            });
        }

        let mut app = GraphBrowserApp::new_from_dir(path);
        let key = app.create_new_node_near_center();
        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.url, "about:blank#6");
    }

    #[test]
    fn test_clear_graph_and_persistence_in_memory_reset() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);

        app.clear_graph_and_persistence();

        assert_eq!(app.graph.node_count(), 0);
        assert!(app.selected_nodes.is_empty());
    }

    #[test]
    fn test_clear_graph_and_persistence_wipes_store() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut app = GraphBrowserApp::new_from_dir(path.clone());
            app.add_node_and_sync("https://persisted.com".to_string(), Point2D::new(1.0, 2.0));
            app.take_snapshot();
            app.clear_graph_and_persistence();
        }

        let recovered = GraphBrowserApp::new_from_dir(path);
        assert!(!recovered.has_recovered_graph());
        assert_eq!(recovered.graph.node_count(), 0);
    }

    #[test]
    fn test_switch_persistence_dir_reloads_graph_state() {
        let dir_a = TempDir::new().unwrap();
        let path_a = dir_a.path().to_path_buf();
        let dir_b = TempDir::new().unwrap();
        let path_b = dir_b.path().to_path_buf();

        {
            let mut store_a = GraphStore::open(path_a.clone()).unwrap();
            store_a.log_mutation(&LogEntry::AddNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://from-a.com".to_string(),
                position_x: 1.0,
                position_y: 2.0,
            });
        }
        {
            let mut store_b = GraphStore::open(path_b.clone()).unwrap();
            store_b.log_mutation(&LogEntry::AddNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://from-b.com".to_string(),
                position_x: 3.0,
                position_y: 4.0,
            });
            store_b.log_mutation(&LogEntry::AddNode {
                node_id: Uuid::new_v4().to_string(),
                url: "about:blank#7".to_string(),
                position_x: 5.0,
                position_y: 6.0,
            });
        }

        let mut app = GraphBrowserApp::new_from_dir(path_a);
        assert!(app.graph.get_node_by_url("https://from-a.com").is_some());
        assert!(app.graph.get_node_by_url("https://from-b.com").is_none());

        app.switch_persistence_dir(path_b).unwrap();

        assert!(app.graph.get_node_by_url("https://from-a.com").is_none());
        assert!(app.graph.get_node_by_url("https://from-b.com").is_some());
        assert!(app.selected_nodes.is_empty());

        let new_placeholder = app.create_new_node_near_center();
        assert_eq!(
            app.graph.get_node(new_placeholder).unwrap().url,
            "about:blank#8"
        );
    }

    #[test]
    fn test_set_snapshot_interval_secs_updates_store() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let mut app = GraphBrowserApp::new_from_dir(path);

        app.set_snapshot_interval_secs(45).unwrap();
        assert_eq!(app.snapshot_interval_secs(), Some(45));
    }

    #[test]
    fn test_set_snapshot_interval_secs_without_persistence_fails() {
        let mut app = GraphBrowserApp::new_for_testing();
        assert!(app.set_snapshot_interval_secs(45).is_err());
        assert_eq!(app.snapshot_interval_secs(), None);
    }
}
