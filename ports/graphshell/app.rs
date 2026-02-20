/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Application state management for the graph browser.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use crate::graph::egui_adapter::EguiGraphState;
use crate::graph::{EdgeType, Graph, NodeKey};
use crate::persistence::GraphStore;
use crate::persistence::types::{LogEntry, PersistedEdgeType};
use egui_graphs::FruchtermanReingoldWithCenterGravityState;
use euclid::default::Point2D;
use log::warn;
// Platform-agnostic renderer handle.
// On desktop this aliases servo::WebViewId so existing callers in the
// desktop module work without any conversion.
// On iOS, Servo is not a dependency, so a standalone opaque type is used.
#[cfg(not(target_os = "ios"))]
use servo::WebViewId;
/// Opaque handle for a renderer instance (webview, PDF viewer, etc.).
/// On desktop: identical to `servo::WebViewId` (type alias, zero cost).
/// On iOS: an opaque counter assigned by the iOS renderer layer.
#[cfg(not(target_os = "ios"))]
pub type RendererId = WebViewId;
#[cfg(target_os = "ios")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererId(u64);
use uuid::Uuid;

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
    order: Vec<NodeKey>,
    primary: Option<NodeKey>,
    revision: u64,
}

#[derive(Debug, Clone)]
pub struct NodeCrashState {
    pub reason: String,
    pub has_backtrace: bool,
    pub crashed_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDisplayMode {
    Highlight,
    Filter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionUpdateMode {
    Replace,
    Add,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardCopyKind {
    Url,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardCopyRequest {
    pub key: NodeKey,
    pub kind: ClipboardCopyKind,
}

#[derive(Clone)]
struct UndoRedoSnapshot {
    graph: Graph,
    selected_nodes: SelectionState,
    highlighted_graph_edge: Option<(NodeKey, NodeKey)>,
    workspace_layout_json: Option<String>,
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
            if self.nodes.contains(&key) {
                self.nodes.remove(&key);
                self.order.retain(|existing| *existing != key);
                self.primary = self.order.last().copied();
                self.revision = self.revision.saturating_add(1);
            } else if self.nodes.insert(key) {
                self.order.push(key);
                self.primary = Some(key);
                self.revision = self.revision.saturating_add(1);
            }
            return;
        }

        if self.nodes.len() == 1 && self.nodes.contains(&key) && self.primary == Some(key) {
            self.nodes.clear();
            self.order.clear();
            self.primary = None;
            self.revision = self.revision.saturating_add(1);
            return;
        }

        self.nodes.clear();
        self.order.clear();
        self.nodes.insert(key);
        self.order.push(key);
        self.primary = Some(key);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn clear(&mut self) {
        if self.nodes.is_empty() && self.primary.is_none() {
            return;
        }
        self.nodes.clear();
        self.order.clear();
        self.primary = None;
        self.revision = self.revision.saturating_add(1);
    }

    pub fn update_many(&mut self, keys: Vec<NodeKey>, mode: SelectionUpdateMode) {
        match mode {
            SelectionUpdateMode::Replace => {
                self.nodes.clear();
                self.order.clear();
                for key in keys {
                    if self.nodes.insert(key) {
                        self.order.push(key);
                    }
                }
                self.primary = self.order.last().copied();
                self.revision = self.revision.saturating_add(1);
            },
            SelectionUpdateMode::Add => {
                let mut changed = false;
                for key in keys {
                    if self.nodes.insert(key) {
                        self.order.push(key);
                        self.primary = Some(key);
                        changed = true;
                    }
                }
                if changed {
                    self.revision = self.revision.saturating_add(1);
                }
            },
            SelectionUpdateMode::Toggle => {
                let mut changed = false;
                for key in keys {
                    if self.nodes.remove(&key) {
                        self.order.retain(|existing| *existing != key);
                        changed = true;
                    } else if self.nodes.insert(key) {
                        self.order.push(key);
                        self.primary = Some(key);
                        changed = true;
                    }
                }
                self.primary = self.order.last().copied();
                if changed {
                    self.revision = self.revision.saturating_add(1);
                }
            },
        }
    }

    /// Ordered pair of selected nodes when exactly two nodes are selected.
    pub fn ordered_pair(&self) -> Option<(NodeKey, NodeKey)> {
        if self.nodes.len() != 2 {
            return None;
        }
        let mut iter = self
            .order
            .iter()
            .copied()
            .filter(|key| self.nodes.contains(key));
        let first = iter.next()?;
        let second = iter.next()?;
        Some((first, second))
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
pub enum EdgeCommand {
    ConnectSelectedPair,
    ConnectPair { from: NodeKey, to: NodeKey },
    ConnectBothDirections,
    ConnectBothDirectionsPair { a: NodeKey, b: NodeKey },
    RemoveUserEdge,
    RemoveUserEdgePair { a: NodeKey, b: NodeKey },
    PinSelected,
    UnpinSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTileOpenMode {
    Tab,
    SplitHorizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingConnectedOpenScope {
    Neighbors,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardZoomRequest {
    In,
    Out,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressureLevel {
    Unknown,
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceOpenAction {
    /// Restore an existing workspace and focus the target node.
    RestoreWorkspace { name: String, node: NodeKey },
    /// No workspace membership exists: open in the current workspace context.
    OpenInCurrentWorkspace { node: NodeKey },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsavedWorkspacePromptRequest {
    WorkspaceSwitch {
        name: String,
        focus_node: Option<NodeKey>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsavedWorkspacePromptAction {
    ProceedWithoutSaving,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooseWorkspacePickerMode {
    OpenNodeInWorkspace,
    AddNodeToWorkspace,
    AddConnectedSelectionToWorkspace,
    AddExactSelectionToWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChooseWorkspacePickerRequest {
    pub node: NodeKey,
    pub mode: ChooseWorkspacePickerMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAnchorPreference {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl ToastAnchorPreference {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::TopRight => "top-right",
            Self::TopLeft => "top-left",
            Self::BottomRight => "bottom-right",
            Self::BottomLeft => "bottom-left",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "top-right" => Some(Self::TopRight),
            "top-left" => Some(Self::TopLeft),
            "bottom-right" => Some(Self::BottomRight),
            "bottom-left" => Some(Self::BottomLeft),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LassoMouseBinding {
    RightDrag,
    ShiftLeftDrag,
}

impl LassoMouseBinding {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::RightDrag => "right-drag",
            Self::ShiftLeftDrag => "shift-left-drag",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "right-drag" => Some(Self::RightDrag),
            "shift-left-drag" => Some(Self::ShiftLeftDrag),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPaletteShortcut {
    F2,
    CtrlK,
}

impl CommandPaletteShortcut {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::F2 => "f2",
            Self::CtrlK => "ctrl-k",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "f2" => Some(Self::F2),
            "ctrl-k" => Some(Self::CtrlK),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpPanelShortcut {
    F1OrQuestion,
    H,
}

impl HelpPanelShortcut {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::F1OrQuestion => "f1-or-question",
            Self::H => "h",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "f1-or-question" => Some(Self::F1OrQuestion),
            "h" => Some(Self::H),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialMenuShortcut {
    F3,
    R,
}

impl RadialMenuShortcut {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::F3 => "f3",
            Self::R => "r",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "f3" => Some(Self::F3),
            "r" => Some(Self::R),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmnibarPreferredScope {
    Auto,
    LocalTabs,
    ConnectedNodes,
    ProviderDefault,
    GlobalNodes,
    GlobalTabs,
}

impl OmnibarPreferredScope {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::LocalTabs => "local-tabs",
            Self::ConnectedNodes => "connected-nodes",
            Self::ProviderDefault => "provider-default",
            Self::GlobalNodes => "global-nodes",
            Self::GlobalTabs => "global-tabs",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "auto" => Some(Self::Auto),
            "local-tabs" => Some(Self::LocalTabs),
            "connected-nodes" => Some(Self::ConnectedNodes),
            "provider-default" => Some(Self::ProviderDefault),
            "global-nodes" => Some(Self::GlobalNodes),
            "global-tabs" => Some(Self::GlobalTabs),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmnibarNonAtOrderPreset {
    ContextualThenProviderThenGlobal,
    ProviderThenContextualThenGlobal,
}

impl OmnibarNonAtOrderPreset {
    fn as_persisted_str(self) -> &'static str {
        match self {
            Self::ContextualThenProviderThenGlobal => "contextual-provider-global",
            Self::ProviderThenContextualThenGlobal => "provider-contextual-global",
        }
    }

    fn from_persisted_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            "contextual-provider-global" => Some(Self::ContextualThenProviderThenGlobal),
            "provider-contextual-global" => Some(Self::ProviderThenContextualThenGlobal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GraphIntent {
    TogglePhysics,
    RequestFitToScreen,
    RequestZoomIn,
    RequestZoomOut,
    RequestZoomReset,
    RequestZoomToSelected,
    ReheatPhysics,
    TogglePhysicsPanel,
    ToggleHelpPanel,
    ToggleCommandPalette,
    ToggleRadialMenu,
    TogglePersistencePanel,
    Undo,
    Redo,
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
    UpdateSelection {
        keys: Vec<NodeKey>,
        mode: SelectionUpdateMode,
    },
    SelectAll,
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
    OpenNodeWorkspaceRouted {
        key: NodeKey,
        prefer_workspace: Option<String>,
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
    CreateUserGroupedEdgeFromPrimarySelection,
    ExecuteEdgeCommand {
        command: EdgeCommand,
    },
    SetHighlightedEdge {
        from: NodeKey,
        to: NodeKey,
    },
    ClearHighlightedEdge,
    SetNodePinned {
        key: NodeKey,
        is_pinned: bool,
    },
    TogglePrimaryNodePin,
    PromoteNodeToActive {
        key: NodeKey,
    },
    DemoteNodeToCold {
        key: NodeKey,
    },
    DemoteNodeToWarm {
        key: NodeKey,
    },
    MapWebviewToNode {
        webview_id: RendererId,
        key: NodeKey,
    },
    UnmapWebview {
        webview_id: RendererId,
    },
    WebViewCreated {
        parent_webview_id: RendererId,
        child_webview_id: RendererId,
        initial_url: Option<String>,
    },
    WebViewUrlChanged {
        webview_id: RendererId,
        new_url: String,
    },
    WebViewHistoryChanged {
        webview_id: RendererId,
        entries: Vec<String>,
        current: usize,
    },
    WebViewScrollChanged {
        webview_id: RendererId,
        scroll_x: f32,
        scroll_y: f32,
    },
    SetNodeFormDraft {
        key: NodeKey,
        form_draft: Option<String>,
    },
    WebViewTitleChanged {
        webview_id: RendererId,
        title: Option<String>,
    },
    WebViewCrashed {
        webview_id: RendererId,
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
    pub physics: FruchtermanReingoldWithCenterGravityState,

    /// Physics running state before user drag/pan interaction began.
    physics_running_before_interaction: Option<bool>,

    /// Currently selected nodes (can be multiple)
    pub selected_nodes: SelectionState,

    /// Bidirectional mapping between renderer instances and graph nodes
    webview_to_node: HashMap<RendererId, NodeKey>,
    node_to_webview: HashMap<NodeKey, RendererId>,
    /// Runtime-only crash metadata keyed by graph node.
    node_crash_state: HashMap<NodeKey, NodeCrashState>,

    /// Nodes that had webviews before switching to graph view (for restoration).
    /// Managed by the webview_controller module.
    pub(crate) active_webview_nodes: Vec<NodeKey>,

    /// Active mapped nodes in LRU order (oldest at index 0, newest at end).
    active_lru: Vec<NodeKey>,

    /// Maximum number of active mapped webviews to retain.
    active_webview_limit: usize,

    /// Warm-cached nodes in LRU order (oldest at index 0, newest at end).
    warm_cache_lru: Vec<NodeKey>,

    /// Maximum number of warm-cached webviews to retain.
    warm_cache_limit: usize,

    /// Counter for unique placeholder URLs (about:blank#1, about:blank#2, ...).
    /// Prevents `url_to_node` clobbering when pressing N multiple times.
    next_placeholder_id: u32,

    /// True while the user is actively interacting (drag/pan) with the graph
    pub(crate) is_interacting: bool,

    /// Short post-drag decay window to preserve "weight" when physics was paused.
    drag_release_frames_remaining: u8,

    /// Whether the physics config panel is open
    pub show_physics_panel: bool,

    /// Whether the keyboard shortcut help panel is open
    pub show_help_panel: bool,

    /// Whether the edge command palette is open
    pub show_command_palette: bool,
    /// Whether the radial command UI is open.
    pub show_radial_menu: bool,

    /// Whether the persistence hub panel is open.
    pub show_persistence_panel: bool,
    /// Preferred toast anchor location.
    pub toast_anchor_preference: ToastAnchorPreference,
    /// Preferred lasso activation gesture.
    pub lasso_mouse_binding: LassoMouseBinding,
    /// Shortcut binding for command palette.
    pub command_palette_shortcut: CommandPaletteShortcut,
    /// Shortcut binding for help panel.
    pub help_panel_shortcut: HelpPanelShortcut,
    /// Shortcut binding for radial menu.
    pub radial_menu_shortcut: RadialMenuShortcut,
    /// Preferred default non-`@` omnibar scope behavior.
    pub omnibar_preferred_scope: OmnibarPreferredScope,
    /// Non-`@` omnibar ordering preset.
    pub omnibar_non_at_order: OmnibarNonAtOrderPreset,
    /// Independent multi-selection for workspace tabs.
    pub selected_tab_nodes: HashSet<NodeKey>,
    /// Range-select anchor for workspace tab multi-selection.
    pub tab_selection_anchor: Option<NodeKey>,
    /// Scroll zoom inertia impulse scale (higher = more responsive/floaty).
    pub scroll_zoom_impulse_scale: f32,
    /// Scroll zoom inertia damping factor (lower = quicker stop).
    pub scroll_zoom_inertia_damping: f32,
    /// Minimum absolute inertia velocity before stopping.
    pub scroll_zoom_inertia_min_abs: f32,

    /// Last hovered node in graph view (updated by graph render pass).
    pub hovered_graph_node: Option<NodeKey>,
    /// Graph search display mode (context-preserving highlight vs strict filter).
    pub search_display_mode: SearchDisplayMode,
    /// Explicit node context target (e.g. right-click) for open commands.
    pending_node_context_target: Option<NodeKey>,
    /// Explicit highlighted edge in graph view (for edge-search targeting).
    pub highlighted_graph_edge: Option<(NodeKey, NodeKey)>,

    /// Pending UI command: open connected nodes for this source, tile mode, and scope.
    pending_open_connected_from: Option<(NodeKey, PendingTileOpenMode, PendingConnectedOpenScope)>,

    /// Pending UI command: open the selected/newly-created node in a tile mode.
    pending_open_selected_tile_mode: Option<PendingTileOpenMode>,

    /// Pending UI command: persist current workspace (tile tree) snapshot.
    pending_save_workspace_snapshot: bool,

    /// Pending UI command: persist named workspace snapshot.
    pending_save_workspace_snapshot_named: Option<String>,

    /// Pending UI command: restore named workspace snapshot.
    pending_restore_workspace_snapshot_named: Option<String>,

    /// One-shot node focus request applied after a routed workspace restore.
    pending_workspace_restore_focus_node: Option<NodeKey>,

    /// Pending modal prompt context for unsaved workspace transitions.
    pending_unsaved_workspace_prompt: Option<UnsavedWorkspacePromptRequest>,

    /// User decision captured from unsaved-workspace modal prompt.
    pending_unsaved_workspace_prompt_action: Option<UnsavedWorkspacePromptAction>,

    /// Node target and mode for "Choose Workspace..." picker window.
    pending_choose_workspace_picker_request: Option<ChooseWorkspacePickerRequest>,

    /// Pending UI command: add a node tab to an existing named workspace snapshot.
    pending_add_node_to_workspace: Option<(NodeKey, String)>,
    /// Pending UI command: add connected nodes (from seed selection) to a named workspace snapshot.
    pending_add_connected_to_workspace: Option<(Vec<NodeKey>, String)>,
    /// Pending exact node set used by workspace picker for explicit import.
    pending_choose_workspace_picker_exact_nodes: Option<Vec<NodeKey>>,
    /// Pending UI command: add an explicit node set to a named workspace snapshot.
    pending_add_exact_to_workspace: Option<(Vec<NodeKey>, String)>,

    /// Pending UI command: persist named full-graph snapshot.
    pending_save_graph_snapshot_named: Option<String>,

    /// Pending UI command: restore named full-graph snapshot.
    pending_restore_graph_snapshot_named: Option<String>,

    /// Pending UI command: restore autosaved latest graph snapshot/replay state.
    pending_restore_graph_snapshot_latest: bool,

    /// Pending UI command: delete named full-graph snapshot.
    pending_delete_graph_snapshot_named: Option<String>,

    /// Pending UI command: detach focused webview pane into split layout.
    pending_detach_node_to_split: Option<NodeKey>,

    /// Pending UI command: prune empty named workspaces.
    pending_prune_empty_workspaces: bool,

    /// Pending UI command: keep only latest N named workspaces.
    pending_keep_latest_named_workspaces: Option<usize>,

    /// Pending clipboard copy request for node-derived values.
    pending_clipboard_copy: Option<ClipboardCopyRequest>,

    /// Pending UI command: switch persistence data directory.
    pending_switch_data_dir: Option<PathBuf>,

    /// Pending keyboard-driven zoom command to apply against graph metadata.
    pending_keyboard_zoom_request: Option<KeyboardZoomRequest>,

    /// Pending "zoom to selected" request for graph render metadata.
    pending_zoom_to_selected_request: bool,

    /// One-shot flag: fit graph to screen on next frame (triggered by 'C' key)
    pub fit_to_screen_requested: bool,

    /// Camera state (zoom bounds)
    pub camera: Camera,

    /// Persistent graph store (fjall log + redb snapshots)
    persistence: Option<GraphStore>,

    /// Global undo history snapshots.
    undo_stack: Vec<UndoRedoSnapshot>,
    /// Global redo history snapshots.
    redo_stack: Vec<UndoRedoSnapshot>,
    /// Pending workspace layout restore emitted by undo/redo.
    pending_history_workspace_layout_json: Option<String>,

    /// Hash of last persisted session workspace layout json.
    last_session_workspace_layout_hash: Option<u64>,

    /// Minimum interval between autosaved session workspace writes.
    workspace_autosave_interval: Duration,

    /// Number of previous autosaved session workspace revisions to keep.
    workspace_autosave_retention: u8,

    /// Timestamp of last autosaved session workspace write.
    last_workspace_autosave_at: Option<Instant>,

    /// Monotonic activation counter for named workspace recency tracking.
    workspace_activation_seq: u64,

    /// Per-node most-recent named workspace activation metadata.
    node_last_active_workspace: HashMap<NodeKey, (u64, String)>,

    /// UUID-keyed workspace membership index (runtime-derived from persisted layouts).
    node_workspace_membership: HashMap<Uuid, BTreeSet<String>>,

    /// True while current tile tree was synthesized without a named restore context.
    /// Retained for routing/session bookkeeping only.
    current_workspace_is_synthesized: bool,

    /// True if graph-mutating action happened since last workspace baseline/save.
    workspace_has_unsaved_changes: bool,

    /// True after we've emitted a warning for the current unsaved workspace state.
    unsaved_workspace_prompt_warned: bool,

    /// Cached egui_graphs state (persists across frames for drag/interaction)
    pub egui_state: Option<EguiGraphState>,

    /// Flag: egui_state needs rebuild (set when graph structure changes)
    pub egui_state_dirty: bool,

    /// Last sampled runtime memory pressure classification.
    memory_pressure_level: MemoryPressureLevel,
    /// Last sampled available system memory (MiB).
    memory_available_mib: u64,
    /// Last sampled total system memory (MiB).
    memory_total_mib: u64,

    /// Whether form draft capture/replay metadata is enabled.
    form_draft_capture_enabled: bool,
}

impl GraphBrowserApp {
    pub const SESSION_WORKSPACE_LAYOUT_NAME: &'static str = "workspace:session-latest";
    const SESSION_WORKSPACE_PREV_PREFIX: &'static str = "workspace:session-prev-";
    pub const WORKSPACE_PIN_WORKSPACE_NAME: &'static str = "workspace:pin-workspace-current";
    pub const WORKSPACE_PIN_PANE_NAME: &'static str = "workspace:pin-pane-current";
    pub const SETTINGS_TOAST_ANCHOR_NAME: &'static str = "workspace:settings-toast-anchor";
    pub const SETTINGS_LASSO_MOUSE_BINDING_NAME: &'static str = "workspace:settings-lasso-binding";
    pub const SETTINGS_COMMAND_PALETTE_SHORTCUT_NAME: &'static str =
        "workspace:settings-command-palette-shortcut";
    pub const SETTINGS_HELP_PANEL_SHORTCUT_NAME: &'static str =
        "workspace:settings-help-panel-shortcut";
    pub const SETTINGS_RADIAL_MENU_SHORTCUT_NAME: &'static str =
        "workspace:settings-radial-menu-shortcut";
    pub const SETTINGS_OMNIBAR_PREFERRED_SCOPE_NAME: &'static str =
        "workspace:settings-omnibar-preferred-scope";
    pub const SETTINGS_OMNIBAR_NON_AT_ORDER_NAME: &'static str =
        "workspace:settings-omnibar-non-at-order";
    pub const SETTINGS_SCROLL_ZOOM_IMPULSE_SCALE_NAME: &'static str =
        "workspace:settings-scroll-zoom-impulse-scale";
    pub const SETTINGS_SCROLL_ZOOM_DAMPING_NAME: &'static str =
        "workspace:settings-scroll-zoom-damping";
    pub const SETTINGS_SCROLL_ZOOM_MIN_ABS_NAME: &'static str =
        "workspace:settings-scroll-zoom-min-abs";
    pub const DEFAULT_SCROLL_ZOOM_IMPULSE_SCALE: f32 = 0.01;
    pub const DEFAULT_SCROLL_ZOOM_INERTIA_DAMPING: f32 = 0.86;
    pub const DEFAULT_SCROLL_ZOOM_INERTIA_MIN_ABS: f32 = 0.00035;
    pub const MIN_SCROLL_ZOOM_IMPULSE_SCALE: f32 = 0.001;
    pub const MAX_SCROLL_ZOOM_IMPULSE_SCALE: f32 = 0.05;
    pub const MIN_SCROLL_ZOOM_INERTIA_DAMPING: f32 = 0.5;
    pub const MAX_SCROLL_ZOOM_INERTIA_DAMPING: f32 = 0.98;
    pub const MIN_SCROLL_ZOOM_INERTIA_MIN_ABS: f32 = 0.00005;
    pub const MAX_SCROLL_ZOOM_INERTIA_MIN_ABS: f32 = 0.005;
    pub const DEFAULT_WORKSPACE_AUTOSAVE_INTERVAL_SECS: u64 = 60;
    pub const DEFAULT_WORKSPACE_AUTOSAVE_RETENTION: u8 = 1;
    pub const DEFAULT_ACTIVE_WEBVIEW_LIMIT: usize = 4;
    pub const DEFAULT_WARM_CACHE_LIMIT: usize = 12;

    pub fn default_physics_state() -> FruchtermanReingoldWithCenterGravityState {
        let mut state = FruchtermanReingoldWithCenterGravityState::default();
        // Compact, less jittery default:
        // - lower repulsion and ideal distance to avoid flyaway spread
        // - higher attraction to pull distant components back together
        // - lower step magnitude for more granular, predictable motion
        state.base.c_repulse = 0.28;
        state.base.c_attract = 0.22;
        state.base.k_scale = 0.42;
        state.base.dt = 0.03;
        state.base.max_step = 3.0;
        state.base.damping = 0.55;
        // Keep the cluster attracted toward viewport center.
        state.extras.0.params.c = 0.18;
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

        let mut app = Self {
            graph,
            physics: Self::default_physics_state(),
            physics_running_before_interaction: None,
            selected_nodes: SelectionState::new(),
            webview_to_node: HashMap::new(),
            node_to_webview: HashMap::new(),
            node_crash_state: HashMap::new(),
            active_webview_nodes: Vec::new(),
            active_lru: Vec::new(),
            active_webview_limit: Self::DEFAULT_ACTIVE_WEBVIEW_LIMIT,
            warm_cache_lru: Vec::new(),
            warm_cache_limit: Self::DEFAULT_WARM_CACHE_LIMIT,
            next_placeholder_id,
            is_interacting: false,
            drag_release_frames_remaining: 0,
            show_physics_panel: false,
            show_help_panel: false,
            show_command_palette: false,
            show_radial_menu: false,
            show_persistence_panel: false,
            toast_anchor_preference: ToastAnchorPreference::BottomRight,
            lasso_mouse_binding: LassoMouseBinding::RightDrag,
            command_palette_shortcut: CommandPaletteShortcut::F2,
            help_panel_shortcut: HelpPanelShortcut::F1OrQuestion,
            radial_menu_shortcut: RadialMenuShortcut::F3,
            omnibar_preferred_scope: OmnibarPreferredScope::Auto,
            omnibar_non_at_order: OmnibarNonAtOrderPreset::ContextualThenProviderThenGlobal,
            selected_tab_nodes: HashSet::new(),
            tab_selection_anchor: None,
            scroll_zoom_impulse_scale: Self::DEFAULT_SCROLL_ZOOM_IMPULSE_SCALE,
            scroll_zoom_inertia_damping: Self::DEFAULT_SCROLL_ZOOM_INERTIA_DAMPING,
            scroll_zoom_inertia_min_abs: Self::DEFAULT_SCROLL_ZOOM_INERTIA_MIN_ABS,
            hovered_graph_node: None,
            search_display_mode: SearchDisplayMode::Highlight,
            pending_node_context_target: None,
            highlighted_graph_edge: None,
            pending_open_connected_from: None,
            pending_open_selected_tile_mode: None,
            pending_save_workspace_snapshot: false,
            pending_save_workspace_snapshot_named: None,
            pending_restore_workspace_snapshot_named: None,
            pending_workspace_restore_focus_node: None,
            pending_unsaved_workspace_prompt: None,
            pending_unsaved_workspace_prompt_action: None,
            pending_choose_workspace_picker_request: None,
            pending_add_node_to_workspace: None,
            pending_add_connected_to_workspace: None,
            pending_choose_workspace_picker_exact_nodes: None,
            pending_add_exact_to_workspace: None,
            pending_save_graph_snapshot_named: None,
            pending_restore_graph_snapshot_named: None,
            pending_restore_graph_snapshot_latest: false,
            pending_delete_graph_snapshot_named: None,
            pending_detach_node_to_split: None,
            pending_prune_empty_workspaces: false,
            pending_keep_latest_named_workspaces: None,
            pending_clipboard_copy: None,
            pending_switch_data_dir: None,
            pending_keyboard_zoom_request: None,
            pending_zoom_to_selected_request: false,
            fit_to_screen_requested: false,
            camera: Camera::new(),
            persistence,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_history_workspace_layout_json: None,
            last_session_workspace_layout_hash: None,
            workspace_autosave_interval: Duration::from_secs(
                Self::DEFAULT_WORKSPACE_AUTOSAVE_INTERVAL_SECS,
            ),
            workspace_autosave_retention: Self::DEFAULT_WORKSPACE_AUTOSAVE_RETENTION,
            last_workspace_autosave_at: None,
            workspace_activation_seq: 0,
            node_last_active_workspace: HashMap::new(),
            node_workspace_membership: HashMap::new(),
            current_workspace_is_synthesized: false,
            workspace_has_unsaved_changes: false,
            unsaved_workspace_prompt_warned: false,
            egui_state: None,
            egui_state_dirty: true,
            memory_pressure_level: MemoryPressureLevel::Unknown,
            memory_available_mib: 0,
            memory_total_mib: 0,
            form_draft_capture_enabled: std::env::var_os("GRAPHSHELL_ENABLE_FORM_DRAFT").is_some(),
        };
        app.load_persisted_ui_settings();
        app
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
            active_lru: Vec::new(),
            active_webview_limit: Self::DEFAULT_ACTIVE_WEBVIEW_LIMIT,
            warm_cache_lru: Vec::new(),
            warm_cache_limit: Self::DEFAULT_WARM_CACHE_LIMIT,
            next_placeholder_id: 0,
            is_interacting: false,
            drag_release_frames_remaining: 0,
            show_physics_panel: false,
            show_help_panel: false,
            show_command_palette: false,
            show_radial_menu: false,
            show_persistence_panel: false,
            toast_anchor_preference: ToastAnchorPreference::BottomRight,
            lasso_mouse_binding: LassoMouseBinding::RightDrag,
            command_palette_shortcut: CommandPaletteShortcut::F2,
            help_panel_shortcut: HelpPanelShortcut::F1OrQuestion,
            radial_menu_shortcut: RadialMenuShortcut::F3,
            omnibar_preferred_scope: OmnibarPreferredScope::Auto,
            omnibar_non_at_order: OmnibarNonAtOrderPreset::ContextualThenProviderThenGlobal,
            selected_tab_nodes: HashSet::new(),
            tab_selection_anchor: None,
            scroll_zoom_impulse_scale: Self::DEFAULT_SCROLL_ZOOM_IMPULSE_SCALE,
            scroll_zoom_inertia_damping: Self::DEFAULT_SCROLL_ZOOM_INERTIA_DAMPING,
            scroll_zoom_inertia_min_abs: Self::DEFAULT_SCROLL_ZOOM_INERTIA_MIN_ABS,
            hovered_graph_node: None,
            search_display_mode: SearchDisplayMode::Highlight,
            pending_node_context_target: None,
            highlighted_graph_edge: None,
            pending_open_connected_from: None,
            pending_open_selected_tile_mode: None,
            pending_save_workspace_snapshot: false,
            pending_save_workspace_snapshot_named: None,
            pending_restore_workspace_snapshot_named: None,
            pending_workspace_restore_focus_node: None,
            pending_unsaved_workspace_prompt: None,
            pending_unsaved_workspace_prompt_action: None,
            pending_choose_workspace_picker_request: None,
            pending_add_node_to_workspace: None,
            pending_add_connected_to_workspace: None,
            pending_choose_workspace_picker_exact_nodes: None,
            pending_add_exact_to_workspace: None,
            pending_save_graph_snapshot_named: None,
            pending_restore_graph_snapshot_named: None,
            pending_restore_graph_snapshot_latest: false,
            pending_delete_graph_snapshot_named: None,
            pending_detach_node_to_split: None,
            pending_prune_empty_workspaces: false,
            pending_keep_latest_named_workspaces: None,
            pending_clipboard_copy: None,
            pending_switch_data_dir: None,
            pending_keyboard_zoom_request: None,
            pending_zoom_to_selected_request: false,
            fit_to_screen_requested: false,
            camera: Camera::new(),
            persistence: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_history_workspace_layout_json: None,
            last_session_workspace_layout_hash: None,
            workspace_autosave_interval: Duration::from_secs(
                Self::DEFAULT_WORKSPACE_AUTOSAVE_INTERVAL_SECS,
            ),
            workspace_autosave_retention: Self::DEFAULT_WORKSPACE_AUTOSAVE_RETENTION,
            last_workspace_autosave_at: None,
            workspace_activation_seq: 0,
            node_last_active_workspace: HashMap::new(),
            node_workspace_membership: HashMap::new(),
            current_workspace_is_synthesized: false,
            workspace_has_unsaved_changes: false,
            unsaved_workspace_prompt_warned: false,
            egui_state: None,
            egui_state_dirty: true,
            memory_pressure_level: MemoryPressureLevel::Unknown,
            memory_available_mib: 0,
            memory_total_mib: 0,
            form_draft_capture_enabled: false,
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

    pub fn set_tab_selection_single(&mut self, key: NodeKey) {
        if self.graph.get_node(key).is_none() {
            return;
        }
        self.selected_tab_nodes.clear();
        self.selected_tab_nodes.insert(key);
        self.tab_selection_anchor = Some(key);
    }

    pub fn toggle_tab_selection(&mut self, key: NodeKey) {
        if self.graph.get_node(key).is_none() {
            return;
        }
        if !self.selected_tab_nodes.remove(&key) {
            self.selected_tab_nodes.insert(key);
        }
        self.tab_selection_anchor = Some(key);
    }

    pub fn add_tab_selection_keys(&mut self, keys: impl IntoIterator<Item = NodeKey>) {
        let mut last = None;
        for key in keys {
            if self.graph.get_node(key).is_none() {
                continue;
            }
            self.selected_tab_nodes.insert(key);
            last = Some(key);
        }
        if let Some(key) = last {
            self.tab_selection_anchor = Some(key);
        }
    }

    /// Request fit-to-screen on next render frame (one-shot)
    pub fn request_fit_to_screen(&mut self) {
        self.fit_to_screen_requested = true;
    }

    /// Consume one pending keyboard zoom request.
    pub fn take_pending_keyboard_zoom_request(&mut self) -> Option<KeyboardZoomRequest> {
        self.pending_keyboard_zoom_request.take()
    }

    /// Consume pending zoom-to-selected request.
    pub fn take_pending_zoom_to_selected_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_zoom_to_selected_request)
    }

    /// Set whether the user is actively interacting with the graph
    pub fn set_interacting(&mut self, interacting: bool) {
        if self.is_interacting == interacting {
            return;
        }
        self.is_interacting = interacting;

        if interacting {
            self.physics_running_before_interaction = Some(self.physics.base.is_running);
            self.physics.base.is_running = false;
            self.drag_release_frames_remaining = 0;
        } else if let Some(was_running) = self.physics_running_before_interaction.take() {
            if was_running {
                self.physics.base.is_running = true;
                self.drag_release_frames_remaining = 0;
            } else {
                self.physics.base.is_running = true;
                self.drag_release_frames_remaining = 10;
            }
        }
    }

    /// Advance frame-local physics housekeeping.
    /// Handles short post-drag inertia decay when simulation was previously paused.
    pub fn tick_frame(&mut self) {
        if self.drag_release_frames_remaining == 0 || self.is_interacting {
            return;
        }
        self.drag_release_frames_remaining -= 1;
        if self.drag_release_frames_remaining == 0 {
            self.physics.base.is_running = false;
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
        if matches!(
            intent,
            GraphIntent::CreateNodeNearCenter
                | GraphIntent::CreateNodeAtUrl { .. }
                | GraphIntent::RemoveSelectedNodes
                | GraphIntent::ClearGraph
                | GraphIntent::CreateUserGroupedEdge { .. }
                | GraphIntent::CreateUserGroupedEdgeFromPrimarySelection
                | GraphIntent::RemoveEdge { .. }
                | GraphIntent::SetNodePinned { .. }
                | GraphIntent::SetNodeUrl { .. }
                | GraphIntent::ExecuteEdgeCommand { .. }
        ) {
            // Any graph mutation starts a fresh unsaved-change episode for
            // workspace-switch prompt gating.
            self.unsaved_workspace_prompt_warned = false;
            self.workspace_has_unsaved_changes = true;
        }

        match intent {
            GraphIntent::TogglePhysics => self.toggle_physics(),
            GraphIntent::RequestFitToScreen => self.request_fit_to_screen(),
            GraphIntent::RequestZoomIn => {
                self.pending_keyboard_zoom_request = Some(KeyboardZoomRequest::In);
            },
            GraphIntent::RequestZoomOut => {
                self.pending_keyboard_zoom_request = Some(KeyboardZoomRequest::Out);
            },
            GraphIntent::RequestZoomReset => {
                self.pending_keyboard_zoom_request = Some(KeyboardZoomRequest::Reset);
            },
            GraphIntent::RequestZoomToSelected => {
                // Context-aware zoom:
                // - 0 or 1 selected node: use full-graph fit.
                // - 2+ selected nodes: fit selected bounds.
                if self.selected_nodes.len() < 2 {
                    self.request_fit_to_screen();
                } else {
                    self.pending_zoom_to_selected_request = true;
                }
            },
            GraphIntent::ReheatPhysics => {
                self.physics.base.is_running = true;
                self.drag_release_frames_remaining = 0;
            },
            GraphIntent::TogglePhysicsPanel => self.toggle_physics_panel(),
            GraphIntent::ToggleHelpPanel => self.toggle_help_panel(),
            GraphIntent::ToggleCommandPalette => self.toggle_command_palette(),
            GraphIntent::ToggleRadialMenu => self.toggle_radial_menu(),
            GraphIntent::TogglePersistencePanel => self.toggle_persistence_panel(),
            GraphIntent::Undo => {
                let current_layout =
                    self.load_workspace_layout_json(Self::SESSION_WORKSPACE_LAYOUT_NAME);
                let _ = self.perform_undo(current_layout);
            },
            GraphIntent::Redo => {
                let current_layout =
                    self.load_workspace_layout_json(Self::SESSION_WORKSPACE_LAYOUT_NAME);
                let _ = self.perform_redo(current_layout);
            },
            GraphIntent::CreateNodeNearCenter => {
                self.create_new_node_near_center();
            },
            GraphIntent::CreateNodeAtUrl { url, position } => {
                let key = self.add_node_and_sync(url, position);
                self.select_node(key, false);
            },
            GraphIntent::RemoveSelectedNodes => self.remove_selected_nodes(),
            GraphIntent::ClearGraph => self.clear_graph(),
            GraphIntent::SelectNode { key, multi_select } => {
                self.select_node(key, multi_select);
                // Single-selecting an unloaded node should prewarm it (without opening a tile).
                if !multi_select
                    && self.selected_nodes.primary() == Some(key)
                    && !self.node_crash_state.contains_key(&key)
                    && self.get_webview_for_node(key).is_none()
                    && self
                        .graph
                        .get_node(key)
                        .map(|node| node.lifecycle != crate::graph::NodeLifecycle::Active)
                        .unwrap_or(false)
                {
                    self.promote_node_to_active(key);
                }
            },
            GraphIntent::UpdateSelection { keys, mode } => {
                self.selected_nodes.update_many(keys, mode);
                self.egui_state_dirty = true;
            },
            GraphIntent::SelectAll => {
                let all_keys: Vec<NodeKey> = self.graph.nodes().map(|(k, _)| k).collect();
                self.selected_nodes
                    .update_many(all_keys, SelectionUpdateMode::Replace);
                self.egui_state_dirty = true;
            },
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
            GraphIntent::OpenNodeWorkspaceRouted {
                key,
                prefer_workspace,
            } => {
                self.select_node(key, false);
                match self.resolve_workspace_open(key, prefer_workspace.as_deref()) {
                    WorkspaceOpenAction::RestoreWorkspace { name, .. } => {
                        self.pending_workspace_restore_focus_node = Some(key);
                        self.request_restore_workspace_snapshot_named(name);
                    },
                    WorkspaceOpenAction::OpenInCurrentWorkspace { .. } => {
                        self.current_workspace_is_synthesized = true;
                        self.pending_workspace_restore_focus_node = None;
                        self.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
                    },
                }
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
            GraphIntent::CreateUserGroupedEdgeFromPrimarySelection => {
                self.create_user_grouped_edge_from_primary_selection();
            },
            GraphIntent::ExecuteEdgeCommand { command } => {
                let intents = self.intents_for_edge_command(command);
                self.apply_intents(intents);
            },
            GraphIntent::SetHighlightedEdge { from, to } => {
                self.highlighted_graph_edge = Some((from, to));
            },
            GraphIntent::ClearHighlightedEdge => {
                self.highlighted_graph_edge = None;
            },
            GraphIntent::SetNodePinned { key, is_pinned } => {
                self.set_node_pinned_and_log(key, is_pinned);
            },
            GraphIntent::TogglePrimaryNodePin => {
                if let Some(key) = self.selected_nodes.primary()
                    && let Some(node) = self.graph.get_node(key)
                {
                    self.apply_intent(GraphIntent::SetNodePinned {
                        key,
                        is_pinned: !node.is_pinned,
                    });
                }
            },
            GraphIntent::PromoteNodeToActive { key } => {
                self.promote_node_to_active(key);
            },
            GraphIntent::DemoteNodeToWarm { key } => {
                self.demote_node_to_warm(key);
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
            GraphIntent::WebViewScrollChanged {
                webview_id,
                scroll_x,
                scroll_y,
            } => {
                let Some(node_key) = self.get_node_for_webview(webview_id) else {
                    return;
                };
                if let Some(node) = self.graph.get_node_mut(node_key) {
                    node.session_scroll = Some((scroll_x, scroll_y));
                }
            },
            GraphIntent::SetNodeFormDraft { key, form_draft } => {
                if !self.form_draft_capture_enabled {
                    return;
                }
                if let Some(node) = self.graph.get_node_mut(key) {
                    node.session_form_draft = form_draft;
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
            self.physics.base.is_running = true;
            self.drag_release_frames_remaining = 0;
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
            self.physics.base.is_running = true;
            self.drag_release_frames_remaining = 0;
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

    /// Persist serialized tile layout JSON under a workspace name.
    pub fn save_workspace_layout_json(&mut self, name: &str, layout_json: &str) {
        if let Some(store) = &mut self.persistence
            && let Err(e) = store.save_workspace_layout_json(name, layout_json)
        {
            warn!("Failed to save workspace layout '{name}': {e}");
        }
        if !Self::is_reserved_workspace_layout_name(name) {
            self.current_workspace_is_synthesized = false;
            self.workspace_has_unsaved_changes = false;
            self.unsaved_workspace_prompt_warned = false;
        }
    }

    fn layout_json_hash(layout_json: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        layout_json.hash(&mut hasher);
        hasher.finish()
    }

    fn session_workspace_history_key(index: u8) -> String {
        format!("{}{index}", Self::SESSION_WORKSPACE_PREV_PREFIX)
    }

    fn rotate_session_workspace_history(&mut self, latest_layout_before_overwrite: &str) {
        let retention = self.workspace_autosave_retention;
        if retention == 0 {
            return;
        }

        for idx in (1..retention).rev() {
            let from_key = Self::session_workspace_history_key(idx);
            let to_key = Self::session_workspace_history_key(idx + 1);
            if let Some(layout) = self.load_workspace_layout_json(&from_key) {
                self.save_workspace_layout_json(&to_key, &layout);
            }
        }
        let first_key = Self::session_workspace_history_key(1);
        self.save_workspace_layout_json(&first_key, latest_layout_before_overwrite);
    }

    /// Persist reserved session workspace layout only when changed.
    pub fn save_session_workspace_layout_json_if_changed(&mut self, layout_json: &str) {
        let next_hash = Self::layout_json_hash(layout_json);
        if self.last_session_workspace_layout_hash == Some(next_hash) {
            return;
        }
        if let Some(last_at) = self.last_workspace_autosave_at
            && last_at.elapsed() < self.workspace_autosave_interval
        {
            return;
        }
        let previous_latest = self.load_workspace_layout_json(Self::SESSION_WORKSPACE_LAYOUT_NAME);
        self.save_workspace_layout_json(Self::SESSION_WORKSPACE_LAYOUT_NAME, layout_json);
        if let Some(previous_latest) = previous_latest {
            self.rotate_session_workspace_history(&previous_latest);
        }
        self.last_session_workspace_layout_hash = Some(next_hash);
        self.last_workspace_autosave_at = Some(Instant::now());
    }

    /// Mark currently loaded layout as session baseline to suppress redundant writes.
    pub fn mark_session_workspace_layout_json(&mut self, layout_json: &str) {
        self.last_session_workspace_layout_hash = Some(Self::layout_json_hash(layout_json));
        self.last_workspace_autosave_at = Some(Instant::now());
    }

    /// Load serialized tile layout JSON by workspace name.
    pub fn load_workspace_layout_json(&self, name: &str) -> Option<String> {
        self.persistence
            .as_ref()
            .and_then(|store| store.load_workspace_layout_json(name))
    }

    /// List persisted workspace layout names in stable order.
    pub fn list_workspace_layout_names(&self) -> Vec<String> {
        self.persistence
            .as_ref()
            .map(|store| store.list_workspace_layout_names())
            .unwrap_or_default()
    }

    pub fn is_reserved_workspace_layout_name(name: &str) -> bool {
        name == "latest"
            || name == Self::SESSION_WORKSPACE_LAYOUT_NAME
            || name == Self::WORKSPACE_PIN_WORKSPACE_NAME
            || name == Self::WORKSPACE_PIN_PANE_NAME
            || name == Self::SETTINGS_TOAST_ANCHOR_NAME
            || name == Self::SETTINGS_LASSO_MOUSE_BINDING_NAME
            || name == Self::SETTINGS_COMMAND_PALETTE_SHORTCUT_NAME
            || name == Self::SETTINGS_HELP_PANEL_SHORTCUT_NAME
            || name == Self::SETTINGS_RADIAL_MENU_SHORTCUT_NAME
            || name == Self::SETTINGS_OMNIBAR_PREFERRED_SCOPE_NAME
            || name == Self::SETTINGS_OMNIBAR_NON_AT_ORDER_NAME
            || name == Self::SETTINGS_SCROLL_ZOOM_IMPULSE_SCALE_NAME
            || name == Self::SETTINGS_SCROLL_ZOOM_DAMPING_NAME
            || name == Self::SETTINGS_SCROLL_ZOOM_MIN_ABS_NAME
            || name.starts_with(Self::SESSION_WORKSPACE_PREV_PREFIX)
    }

    pub fn set_toast_anchor_preference(&mut self, preference: ToastAnchorPreference) {
        self.toast_anchor_preference = preference;
        self.save_toast_anchor_preference();
    }

    fn save_toast_anchor_preference(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_TOAST_ANCHOR_NAME,
            self.toast_anchor_preference.as_persisted_str(),
        );
    }

    pub fn set_lasso_mouse_binding(&mut self, binding: LassoMouseBinding) {
        self.lasso_mouse_binding = binding;
        self.save_lasso_mouse_binding();
    }

    fn save_lasso_mouse_binding(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_LASSO_MOUSE_BINDING_NAME,
            self.lasso_mouse_binding.as_persisted_str(),
        );
    }

    pub fn set_command_palette_shortcut(&mut self, shortcut: CommandPaletteShortcut) {
        self.command_palette_shortcut = shortcut;
        self.save_command_palette_shortcut();
    }

    fn save_command_palette_shortcut(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_COMMAND_PALETTE_SHORTCUT_NAME,
            self.command_palette_shortcut.as_persisted_str(),
        );
    }

    pub fn set_help_panel_shortcut(&mut self, shortcut: HelpPanelShortcut) {
        self.help_panel_shortcut = shortcut;
        self.save_help_panel_shortcut();
    }

    fn save_help_panel_shortcut(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_HELP_PANEL_SHORTCUT_NAME,
            self.help_panel_shortcut.as_persisted_str(),
        );
    }

    pub fn set_radial_menu_shortcut(&mut self, shortcut: RadialMenuShortcut) {
        self.radial_menu_shortcut = shortcut;
        self.save_radial_menu_shortcut();
    }

    fn save_radial_menu_shortcut(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_RADIAL_MENU_SHORTCUT_NAME,
            self.radial_menu_shortcut.as_persisted_str(),
        );
    }

    pub fn set_omnibar_preferred_scope(&mut self, scope: OmnibarPreferredScope) {
        self.omnibar_preferred_scope = scope;
        self.save_omnibar_preferred_scope();
    }

    fn save_omnibar_preferred_scope(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_OMNIBAR_PREFERRED_SCOPE_NAME,
            self.omnibar_preferred_scope.as_persisted_str(),
        );
    }

    pub fn set_omnibar_non_at_order(&mut self, order: OmnibarNonAtOrderPreset) {
        self.omnibar_non_at_order = order;
        self.save_omnibar_non_at_order();
    }

    fn save_omnibar_non_at_order(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_OMNIBAR_NON_AT_ORDER_NAME,
            self.omnibar_non_at_order.as_persisted_str(),
        );
    }

    pub fn set_scroll_zoom_impulse_scale(&mut self, value: f32) {
        self.scroll_zoom_impulse_scale = value.clamp(
            Self::MIN_SCROLL_ZOOM_IMPULSE_SCALE,
            Self::MAX_SCROLL_ZOOM_IMPULSE_SCALE,
        );
        self.save_scroll_zoom_impulse_scale();
    }

    fn save_scroll_zoom_impulse_scale(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_SCROLL_ZOOM_IMPULSE_SCALE_NAME,
            &self.scroll_zoom_impulse_scale.to_string(),
        );
    }

    pub fn set_scroll_zoom_inertia_damping(&mut self, value: f32) {
        self.scroll_zoom_inertia_damping = value.clamp(
            Self::MIN_SCROLL_ZOOM_INERTIA_DAMPING,
            Self::MAX_SCROLL_ZOOM_INERTIA_DAMPING,
        );
        self.save_scroll_zoom_inertia_damping();
    }

    fn save_scroll_zoom_inertia_damping(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_SCROLL_ZOOM_DAMPING_NAME,
            &self.scroll_zoom_inertia_damping.to_string(),
        );
    }

    pub fn set_scroll_zoom_inertia_min_abs(&mut self, value: f32) {
        self.scroll_zoom_inertia_min_abs = value.clamp(
            Self::MIN_SCROLL_ZOOM_INERTIA_MIN_ABS,
            Self::MAX_SCROLL_ZOOM_INERTIA_MIN_ABS,
        );
        self.save_scroll_zoom_inertia_min_abs();
    }

    fn save_scroll_zoom_inertia_min_abs(&mut self) {
        self.save_workspace_layout_json(
            Self::SETTINGS_SCROLL_ZOOM_MIN_ABS_NAME,
            &self.scroll_zoom_inertia_min_abs.to_string(),
        );
    }

    fn load_persisted_ui_settings(&mut self) {
        let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_TOAST_ANCHOR_NAME) else {
            return self.load_additional_persisted_ui_settings();
        };
        if let Some(preference) = ToastAnchorPreference::from_persisted_str(&raw) {
            self.toast_anchor_preference = preference;
        } else {
            warn!("Ignoring invalid persisted toast anchor preference: '{raw}'");
        }
        self.load_additional_persisted_ui_settings();
    }

    fn load_additional_persisted_ui_settings(&mut self) {
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_LASSO_MOUSE_BINDING_NAME)
        {
            if let Some(binding) = LassoMouseBinding::from_persisted_str(&raw) {
                self.lasso_mouse_binding = binding;
            } else {
                warn!("Ignoring invalid persisted lasso binding: '{raw}'");
            }
        }
        if let Some(raw) =
            self.load_workspace_layout_json(Self::SETTINGS_COMMAND_PALETTE_SHORTCUT_NAME)
        {
            if let Some(shortcut) = CommandPaletteShortcut::from_persisted_str(&raw) {
                self.command_palette_shortcut = shortcut;
            } else {
                warn!("Ignoring invalid persisted command-palette shortcut: '{raw}'");
            }
        }
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_HELP_PANEL_SHORTCUT_NAME)
        {
            if let Some(shortcut) = HelpPanelShortcut::from_persisted_str(&raw) {
                self.help_panel_shortcut = shortcut;
            } else {
                warn!("Ignoring invalid persisted help-panel shortcut: '{raw}'");
            }
        }
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_RADIAL_MENU_SHORTCUT_NAME)
        {
            if let Some(shortcut) = RadialMenuShortcut::from_persisted_str(&raw) {
                self.radial_menu_shortcut = shortcut;
            } else {
                warn!("Ignoring invalid persisted radial-menu shortcut: '{raw}'");
            }
        }
        if let Some(raw) =
            self.load_workspace_layout_json(Self::SETTINGS_OMNIBAR_PREFERRED_SCOPE_NAME)
        {
            if let Some(scope) = OmnibarPreferredScope::from_persisted_str(&raw) {
                self.omnibar_preferred_scope = scope;
            } else {
                warn!("Ignoring invalid persisted omnibar preferred scope: '{raw}'");
            }
        }
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_OMNIBAR_NON_AT_ORDER_NAME)
        {
            if let Some(order) = OmnibarNonAtOrderPreset::from_persisted_str(&raw) {
                self.omnibar_non_at_order = order;
            } else {
                warn!("Ignoring invalid persisted omnibar non-@ order preset: '{raw}'");
            }
        }
        if let Some(raw) =
            self.load_workspace_layout_json(Self::SETTINGS_SCROLL_ZOOM_IMPULSE_SCALE_NAME)
        {
            match raw.trim().parse::<f32>() {
                Ok(value) => {
                    self.scroll_zoom_impulse_scale = value.clamp(
                        Self::MIN_SCROLL_ZOOM_IMPULSE_SCALE,
                        Self::MAX_SCROLL_ZOOM_IMPULSE_SCALE,
                    );
                },
                Err(_) => warn!("Ignoring invalid persisted scroll zoom impulse scale: '{raw}'"),
            }
        }
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_SCROLL_ZOOM_DAMPING_NAME)
        {
            match raw.trim().parse::<f32>() {
                Ok(value) => {
                    self.scroll_zoom_inertia_damping = value.clamp(
                        Self::MIN_SCROLL_ZOOM_INERTIA_DAMPING,
                        Self::MAX_SCROLL_ZOOM_INERTIA_DAMPING,
                    );
                },
                Err(_) => warn!("Ignoring invalid persisted scroll zoom damping: '{raw}'"),
            }
        }
        if let Some(raw) = self.load_workspace_layout_json(Self::SETTINGS_SCROLL_ZOOM_MIN_ABS_NAME)
        {
            match raw.trim().parse::<f32>() {
                Ok(value) => {
                    self.scroll_zoom_inertia_min_abs = value.clamp(
                        Self::MIN_SCROLL_ZOOM_INERTIA_MIN_ABS,
                        Self::MAX_SCROLL_ZOOM_INERTIA_MIN_ABS,
                    );
                },
                Err(_) => {
                    warn!("Ignoring invalid persisted scroll zoom inertia minimum velocity: '{raw}'")
                },
            }
        }
    }

    /// Delete a persisted workspace layout by name.
    pub fn delete_workspace_layout(&mut self, name: &str) -> Result<(), String> {
        if Self::is_reserved_workspace_layout_name(name) {
            return Err(format!("Cannot delete reserved workspace '{name}'"));
        }
        self.persistence
            .as_mut()
            .ok_or_else(|| "Persistence is not enabled".to_string())?
            .delete_workspace_layout(name)
            .map_err(|e| e.to_string())?;
        self.node_last_active_workspace
            .retain(|_, (_, workspace_name)| workspace_name != name);
        for memberships in self.node_workspace_membership.values_mut() {
            memberships.remove(name);
        }
        self.node_workspace_membership
            .retain(|_, memberships| !memberships.is_empty());
        self.egui_state_dirty = true;
        Ok(())
    }

    /// Delete the reserved session workspace snapshot and reset hash baseline.
    pub fn clear_session_workspace_layout(&mut self) -> Result<(), String> {
        let mut names_to_delete = vec![Self::SESSION_WORKSPACE_LAYOUT_NAME.to_string()];
        for idx in 1..=5 {
            names_to_delete.push(Self::session_workspace_history_key(idx));
        }
        let store = self
            .persistence
            .as_mut()
            .ok_or_else(|| "Persistence is not enabled".to_string())?;
        for name in names_to_delete {
            let _ = store.delete_workspace_layout(&name);
        }
        self.last_session_workspace_layout_hash = None;
        self.last_workspace_autosave_at = None;
        Ok(())
    }

    pub fn workspace_autosave_interval_secs(&self) -> u64 {
        self.workspace_autosave_interval.as_secs()
    }

    pub fn set_workspace_autosave_interval_secs(&mut self, secs: u64) -> Result<(), String> {
        if secs == 0 {
            return Err("Workspace autosave interval must be greater than zero".to_string());
        }
        self.workspace_autosave_interval = Duration::from_secs(secs);
        Ok(())
    }

    pub fn workspace_autosave_retention(&self) -> u8 {
        self.workspace_autosave_retention
    }

    pub fn set_workspace_autosave_retention(&mut self, count: u8) -> Result<(), String> {
        if count > 5 {
            return Err("Workspace autosave retention must be between 0 and 5".to_string());
        }
        if count < self.workspace_autosave_retention
            && let Some(store) = self.persistence.as_mut()
        {
            for idx in (count + 1)..=5 {
                let _ = store.delete_workspace_layout(&Self::session_workspace_history_key(idx));
            }
        }
        self.workspace_autosave_retention = count;
        Ok(())
    }

    /// Whether the current workspace has unsaved graph changes.
    pub fn should_prompt_unsaved_workspace_save(&self) -> bool {
        self.workspace_has_unsaved_changes
    }

    /// Returns true once per unsaved-changes episode to enable one-shot warnings.
    pub fn consume_unsaved_workspace_prompt_warning(&mut self) -> bool {
        if !self.should_prompt_unsaved_workspace_save() || self.unsaved_workspace_prompt_warned {
            return false;
        }
        self.unsaved_workspace_prompt_warned = true;
        true
    }

    /// Queue/replace an unsaved-workspace prompt request.
    pub fn request_unsaved_workspace_prompt(&mut self, request: UnsavedWorkspacePromptRequest) {
        self.pending_unsaved_workspace_prompt = Some(request);
        self.pending_unsaved_workspace_prompt_action = None;
    }

    /// Inspect active unsaved-workspace prompt request.
    pub fn unsaved_workspace_prompt_request(&self) -> Option<&UnsavedWorkspacePromptRequest> {
        self.pending_unsaved_workspace_prompt.as_ref()
    }

    /// Capture user action from unsaved-workspace prompt UI.
    pub fn set_unsaved_workspace_prompt_action(&mut self, action: UnsavedWorkspacePromptAction) {
        self.pending_unsaved_workspace_prompt_action = Some(action);
    }

    /// Resolve and clear active unsaved-workspace prompt when an action was chosen.
    pub fn take_unsaved_workspace_prompt_resolution(
        &mut self,
    ) -> Option<(UnsavedWorkspacePromptRequest, UnsavedWorkspacePromptAction)> {
        let action = self.pending_unsaved_workspace_prompt_action?;
        let request = self.pending_unsaved_workspace_prompt.take()?;
        self.pending_unsaved_workspace_prompt_action = None;
        Some((request, action))
    }

    /// Mark the current workspace context as synthesized from runtime actions.
    pub fn mark_current_workspace_synthesized(&mut self) {
        self.current_workspace_is_synthesized = true;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
    }

    /// Workspace-activation recency sequence for a node (higher = more recent).
    pub fn workspace_recency_seq_for_node(&self, key: NodeKey) -> u64 {
        self.node_last_active_workspace
            .get(&key)
            .map(|(seq, _)| *seq)
            .unwrap_or(0)
    }

    /// Workspace memberships for a node sorted by recency (most recent first), then name.
    pub fn sorted_workspaces_for_node_key(&self, key: NodeKey) -> Vec<String> {
        let mut names: Vec<String> = self.workspaces_for_node_key(key).iter().cloned().collect();
        if let Some((_, recent)) = self.node_last_active_workspace.get(&key)
            && let Some(idx) = names.iter().position(|name| name == recent)
        {
            let recent = names.remove(idx);
            names.insert(0, recent);
        }
        names
    }

    /// Last activation sequence associated with a workspace name.
    pub fn workspace_recency_seq_for_name(&self, workspace_name: &str) -> u64 {
        self.node_last_active_workspace
            .values()
            .filter_map(|(seq, name)| (name == workspace_name).then_some(*seq))
            .max()
            .unwrap_or(0)
    }

    /// Mark a named workspace as activated, updating per-node recency.
    pub fn note_workspace_activated(
        &mut self,
        workspace_name: &str,
        nodes: impl IntoIterator<Item = NodeKey>,
    ) {
        self.workspace_activation_seq = self.workspace_activation_seq.saturating_add(1);
        let seq = self.workspace_activation_seq;
        let workspace_name = workspace_name.to_string();
        for key in nodes {
            let Some(node) = self.graph.get_node(key) else {
                continue;
            };
            self.node_last_active_workspace
                .insert(key, (seq, workspace_name.clone()));
            self.node_workspace_membership
                .entry(node.id)
                .or_default()
                .insert(workspace_name.clone());
        }
        self.current_workspace_is_synthesized = false;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
        self.egui_state_dirty = true;
    }

    /// Initialize membership index from desktop-layer workspace scan.
    pub fn init_membership_index(&mut self, index: HashMap<Uuid, BTreeSet<String>>) {
        self.node_workspace_membership = index;
        self.egui_state_dirty = true;
    }

    fn empty_workspace_membership() -> &'static BTreeSet<String> {
        static EMPTY: OnceLock<BTreeSet<String>> = OnceLock::new();
        EMPTY.get_or_init(BTreeSet::new)
    }

    /// Workspace membership set for a stable node UUID.
    pub fn membership_for_node(&self, uuid: Uuid) -> &BTreeSet<String> {
        self.node_workspace_membership
            .get(&uuid)
            .unwrap_or_else(|| Self::empty_workspace_membership())
    }

    /// Workspace membership set for a NodeKey in the current graph.
    pub fn workspaces_for_node_key(&self, key: NodeKey) -> &BTreeSet<String> {
        let Some(node) = self.graph.get_node(key) else {
            return Self::empty_workspace_membership();
        };
        self.membership_for_node(node.id)
    }

    /// Resolve workspace-aware node-open behavior with deterministic fallback.
    pub fn resolve_workspace_open(
        &self,
        node: NodeKey,
        prefer_workspace: Option<&str>,
    ) -> WorkspaceOpenAction {
        if self.graph.get_node(node).is_none() {
            return WorkspaceOpenAction::OpenInCurrentWorkspace { node };
        }
        let memberships = self.workspaces_for_node_key(node);

        if let Some(preferred_name) = prefer_workspace
            && memberships.contains(preferred_name)
        {
            return WorkspaceOpenAction::RestoreWorkspace {
                name: preferred_name.to_string(),
                node,
            };
        }

        if !memberships.is_empty() {
            if let Some((_, recent_workspace)) = self.node_last_active_workspace.get(&node)
                && memberships.contains(recent_workspace)
            {
                return WorkspaceOpenAction::RestoreWorkspace {
                    name: recent_workspace.clone(),
                    node,
                };
            }
            if let Some(name) = memberships.iter().next() {
                return WorkspaceOpenAction::RestoreWorkspace {
                    name: name.clone(),
                    node,
                };
            }
        }

        WorkspaceOpenAction::OpenInCurrentWorkspace { node }
    }

    /// Persist a named full-graph snapshot.
    pub fn save_named_graph_snapshot(&mut self, name: &str) -> Result<(), String> {
        self.persistence
            .as_mut()
            .ok_or_else(|| "Persistence is not enabled".to_string())?
            .save_named_graph_snapshot(name, &self.graph)
            .map_err(|e| e.to_string())
    }

    /// Load a named full-graph snapshot and reset runtime mappings.
    pub fn load_named_graph_snapshot(&mut self, name: &str) -> Result<(), String> {
        let graph = self
            .persistence
            .as_ref()
            .ok_or_else(|| "Persistence is not enabled".to_string())?
            .load_named_graph_snapshot(name)
            .ok_or_else(|| format!("Named graph snapshot '{name}' not found"))?;

        self.apply_loaded_graph(graph);
        Ok(())
    }

    /// Load a named full-graph snapshot without mutating runtime state.
    pub fn peek_named_graph_snapshot(&self, name: &str) -> Option<Graph> {
        self.persistence
            .as_ref()
            .and_then(|store| store.load_named_graph_snapshot(name))
    }

    /// Load autosaved latest graph snapshot/replay state.
    pub fn load_latest_graph_snapshot(&mut self) -> Result<(), String> {
        let graph = self
            .persistence
            .as_ref()
            .ok_or_else(|| "Persistence is not enabled".to_string())?
            .recover()
            .ok_or_else(|| "Latest graph snapshot is not available".to_string())?;

        self.apply_loaded_graph(graph);
        Ok(())
    }

    /// Load autosaved latest graph snapshot/replay state without mutating runtime state.
    pub fn peek_latest_graph_snapshot(&self) -> Option<Graph> {
        self.persistence.as_ref().and_then(|store| store.recover())
    }

    /// Whether an autosaved latest graph snapshot/replay state can be restored.
    pub fn has_latest_graph_snapshot(&self) -> bool {
        self.persistence
            .as_ref()
            .and_then(|store| store.recover())
            .is_some()
    }

    fn apply_loaded_graph(&mut self, graph: Graph) {
        self.graph = graph;
        self.selected_nodes.clear();
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.active_lru.clear();
        self.warm_cache_lru.clear();
        self.node_crash_state.clear();
        self.active_webview_nodes.clear();
        self.pending_node_context_target = None;
        self.pending_workspace_restore_focus_node = None;
        self.pending_unsaved_workspace_prompt = None;
        self.pending_unsaved_workspace_prompt_action = None;
        self.pending_choose_workspace_picker_request = None;
        self.pending_add_node_to_workspace = None;
        self.pending_add_connected_to_workspace = None;
        self.pending_choose_workspace_picker_exact_nodes = None;
        self.pending_add_exact_to_workspace = None;
        self.pending_prune_empty_workspaces = false;
        self.pending_keep_latest_named_workspaces = None;
        self.pending_keyboard_zoom_request = None;
        self.pending_zoom_to_selected_request = false;
        self.node_workspace_membership.clear();
        self.current_workspace_is_synthesized = false;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
        self.next_placeholder_id = Self::scan_max_placeholder_id(&self.graph);
        self.egui_state = None;
        self.egui_state_dirty = true;
        self.fit_to_screen_requested = true;
    }

    /// List named full-graph snapshots.
    pub fn list_named_graph_snapshot_names(&self) -> Vec<String> {
        self.persistence
            .as_ref()
            .map(|store| store.list_named_graph_snapshot_names())
            .unwrap_or_default()
    }

    /// Delete a named full-graph snapshot.
    pub fn delete_named_graph_snapshot(&mut self, name: &str) -> Result<(), String> {
        self.persistence
            .as_mut()
            .ok_or_else(|| "Persistence is not enabled".to_string())?
            .delete_named_graph_snapshot(name)
            .map_err(|e| e.to_string())
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
        self.active_lru.clear();
        self.warm_cache_lru.clear();
        self.node_crash_state.clear();
        self.active_webview_nodes.clear();
        self.pending_node_context_target = None;
        self.pending_workspace_restore_focus_node = None;
        self.pending_unsaved_workspace_prompt = None;
        self.pending_unsaved_workspace_prompt_action = None;
        self.pending_choose_workspace_picker_request = None;
        self.pending_add_node_to_workspace = None;
        self.pending_add_connected_to_workspace = None;
        self.pending_choose_workspace_picker_exact_nodes = None;
        self.pending_add_exact_to_workspace = None;
        self.pending_prune_empty_workspaces = false;
        self.pending_keep_latest_named_workspaces = None;
        self.pending_keyboard_zoom_request = None;
        self.pending_zoom_to_selected_request = false;
        self.next_placeholder_id = next_placeholder_id;
        self.egui_state = None;
        self.egui_state_dirty = true;
        self.last_session_workspace_layout_hash = None;
        self.last_workspace_autosave_at = None;
        self.workspace_activation_seq = 0;
        self.node_last_active_workspace.clear();
        self.node_workspace_membership.clear();
        self.current_workspace_is_synthesized = false;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
        self.is_interacting = false;
        self.physics_running_before_interaction = None;
        self.toast_anchor_preference = ToastAnchorPreference::BottomRight;
        self.lasso_mouse_binding = LassoMouseBinding::RightDrag;
        self.command_palette_shortcut = CommandPaletteShortcut::F2;
        self.help_panel_shortcut = HelpPanelShortcut::F1OrQuestion;
        self.radial_menu_shortcut = RadialMenuShortcut::F3;
        self.omnibar_preferred_scope = OmnibarPreferredScope::Auto;
        self.omnibar_non_at_order = OmnibarNonAtOrderPreset::ContextualThenProviderThenGlobal;
        self.selected_tab_nodes.clear();
        self.tab_selection_anchor = None;
        self.scroll_zoom_impulse_scale = Self::DEFAULT_SCROLL_ZOOM_IMPULSE_SCALE;
        self.scroll_zoom_inertia_damping = Self::DEFAULT_SCROLL_ZOOM_INERTIA_DAMPING;
        self.scroll_zoom_inertia_min_abs = Self::DEFAULT_SCROLL_ZOOM_INERTIA_MIN_ABS;
        self.load_persisted_ui_settings();
        Ok(())
    }

    /// Add a bidirectional mapping between a renderer instance and a node
    pub fn map_webview_to_node(&mut self, webview_id: RendererId, node_key: NodeKey) {
        if let Some(previous_node) = self.webview_to_node.remove(&webview_id) {
            self.node_to_webview.remove(&previous_node);
            self.remove_active_node(previous_node);
            self.remove_warm_cache_node(previous_node);
        }
        if let Some(previous_webview_id) = self.node_to_webview.remove(&node_key) {
            self.webview_to_node.remove(&previous_webview_id);
        }
        self.webview_to_node.insert(webview_id, node_key);
        self.node_to_webview.insert(node_key, webview_id);
        self.touch_active_node(node_key);
        self.remove_warm_cache_node(node_key);
    }

    /// Remove the mapping for a renderer instance and its corresponding node
    pub fn unmap_webview(&mut self, webview_id: RendererId) -> Option<NodeKey> {
        if let Some(node_key) = self.webview_to_node.remove(&webview_id) {
            self.node_to_webview.remove(&node_key);
            self.remove_active_node(node_key);
            self.remove_warm_cache_node(node_key);
            Some(node_key)
        } else {
            None
        }
    }

    /// Get the node key for a given renderer instance
    pub fn get_node_for_webview(&self, webview_id: RendererId) -> Option<NodeKey> {
        self.webview_to_node.get(&webview_id).copied()
    }

    pub fn get_node_crash_state(&self, node_key: NodeKey) -> Option<&NodeCrashState> {
        self.node_crash_state.get(&node_key)
    }

    pub fn crashed_node_keys(&self) -> impl Iterator<Item = NodeKey> + '_ {
        self.node_crash_state.keys().copied()
    }

    /// Get the renderer ID for a given node
    pub fn get_webview_for_node(&self, node_key: NodeKey) -> Option<RendererId> {
        self.node_to_webview.get(&node_key).copied()
    }

    /// Get all renderer-node mappings as an iterator
    pub fn webview_node_mappings(&self) -> impl Iterator<Item = (RendererId, NodeKey)> + '_ {
        self.webview_to_node.iter().map(|(&wv, &nk)| (wv, nk))
    }

    /// Toggle force-directed layout simulation.
    pub fn toggle_physics(&mut self) {
        if self.is_interacting {
            let next = !self
                .physics_running_before_interaction
                .unwrap_or(self.physics.base.is_running);
            self.physics_running_before_interaction = Some(next);
            self.drag_release_frames_remaining = 0;
            return;
        }
        self.physics.base.is_running = !self.physics.base.is_running;
        self.drag_release_frames_remaining = 0;
    }

    /// Update force-directed layout configuration.
    pub fn update_physics_config(&mut self, config: FruchtermanReingoldWithCenterGravityState) {
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

    /// Toggle edge command palette visibility.
    pub fn toggle_command_palette(&mut self) {
        self.show_command_palette = !self.show_command_palette;
    }

    /// Toggle radial command menu visibility.
    pub fn toggle_radial_menu(&mut self) {
        self.show_radial_menu = !self.show_radial_menu;
        if !self.show_radial_menu {
            self.pending_node_context_target = None;
        }
    }

    /// Toggle persistence hub visibility.
    pub fn toggle_persistence_panel(&mut self) {
        self.show_persistence_panel = !self.show_persistence_panel;
    }

    /// Capture current global state as an undo checkpoint.
    pub fn capture_undo_checkpoint(&mut self, workspace_layout_json: Option<String>) {
        self.undo_stack.push(UndoRedoSnapshot {
            graph: self.graph.clone(),
            selected_nodes: self.selected_nodes.clone(),
            highlighted_graph_edge: self.highlighted_graph_edge,
            workspace_layout_json,
        });
        self.redo_stack.clear();
        const MAX_UNDO_STEPS: usize = 128;
        if self.undo_stack.len() > MAX_UNDO_STEPS {
            let excess = self.undo_stack.len() - MAX_UNDO_STEPS;
            self.undo_stack.drain(0..excess);
        }
    }

    /// Perform one global undo step using current workspace layout as redo checkpoint.
    pub fn perform_undo(&mut self, current_workspace_layout_json: Option<String>) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(UndoRedoSnapshot {
            graph: self.graph.clone(),
            selected_nodes: self.selected_nodes.clone(),
            highlighted_graph_edge: self.highlighted_graph_edge,
            workspace_layout_json: current_workspace_layout_json,
        });
        self.apply_loaded_graph(prev.graph);
        self.selected_nodes = prev.selected_nodes;
        self.highlighted_graph_edge = prev.highlighted_graph_edge;
        self.pending_history_workspace_layout_json = prev.workspace_layout_json;
        true
    }

    /// Perform one global redo step using current workspace layout as undo checkpoint.
    pub fn perform_redo(&mut self, current_workspace_layout_json: Option<String>) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(UndoRedoSnapshot {
            graph: self.graph.clone(),
            selected_nodes: self.selected_nodes.clone(),
            highlighted_graph_edge: self.highlighted_graph_edge,
            workspace_layout_json: current_workspace_layout_json,
        });
        self.apply_loaded_graph(next.graph);
        self.selected_nodes = next.selected_nodes;
        self.highlighted_graph_edge = next.highlighted_graph_edge;
        self.pending_history_workspace_layout_json = next.workspace_layout_json;
        true
    }

    /// Take pending workspace layout restore emitted by undo/redo.
    pub fn take_pending_history_workspace_layout_json(&mut self) -> Option<String> {
        self.pending_history_workspace_layout_json.take()
    }

    /// Current explicit node context target for command-surface actions.
    pub fn pending_node_context_target(&self) -> Option<NodeKey> {
        self.pending_node_context_target
    }

    /// Set/clear explicit node context target for command-surface actions.
    pub fn set_pending_node_context_target(&mut self, target: Option<NodeKey>) {
        self.pending_node_context_target = target;
    }

    /// Request opening the "Choose Workspace..." picker for a node and mode.
    pub fn request_choose_workspace_picker_for_mode(
        &mut self,
        key: NodeKey,
        mode: ChooseWorkspacePickerMode,
    ) {
        self.pending_choose_workspace_picker_request =
            Some(ChooseWorkspacePickerRequest { node: key, mode });
    }

    /// Request opening the "Choose Workspace..." picker to open a node in a workspace.
    pub fn request_choose_workspace_picker(&mut self, key: NodeKey) {
        self.request_choose_workspace_picker_for_mode(
            key,
            ChooseWorkspacePickerMode::OpenNodeInWorkspace,
        );
    }

    /// Request opening the "Choose Workspace..." picker to add node tab membership.
    pub fn request_add_node_to_workspace_picker(&mut self, key: NodeKey) {
        self.request_choose_workspace_picker_for_mode(
            key,
            ChooseWorkspacePickerMode::AddNodeToWorkspace,
        );
    }

    /// Request opening the "Choose Workspace..." picker to add connected nodes.
    pub fn request_add_connected_to_workspace_picker(&mut self, key: NodeKey) {
        self.request_choose_workspace_picker_for_mode(
            key,
            ChooseWorkspacePickerMode::AddConnectedSelectionToWorkspace,
        );
    }

    /// Request opening the "Choose Workspace..." picker to add an exact node set.
    pub fn request_add_exact_selection_to_workspace_picker(&mut self, mut keys: Vec<NodeKey>) {
        keys.retain(|key| self.graph.get_node(*key).is_some());
        keys.sort_by_key(|key| key.index());
        keys.dedup();
        let Some(anchor) = keys.first().copied() else {
            return;
        };
        self.pending_choose_workspace_picker_exact_nodes = Some(keys);
        self.request_choose_workspace_picker_for_mode(
            anchor,
            ChooseWorkspacePickerMode::AddExactSelectionToWorkspace,
        );
    }

    /// Active request for "Choose Workspace..." picker.
    pub fn choose_workspace_picker_request(&self) -> Option<ChooseWorkspacePickerRequest> {
        self.pending_choose_workspace_picker_request
    }

    /// Close "Choose Workspace..." picker.
    pub fn clear_choose_workspace_picker(&mut self) {
        self.pending_choose_workspace_picker_request = None;
        self.pending_choose_workspace_picker_exact_nodes = None;
    }

    /// Request adding `node` to named workspace snapshot `workspace_name`.
    pub fn request_add_node_to_workspace(
        &mut self,
        node: NodeKey,
        workspace_name: impl Into<String>,
    ) {
        self.pending_add_node_to_workspace = Some((node, workspace_name.into()));
    }

    /// Take and clear pending add-node-to-workspace request.
    pub fn take_pending_add_node_to_workspace(&mut self) -> Option<(NodeKey, String)> {
        self.pending_add_node_to_workspace.take()
    }

    /// Request adding nodes connected to `seed_nodes` into named workspace snapshot `workspace_name`.
    pub fn request_add_connected_to_workspace(
        &mut self,
        seed_nodes: Vec<NodeKey>,
        workspace_name: impl Into<String>,
    ) {
        self.pending_add_connected_to_workspace = Some((seed_nodes, workspace_name.into()));
    }

    /// Take and clear pending add-connected-to-workspace request.
    pub fn take_pending_add_connected_to_workspace(&mut self) -> Option<(Vec<NodeKey>, String)> {
        self.pending_add_connected_to_workspace.take()
    }

    /// Current explicit node set associated with active choose-workspace picker flow.
    pub fn choose_workspace_picker_exact_nodes(&self) -> Option<&[NodeKey]> {
        self.pending_choose_workspace_picker_exact_nodes.as_deref()
    }

    /// Request adding an exact node set into named workspace snapshot `workspace_name`.
    pub fn request_add_exact_nodes_to_workspace(
        &mut self,
        nodes: Vec<NodeKey>,
        workspace_name: impl Into<String>,
    ) {
        self.pending_add_exact_to_workspace = Some((nodes, workspace_name.into()));
    }

    /// Take and clear pending exact-add-to-workspace request.
    pub fn take_pending_add_exact_to_workspace(&mut self) -> Option<(Vec<NodeKey>, String)> {
        self.pending_add_exact_to_workspace.take()
    }

    /// Request opening connected nodes for a given source node, tile mode, and scope.
    pub fn request_open_connected_from(
        &mut self,
        source: NodeKey,
        mode: PendingTileOpenMode,
        scope: PendingConnectedOpenScope,
    ) {
        self.pending_open_connected_from = Some((source, mode, scope));
    }

    /// Take and clear pending connected-open request.
    pub fn take_pending_open_connected_from(
        &mut self,
    ) -> Option<(NodeKey, PendingTileOpenMode, PendingConnectedOpenScope)> {
        self.pending_open_connected_from.take()
    }

    /// Request opening the selected/new node as a tile in the given mode.
    pub fn request_open_selected_tile_mode(&mut self, mode: PendingTileOpenMode) {
        self.pending_open_selected_tile_mode = Some(mode);
    }

    /// Take and clear pending selected-tile open-mode request.
    pub fn take_pending_open_selected_tile_mode(&mut self) -> Option<PendingTileOpenMode> {
        self.pending_open_selected_tile_mode.take()
    }

    /// Request saving current workspace (tile layout) snapshot.
    pub fn request_save_workspace_snapshot(&mut self) {
        self.pending_save_workspace_snapshot = true;
    }

    /// Take and clear pending workspace save request.
    pub fn take_pending_save_workspace_snapshot(&mut self) -> bool {
        std::mem::take(&mut self.pending_save_workspace_snapshot)
    }

    /// Request saving a named workspace snapshot.
    pub fn request_save_workspace_snapshot_named(&mut self, name: impl Into<String>) {
        self.pending_save_workspace_snapshot_named = Some(name.into());
    }

    /// Take and clear pending named workspace save request.
    pub fn take_pending_save_workspace_snapshot_named(&mut self) -> Option<String> {
        self.pending_save_workspace_snapshot_named.take()
    }

    /// Request restoring a named workspace snapshot.
    pub fn request_restore_workspace_snapshot_named(&mut self, name: impl Into<String>) {
        self.pending_restore_workspace_snapshot_named = Some(name.into());
    }

    /// Take and clear pending named workspace restore request.
    pub fn take_pending_restore_workspace_snapshot_named(&mut self) -> Option<String> {
        self.pending_restore_workspace_snapshot_named.take()
    }

    /// Take and clear one-shot focus target for routed workspace restore.
    pub fn take_pending_workspace_restore_focus_node(&mut self) -> Option<NodeKey> {
        self.pending_workspace_restore_focus_node.take()
    }

    /// Request saving a named graph snapshot.
    pub fn request_save_graph_snapshot_named(&mut self, name: impl Into<String>) {
        self.pending_save_graph_snapshot_named = Some(name.into());
    }

    /// Take and clear pending named graph save request.
    pub fn take_pending_save_graph_snapshot_named(&mut self) -> Option<String> {
        self.pending_save_graph_snapshot_named.take()
    }

    /// Request restoring a named graph snapshot.
    pub fn request_restore_graph_snapshot_named(&mut self, name: impl Into<String>) {
        self.pending_restore_graph_snapshot_named = Some(name.into());
    }

    /// Take and clear pending named graph restore request.
    pub fn take_pending_restore_graph_snapshot_named(&mut self) -> Option<String> {
        self.pending_restore_graph_snapshot_named.take()
    }

    /// Request restoring autosaved latest graph snapshot/replay state.
    pub fn request_restore_graph_snapshot_latest(&mut self) {
        self.pending_restore_graph_snapshot_latest = true;
    }

    /// Take and clear pending autosaved graph restore request.
    pub fn take_pending_restore_graph_snapshot_latest(&mut self) -> bool {
        std::mem::take(&mut self.pending_restore_graph_snapshot_latest)
    }

    /// Request deleting a named graph snapshot.
    pub fn request_delete_graph_snapshot_named(&mut self, name: impl Into<String>) {
        self.pending_delete_graph_snapshot_named = Some(name.into());
    }

    /// Take and clear pending named graph delete request.
    pub fn take_pending_delete_graph_snapshot_named(&mut self) -> Option<String> {
        self.pending_delete_graph_snapshot_named.take()
    }

    /// Request detaching a node's pane into split layout.
    pub fn request_detach_node_to_split(&mut self, key: NodeKey) {
        self.pending_detach_node_to_split = Some(key);
    }

    /// Take and clear pending detach-to-split request.
    pub fn take_pending_detach_node_to_split(&mut self) -> Option<NodeKey> {
        self.pending_detach_node_to_split.take()
    }

    /// Request batch prune of empty named workspaces.
    pub fn request_prune_empty_workspaces(&mut self) {
        self.pending_prune_empty_workspaces = true;
    }

    /// Take pending empty-workspace prune request.
    pub fn take_pending_prune_empty_workspaces(&mut self) -> bool {
        std::mem::take(&mut self.pending_prune_empty_workspaces)
    }

    /// Request keeping latest N named workspaces.
    pub fn request_keep_latest_named_workspaces(&mut self, keep: usize) {
        self.pending_keep_latest_named_workspaces = Some(keep);
    }

    /// Take pending keep-latest-N named workspaces request.
    pub fn take_pending_keep_latest_named_workspaces(&mut self) -> Option<usize> {
        self.pending_keep_latest_named_workspaces.take()
    }

    pub fn request_copy_node_url(&mut self, key: NodeKey) {
        self.pending_clipboard_copy = Some(ClipboardCopyRequest {
            key,
            kind: ClipboardCopyKind::Url,
        });
    }

    pub fn request_copy_node_title(&mut self, key: NodeKey) {
        self.pending_clipboard_copy = Some(ClipboardCopyRequest {
            key,
            kind: ClipboardCopyKind::Title,
        });
    }

    pub fn take_pending_clipboard_copy(&mut self) -> Option<ClipboardCopyRequest> {
        self.pending_clipboard_copy.take()
    }

    pub fn request_switch_data_dir(&mut self, path: impl AsRef<Path>) {
        self.pending_switch_data_dir = Some(path.as_ref().to_path_buf());
    }

    pub fn take_pending_switch_data_dir(&mut self) -> Option<PathBuf> {
        self.pending_switch_data_dir.take()
    }

    /// Promote a node to Active lifecycle (mark as needing webview)
    pub fn promote_node_to_active(&mut self, node_key: NodeKey) {
        use crate::graph::NodeLifecycle;
        if let Some(node) = self.graph.get_node_mut(node_key) {
            node.lifecycle = NodeLifecycle::Active;
        }
        self.touch_active_node(node_key);
        self.remove_warm_cache_node(node_key);
        self.node_crash_state.remove(&node_key);
    }

    /// Demote a node to Warm lifecycle (keep mapped webview alive in cache).
    pub fn demote_node_to_warm(&mut self, node_key: NodeKey) {
        use crate::graph::NodeLifecycle;
        let has_mapped_webview = self.node_to_webview.contains_key(&node_key);
        if let Some(node) = self.graph.get_node_mut(node_key)
            && has_mapped_webview
        {
            node.lifecycle = NodeLifecycle::Warm;
            self.touch_warm_cache_node(node_key);
        } else {
            self.remove_warm_cache_node(node_key);
        }
        self.remove_active_node(node_key);
    }

    /// Demote a node to Cold lifecycle (mark as not needing webview)
    pub fn demote_node_to_cold(&mut self, node_key: NodeKey) {
        use crate::graph::NodeLifecycle;
        if let Some(node) = self.graph.get_node_mut(node_key) {
            node.lifecycle = NodeLifecycle::Cold;
        }
        self.remove_active_node(node_key);
        self.remove_warm_cache_node(node_key);
        // Also unmap webview association if it exists
        if let Some(webview_id) = self.node_to_webview.get(&node_key).copied() {
            self.webview_to_node.remove(&webview_id);
            self.node_to_webview.remove(&node_key);
        }
    }

    fn touch_active_node(&mut self, node_key: NodeKey) {
        self.remove_active_node(node_key);
        self.active_lru.push(node_key);
    }

    fn remove_active_node(&mut self, node_key: NodeKey) {
        self.active_lru.retain(|key| *key != node_key);
    }

    fn touch_warm_cache_node(&mut self, node_key: NodeKey) {
        self.remove_warm_cache_node(node_key);
        self.warm_cache_lru.push(node_key);
    }

    fn remove_warm_cache_node(&mut self, node_key: NodeKey) {
        self.warm_cache_lru.retain(|key| *key != node_key);
    }

    /// Return least-recently-used warm nodes that must be hard-evicted.
    pub(crate) fn take_warm_cache_evictions(&mut self) -> Vec<NodeKey> {
        let mut normalized = Vec::with_capacity(self.warm_cache_lru.len());
        for key in self.warm_cache_lru.drain(..) {
            let keep = self
                .graph
                .get_node(key)
                .map(|node| node.lifecycle == crate::graph::NodeLifecycle::Warm)
                .unwrap_or(false)
                && self.node_to_webview.contains_key(&key)
                && !normalized.contains(&key);
            if keep {
                normalized.push(key);
            }
        }
        self.warm_cache_lru = normalized;

        let mut evicted = Vec::new();
        while self.warm_cache_lru.len() > self.warm_cache_limit {
            evicted.push(self.warm_cache_lru.remove(0));
        }
        evicted
    }

    /// Return least-recently-used active nodes that should be demoted.
    pub(crate) fn take_active_webview_evictions(
        &mut self,
        protected: &HashSet<NodeKey>,
    ) -> Vec<NodeKey> {
        self.take_active_webview_evictions_with_limit(self.active_webview_limit, protected)
    }

    /// Return least-recently-used active nodes that exceed `limit`.
    pub(crate) fn take_active_webview_evictions_with_limit(
        &mut self,
        limit: usize,
        protected: &HashSet<NodeKey>,
    ) -> Vec<NodeKey> {
        let mut normalized = Vec::with_capacity(self.active_lru.len());
        for key in self.active_lru.drain(..) {
            let keep = self
                .graph
                .get_node(key)
                .map(|node| node.lifecycle == crate::graph::NodeLifecycle::Active)
                .unwrap_or(false)
                && self.node_to_webview.contains_key(&key)
                && !normalized.contains(&key);
            if keep {
                normalized.push(key);
            }
        }

        // Backfill any mapped-active nodes not seen in LRU (defensive against stale state).
        for (&key, _) in &self.node_to_webview {
            let is_active = self
                .graph
                .get_node(key)
                .map(|node| node.lifecycle == crate::graph::NodeLifecycle::Active)
                .unwrap_or(false);
            if is_active && !normalized.contains(&key) {
                normalized.push(key);
            }
        }
        self.active_lru = normalized;

        let mut evicted = Vec::new();
        while self.active_lru.len() > limit {
            let candidate_idx = self
                .active_lru
                .iter()
                .position(|key| !protected.contains(key));
            let Some(candidate_idx) = candidate_idx else {
                break;
            };
            let key = self.active_lru.remove(candidate_idx);
            evicted.push(key);
        }
        evicted
    }

    pub fn active_webview_limit(&self) -> usize {
        self.active_webview_limit
    }

    pub fn warm_cache_limit(&self) -> usize {
        self.warm_cache_limit
    }

    pub fn lifecycle_counts(&self) -> (usize, usize, usize) {
        let mut active = 0usize;
        let mut warm = 0usize;
        let mut cold = 0usize;
        for (_, node) in self.graph.nodes() {
            match node.lifecycle {
                crate::graph::NodeLifecycle::Active => active += 1,
                crate::graph::NodeLifecycle::Warm => warm += 1,
                crate::graph::NodeLifecycle::Cold => cold += 1,
            }
        }
        (active, warm, cold)
    }

    pub fn mapped_webview_count(&self) -> usize {
        self.node_to_webview.len()
    }

    pub fn memory_pressure_level(&self) -> MemoryPressureLevel {
        self.memory_pressure_level
    }

    #[cfg(test)]
    fn set_form_draft_capture_enabled_for_testing(&mut self, enabled: bool) {
        self.form_draft_capture_enabled = enabled;
    }

    pub fn memory_available_mib(&self) -> u64 {
        self.memory_available_mib
    }

    pub fn memory_total_mib(&self) -> u64 {
        self.memory_total_mib
    }

    pub(crate) fn set_memory_pressure_status(
        &mut self,
        level: MemoryPressureLevel,
        available_mib: u64,
        total_mib: u64,
    ) {
        self.memory_pressure_level = level;
        self.memory_available_mib = available_mib;
        self.memory_total_mib = total_mib;
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

    fn create_user_grouped_edge_from_primary_selection(&mut self) {
        let Some(from) = self.selected_nodes.primary() else {
            return;
        };
        let to = self.selected_nodes.iter().copied().find(|key| *key != from);
        if let Some(to) = to {
            self.add_user_grouped_edge_if_missing(from, to);
        }
    }

    fn selected_pair_in_order(&self) -> Option<(NodeKey, NodeKey)> {
        self.selected_nodes.ordered_pair()
    }

    fn intents_for_edge_command(&self, command: EdgeCommand) -> Vec<GraphIntent> {
        match command {
            EdgeCommand::ConnectSelectedPair => self
                .selected_pair_in_order()
                .map(|(from, to)| vec![GraphIntent::CreateUserGroupedEdge { from, to }])
                .unwrap_or_default(),
            EdgeCommand::ConnectPair { from, to } => {
                vec![GraphIntent::CreateUserGroupedEdge { from, to }]
            },
            EdgeCommand::ConnectBothDirections => self
                .selected_pair_in_order()
                .map(|(from, to)| {
                    vec![
                        GraphIntent::CreateUserGroupedEdge { from, to },
                        GraphIntent::CreateUserGroupedEdge { from: to, to: from },
                    ]
                })
                .unwrap_or_default(),
            EdgeCommand::ConnectBothDirectionsPair { a, b } => {
                vec![
                    GraphIntent::CreateUserGroupedEdge { from: a, to: b },
                    GraphIntent::CreateUserGroupedEdge { from: b, to: a },
                ]
            },
            EdgeCommand::RemoveUserEdge => self
                .selected_pair_in_order()
                .map(|(from, to)| {
                    vec![
                        GraphIntent::RemoveEdge {
                            from,
                            to,
                            edge_type: EdgeType::UserGrouped,
                        },
                        GraphIntent::RemoveEdge {
                            from: to,
                            to: from,
                            edge_type: EdgeType::UserGrouped,
                        },
                    ]
                })
                .unwrap_or_default(),
            EdgeCommand::RemoveUserEdgePair { a, b } => {
                vec![
                    GraphIntent::RemoveEdge {
                        from: a,
                        to: b,
                        edge_type: EdgeType::UserGrouped,
                    },
                    GraphIntent::RemoveEdge {
                        from: b,
                        to: a,
                        edge_type: EdgeType::UserGrouped,
                    },
                ]
            },
            EdgeCommand::PinSelected => self
                .selected_nodes
                .iter()
                .copied()
                .map(|key| GraphIntent::SetNodePinned {
                    key,
                    is_pinned: true,
                })
                .collect(),
            EdgeCommand::UnpinSelected => self
                .selected_nodes
                .iter()
                .copied()
                .map(|key| GraphIntent::SetNodePinned {
                    key,
                    is_pinned: false,
                })
                .collect(),
        }
    }

    fn set_node_pinned_and_log(&mut self, key: NodeKey, is_pinned: bool) {
        let Some(node) = self.graph.get_node_mut(key) else {
            return;
        };
        if node.is_pinned == is_pinned {
            return;
        }
        node.is_pinned = is_pinned;
        self.egui_state_dirty = true;
        if let Some(store) = &mut self.persistence {
            store.log_mutation(&LogEntry::PinNode {
                node_id: node.id.to_string(),
                is_pinned,
            });
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
            let node_id = self.graph.get_node(node_key).map(|node| node.id);

            // Log removal to persistence before removing from graph
            if let Some(store) = &mut self.persistence {
                if let Some(node_id) = node_id {
                    store.log_mutation(&LogEntry::RemoveNode {
                        node_id: node_id.to_string(),
                    });
                }
            }

            // Unmap webview if it exists
            if let Some(webview_id) = self.node_to_webview.get(&node_key).copied() {
                let _ = self.unmap_webview(webview_id);
            }
            self.remove_active_node(node_key);
            self.remove_warm_cache_node(node_key);
            self.node_crash_state.remove(&node_key);
            self.node_last_active_workspace.remove(&node_key);
            if let Some(node_id) = node_id {
                self.node_workspace_membership.remove(&node_id);
            }

            // Remove from graph
            self.graph.remove_node(node_key);
            self.egui_state_dirty = true;
        }

        // Clear selection
        self.selected_nodes.clear();
        self.highlighted_graph_edge = None;
        self.pending_node_context_target = self
            .pending_node_context_target
            .filter(|key| self.graph.get_node(*key).is_some());
        self.pending_choose_workspace_picker_request = self
            .pending_choose_workspace_picker_request
            .filter(|req| self.graph.get_node(req.node).is_some());
        self.pending_choose_workspace_picker_exact_nodes = self
            .pending_choose_workspace_picker_exact_nodes
            .take()
            .map(|keys| {
                keys.into_iter()
                    .filter(|key| self.graph.get_node(*key).is_some())
                    .collect::<Vec<_>>()
            })
            .filter(|keys| !keys.is_empty());
        self.pending_add_node_to_workspace = self
            .pending_add_node_to_workspace
            .take()
            .filter(|(key, _)| self.graph.get_node(*key).is_some());
        self.pending_add_connected_to_workspace = self
            .pending_add_connected_to_workspace
            .take()
            .map(|(keys, name)| {
                (
                    keys.into_iter()
                        .filter(|key| self.graph.get_node(*key).is_some())
                        .collect::<Vec<_>>(),
                    name,
                )
            })
            .filter(|(keys, _)| !keys.is_empty());
        self.pending_add_exact_to_workspace = self
            .pending_add_exact_to_workspace
            .take()
            .map(|(keys, name)| {
                (
                    keys.into_iter()
                        .filter(|key| self.graph.get_node(*key).is_some())
                        .collect::<Vec<_>>(),
                    name,
                )
            })
            .filter(|(keys, _)| !keys.is_empty());
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
        self.highlighted_graph_edge = None;
        self.pending_node_context_target = None;
        self.pending_choose_workspace_picker_request = None;
        self.pending_add_node_to_workspace = None;
        self.pending_add_connected_to_workspace = None;
        self.pending_choose_workspace_picker_exact_nodes = None;
        self.pending_add_exact_to_workspace = None;
        self.pending_unsaved_workspace_prompt = None;
        self.pending_unsaved_workspace_prompt_action = None;
        self.pending_prune_empty_workspaces = false;
        self.pending_keep_latest_named_workspaces = None;
        self.pending_keyboard_zoom_request = None;
        self.pending_zoom_to_selected_request = false;
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.active_lru.clear();
        self.warm_cache_lru.clear();
        self.node_crash_state.clear();
        self.node_last_active_workspace.clear();
        self.node_workspace_membership.clear();
        self.current_workspace_is_synthesized = false;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
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
        self.highlighted_graph_edge = None;
        self.pending_node_context_target = None;
        self.pending_choose_workspace_picker_request = None;
        self.pending_add_node_to_workspace = None;
        self.pending_add_connected_to_workspace = None;
        self.pending_choose_workspace_picker_exact_nodes = None;
        self.pending_add_exact_to_workspace = None;
        self.pending_unsaved_workspace_prompt = None;
        self.pending_unsaved_workspace_prompt_action = None;
        self.pending_prune_empty_workspaces = false;
        self.pending_keep_latest_named_workspaces = None;
        self.pending_keyboard_zoom_request = None;
        self.pending_zoom_to_selected_request = false;
        self.webview_to_node.clear();
        self.node_to_webview.clear();
        self.active_lru.clear();
        self.warm_cache_lru.clear();
        self.node_crash_state.clear();
        self.node_last_active_workspace.clear();
        self.node_workspace_membership.clear();
        self.current_workspace_is_synthesized = false;
        self.workspace_has_unsaved_changes = false;
        self.unsaved_workspace_prompt_warned = false;
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

    /// Create a unique RendererId for testing.
    fn test_webview_id() -> RendererId {
        #[cfg(not(target_os = "ios"))]
        {
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
        #[cfg(target_os = "ios")]
        {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            RendererId(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
        }
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
    fn test_zoom_intents_queue_keyboard_zoom_requests() {
        let mut app = GraphBrowserApp::new_for_testing();

        app.apply_intents([GraphIntent::RequestZoomIn]);
        assert_eq!(
            app.take_pending_keyboard_zoom_request(),
            Some(KeyboardZoomRequest::In)
        );
        assert_eq!(app.take_pending_keyboard_zoom_request(), None);

        app.apply_intents([GraphIntent::RequestZoomOut]);
        assert_eq!(
            app.take_pending_keyboard_zoom_request(),
            Some(KeyboardZoomRequest::Out)
        );

        app.apply_intents([GraphIntent::RequestZoomReset]);
        assert_eq!(
            app.take_pending_keyboard_zoom_request(),
            Some(KeyboardZoomRequest::Reset)
        );
    }

    #[test]
    fn test_zoom_to_selected_falls_back_to_fit_when_selection_empty() {
        let mut app = GraphBrowserApp::new_for_testing();
        assert!(app.selected_nodes.is_empty());
        assert!(!app.fit_to_screen_requested);

        app.apply_intents([GraphIntent::RequestZoomToSelected]);

        assert!(app.fit_to_screen_requested);
        assert!(!app.take_pending_zoom_to_selected_request());
    }

    #[test]
    fn test_zoom_to_selected_falls_back_to_fit_when_single_selected() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("test".to_string(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);
        assert!(!app.fit_to_screen_requested);

        app.apply_intents([GraphIntent::RequestZoomToSelected]);

        assert!(app.fit_to_screen_requested);
        assert!(!app.take_pending_zoom_to_selected_request());
    }

    #[test]
    fn test_zoom_to_selected_sets_pending_when_multi_selected() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key_a = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key_b = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 50.0));
        app.select_node(key_a, false);
        app.select_node(key_b, true);
        assert_eq!(app.selected_nodes.len(), 2);
        assert!(!app.fit_to_screen_requested);

        app.apply_intents([GraphIntent::RequestZoomToSelected]);

        assert!(app.take_pending_zoom_to_selected_request());
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
    fn test_select_node_single_click_selected_toggles_off() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("test".to_string(), Point2D::new(0.0, 0.0));

        app.select_node(key, false);
        assert_eq!(app.selected_nodes.primary(), Some(key));

        app.select_node(key, false);
        assert!(app.selected_nodes.is_empty());
        assert_eq!(app.selected_nodes.primary(), None);
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
    fn test_select_node_multi_click_selected_toggles_off() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key2 = app
            .graph
            .add_node("b".to_string(), Point2D::new(100.0, 0.0));

        app.select_node(key1, false);
        app.select_node(key2, true);
        assert_eq!(app.selected_nodes.len(), 2);
        assert_eq!(app.selected_nodes.primary(), Some(key2));

        // Ctrl-click selected node toggles it off.
        app.select_node(key2, true);
        assert_eq!(app.selected_nodes.len(), 1);
        assert!(app.selected_nodes.contains(&key1));
        assert!(!app.selected_nodes.contains(&key2));
        assert_eq!(app.selected_nodes.primary(), Some(key1));
    }

    #[test]
    fn test_select_node_multi_click_only_selected_clears_selection() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));

        app.select_node(key, false);
        assert_eq!(app.selected_nodes.primary(), Some(key));

        // Ctrl-click selected single node toggles it off, clearing selection.
        app.select_node(key, true);
        assert!(app.selected_nodes.is_empty());
        assert_eq!(app.selected_nodes.primary(), None);
    }

    #[test]
    fn test_select_node_intent_single_prewarms_cold_node() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        );

        app.apply_intents([GraphIntent::SelectNode {
            key,
            multi_select: false,
        }]);

        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        );
    }

    #[test]
    fn test_select_node_intent_toggle_off_does_not_prewarm() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));

        app.apply_intents([GraphIntent::SelectNode {
            key,
            multi_select: false,
        }]);
        app.demote_node_to_cold(key);
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        );

        // Clicking the already-selected node toggles it off and should not re-promote.
        app.apply_intents([GraphIntent::SelectNode {
            key,
            multi_select: false,
        }]);

        assert!(app.selected_nodes.is_empty());
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        );
    }

    #[test]
    fn test_select_node_intent_multiselect_does_not_prewarm_cold_node() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key1 = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key2 = app.graph.add_node("b".to_string(), Point2D::new(10.0, 0.0));

        app.apply_intents([GraphIntent::SelectNode {
            key: key1,
            multi_select: false,
        }]);
        app.demote_node_to_cold(key1);
        assert_eq!(
            app.graph.get_node(key1).unwrap().lifecycle,
            NodeLifecycle::Cold
        );
        assert_eq!(
            app.graph.get_node(key2).unwrap().lifecycle,
            NodeLifecycle::Cold
        );

        app.apply_intents([GraphIntent::SelectNode {
            key: key2,
            multi_select: true,
        }]);

        assert_eq!(
            app.graph.get_node(key2).unwrap().lifecycle,
            NodeLifecycle::Cold
        );
    }

    #[test]
    fn test_select_node_intent_does_not_prewarm_crashed_node() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let webview_id = test_webview_id();
        app.map_webview_to_node(webview_id, key);
        app.apply_intents([GraphIntent::WebViewCrashed {
            webview_id,
            reason: "boom".to_string(),
            has_backtrace: false,
        }]);
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        );
        assert!(app.get_node_crash_state(key).is_some());

        app.apply_intents([GraphIntent::SelectNode {
            key,
            multi_select: false,
        }]);

        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Cold
        );
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
        assert!(rev2 > rev1);
        assert!(app.selected_nodes.is_empty());

        app.select_node(key2, true);
        let rev3 = app.selected_nodes.revision();
        assert!(rev3 > rev2);

        app.select_node(key2, true);
        let rev4 = app.selected_nodes.revision();
        assert!(rev4 > rev3);
    }

    #[test]
    fn test_update_selection_replace_sets_exact_members() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("a".to_string(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("b".to_string(), Point2D::new(10.0, 0.0));
        let c = app.add_node_and_sync("c".to_string(), Point2D::new(20.0, 0.0));
        app.select_node(a, false);

        app.apply_intents([GraphIntent::UpdateSelection {
            keys: vec![b, c],
            mode: SelectionUpdateMode::Replace,
        }]);

        assert_eq!(app.selected_nodes.len(), 2);
        assert!(!app.selected_nodes.contains(&a));
        assert!(app.selected_nodes.contains(&b));
        assert!(app.selected_nodes.contains(&c));
        assert_eq!(app.selected_nodes.primary(), Some(c));
    }

    #[test]
    fn test_update_selection_add_and_toggle() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("a".to_string(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("b".to_string(), Point2D::new(10.0, 0.0));
        app.apply_intents([GraphIntent::UpdateSelection {
            keys: vec![a],
            mode: SelectionUpdateMode::Replace,
        }]);
        app.apply_intents([GraphIntent::UpdateSelection {
            keys: vec![b],
            mode: SelectionUpdateMode::Add,
        }]);
        assert!(app.selected_nodes.contains(&a));
        assert!(app.selected_nodes.contains(&b));
        assert_eq!(app.selected_nodes.primary(), Some(b));

        app.apply_intents([GraphIntent::UpdateSelection {
            keys: vec![a],
            mode: SelectionUpdateMode::Toggle,
        }]);
        assert!(!app.selected_nodes.contains(&a));
        assert!(app.selected_nodes.contains(&b));
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
    fn test_intent_webview_scroll_changed_updates_node_session_scroll() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let wv = test_webview_id();
        app.map_webview_to_node(wv, key);

        app.apply_intents([GraphIntent::WebViewScrollChanged {
            webview_id: wv,
            scroll_x: 15.0,
            scroll_y: 320.0,
        }]);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.session_scroll, Some((15.0, 320.0)));
    }

    #[test]
    fn test_form_draft_restore_feature_flag_guarded() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));

        app.set_form_draft_capture_enabled_for_testing(false);
        app.apply_intents([GraphIntent::SetNodeFormDraft {
            key,
            form_draft: Some("draft text".to_string()),
        }]);
        assert_eq!(app.graph.get_node(key).unwrap().session_form_draft, None);

        app.set_form_draft_capture_enabled_for_testing(true);
        app.apply_intents([GraphIntent::SetNodeFormDraft {
            key,
            form_draft: Some("draft text".to_string()),
        }]);
        assert_eq!(
            app.graph.get_node(key).unwrap().session_form_draft,
            Some("draft text".to_string())
        );
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
    fn test_intent_create_user_grouped_edge_from_primary_selection() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let b = app
            .graph
            .add_node("https://b.com".into(), Point2D::new(10.0, 0.0));

        app.select_node(b, false);
        app.select_node(a, true);

        app.apply_intents([GraphIntent::CreateUserGroupedEdgeFromPrimarySelection]);

        let count = app
            .graph
            .edges()
            .filter(|e| e.edge_type == EdgeType::UserGrouped && e.from == a && e.to == b)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_intent_create_user_grouped_edge_from_primary_selection_noop_for_single_select() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        app.select_node(a, false);

        app.apply_intents([GraphIntent::CreateUserGroupedEdgeFromPrimarySelection]);

        let count = app
            .graph
            .edges()
            .filter(|e| e.edge_type == EdgeType::UserGrouped)
            .count();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_execute_edge_command_connect_selected_pair() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://to.com".into(), Point2D::new(10.0, 0.0));

        app.select_node(from, false);
        app.select_node(to, true);
        app.physics.base.is_running = false;

        app.apply_intents([GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::ConnectSelectedPair,
        }]);

        assert!(
            app.graph
                .edges()
                .any(|e| e.edge_type == EdgeType::UserGrouped && e.from == from && e.to == to)
        );
        assert!(app.physics.base.is_running);
    }

    #[test]
    fn test_selection_ordered_pair_uses_first_selected_as_source() {
        let mut app = GraphBrowserApp::new_for_testing();
        let first = app
            .graph
            .add_node("https://first.com".into(), Point2D::new(0.0, 0.0));
        let second = app
            .graph
            .add_node("https://second.com".into(), Point2D::new(10.0, 0.0));

        app.select_node(first, false);
        app.select_node(second, true);

        assert_eq!(app.selected_nodes.ordered_pair(), Some((first, second)));
    }

    #[test]
    fn test_execute_edge_command_remove_user_edge_removes_both_directions() {
        let mut app = GraphBrowserApp::new_for_testing();
        let from = app
            .graph
            .add_node("https://from.com".into(), Point2D::new(0.0, 0.0));
        let to = app
            .graph
            .add_node("https://to.com".into(), Point2D::new(10.0, 0.0));

        app.add_user_grouped_edge_if_missing(from, to);
        app.add_user_grouped_edge_if_missing(to, from);
        app.select_node(from, false);
        app.select_node(to, true);
        app.physics.base.is_running = false;

        app.apply_intents([GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::RemoveUserEdge,
        }]);

        assert!(
            !app.graph
                .edges()
                .any(|e| e.edge_type == EdgeType::UserGrouped)
        );
        assert!(app.physics.base.is_running);
    }

    #[test]
    fn test_execute_edge_command_pin_and_unpin_selected() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://pin.com".into(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);

        app.apply_intents([GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::PinSelected,
        }]);
        assert!(app.graph.get_node(key).is_some_and(|node| node.is_pinned));

        app.apply_intents([GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::UnpinSelected,
        }]);
        assert!(app.graph.get_node(key).is_some_and(|node| !node.is_pinned));
    }

    #[test]
    fn test_reheat_physics_intent_enables_simulation() {
        let mut app = GraphBrowserApp::new_for_testing();
        app.physics.base.is_running = false;
        app.drag_release_frames_remaining = 5;

        app.apply_intents([GraphIntent::ReheatPhysics]);

        assert!(app.physics.base.is_running);
        assert_eq!(app.drag_release_frames_remaining, 0);
    }

    #[test]
    fn test_toggle_primary_node_pin_toggles_selected_primary() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://pin.com".into(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);

        app.apply_intents([GraphIntent::TogglePrimaryNodePin]);
        assert!(app.graph.get_node(key).is_some_and(|node| node.is_pinned));

        app.apply_intents([GraphIntent::TogglePrimaryNodePin]);
        assert!(app.graph.get_node(key).is_some_and(|node| !node.is_pinned));
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

        // No selection â€” should be a no-op
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
        app.demote_node_to_warm(k1);
        assert_eq!(app.warm_cache_lru, vec![k1]);

        app.clear_graph();

        assert_eq!(app.graph.node_count(), 0);
        assert!(app.selected_nodes.is_empty());
        assert!(app.get_node_for_webview(fake_wv_id).is_none());
        assert!(app.warm_cache_lru.is_empty());
        assert!(!app.workspace_has_unsaved_changes);
        assert!(!app.should_prompt_unsaved_workspace_save());
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
    fn test_demote_to_warm_requires_mapping_and_sets_lifecycle() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        app.promote_node_to_active(key);

        app.demote_node_to_warm(key);
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        );
        assert!(app.warm_cache_lru.is_empty());

        let wv_id = test_webview_id();
        app.map_webview_to_node(wv_id, key);
        app.demote_node_to_warm(key);
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Warm
        );
        assert_eq!(app.warm_cache_lru, vec![key]);
    }

    #[test]
    fn test_promote_to_active_removes_warm_cache_membership() {
        use crate::graph::NodeLifecycle;

        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();
        app.map_webview_to_node(wv_id, key);
        app.demote_node_to_warm(key);

        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Warm
        );
        assert_eq!(app.warm_cache_lru, vec![key]);

        app.promote_node_to_active(key);
        assert_eq!(
            app.graph.get_node(key).unwrap().lifecycle,
            NodeLifecycle::Active
        );
        assert!(app.warm_cache_lru.is_empty());
    }

    #[test]
    fn test_unmap_webview_removes_warm_cache_membership() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let wv_id = test_webview_id();
        app.map_webview_to_node(wv_id, key);
        app.demote_node_to_warm(key);
        assert_eq!(app.warm_cache_lru, vec![key]);

        let _ = app.unmap_webview(wv_id);
        assert!(app.warm_cache_lru.is_empty());
    }

    #[test]
    fn test_take_warm_cache_evictions_respects_lru_and_limit() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key_a = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key_b = app.graph.add_node("b".to_string(), Point2D::new(1.0, 0.0));
        let key_c = app.graph.add_node("c".to_string(), Point2D::new(2.0, 0.0));

        app.map_webview_to_node(test_webview_id(), key_a);
        app.demote_node_to_warm(key_a);
        app.map_webview_to_node(test_webview_id(), key_b);
        app.demote_node_to_warm(key_b);
        app.map_webview_to_node(test_webview_id(), key_c);
        app.demote_node_to_warm(key_c);

        assert_eq!(app.warm_cache_lru, vec![key_a, key_b, key_c]);

        app.warm_cache_limit = 2;
        let evicted = app.take_warm_cache_evictions();
        assert_eq!(evicted, vec![key_a]);
        assert_eq!(app.warm_cache_lru, vec![key_b, key_c]);
    }

    #[test]
    fn test_take_active_webview_evictions_respects_limit_and_protection() {
        use std::collections::HashSet;

        let mut app = GraphBrowserApp::new_for_testing();
        let key_a = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key_b = app.graph.add_node("b".to_string(), Point2D::new(1.0, 0.0));
        let key_c = app.graph.add_node("c".to_string(), Point2D::new(2.0, 0.0));
        let key_d = app.graph.add_node("d".to_string(), Point2D::new(3.0, 0.0));

        for key in [key_a, key_b, key_c, key_d] {
            app.promote_node_to_active(key);
            app.map_webview_to_node(test_webview_id(), key);
        }

        app.active_webview_limit = 3;
        let protected = HashSet::from([key_a]);
        let evicted = app.take_active_webview_evictions(&protected);

        assert_eq!(evicted.len(), 1);
        assert!(!protected.contains(&evicted[0]));
    }

    #[test]
    fn test_take_active_webview_evictions_with_lower_limit() {
        use std::collections::HashSet;

        let mut app = GraphBrowserApp::new_for_testing();
        let key_a = app.graph.add_node("a".to_string(), Point2D::new(0.0, 0.0));
        let key_b = app.graph.add_node("b".to_string(), Point2D::new(1.0, 0.0));
        let key_c = app.graph.add_node("c".to_string(), Point2D::new(2.0, 0.0));

        for key in [key_a, key_b, key_c] {
            app.promote_node_to_active(key);
            app.map_webview_to_node(test_webview_id(), key);
        }

        let evicted = app.take_active_webview_evictions_with_limit(1, &HashSet::new());
        assert_eq!(evicted.len(), 2);
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
    fn test_resolve_workspace_open_prefers_recent_membership() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;

        let mut index = HashMap::new();
        index.insert(
            node_id,
            BTreeSet::from(["alpha".to_string(), "beta".to_string()]),
        );
        app.init_membership_index(index);
        app.note_workspace_activated("beta", [key]);

        assert_eq!(
            app.resolve_workspace_open(key, None),
            WorkspaceOpenAction::RestoreWorkspace {
                name: "beta".to_string(),
                node: key
            }
        );
    }

    #[test]
    fn test_resolve_workspace_open_honors_preferred_workspace() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;

        let mut index = HashMap::new();
        index.insert(
            node_id,
            BTreeSet::from(["alpha".to_string(), "beta".to_string()]),
        );
        app.init_membership_index(index);
        app.note_workspace_activated("beta", [key]);

        assert_eq!(
            app.resolve_workspace_open(key, Some("alpha")),
            WorkspaceOpenAction::RestoreWorkspace {
                name: "alpha".to_string(),
                node: key
            }
        );
    }

    #[test]
    fn test_resolve_workspace_open_deterministic_fallback_without_recency_match() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;

        let mut index = HashMap::new();
        index.insert(
            node_id,
            BTreeSet::from([
                "workspace-z".to_string(),
                "workspace-a".to_string(),
                "workspace-m".to_string(),
            ]),
        );
        app.init_membership_index(index);
        app.node_last_active_workspace
            .insert(key, (99, "workspace-missing".to_string()));

        for _ in 0..5 {
            assert_eq!(
                app.resolve_workspace_open(key, None),
                WorkspaceOpenAction::RestoreWorkspace {
                    name: "workspace-a".to_string(),
                    node: key
                }
            );
        }
    }

    #[test]
    fn test_open_node_workspace_routed_with_preferred_workspace_requests_restore() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;
        let mut index = HashMap::new();
        index.insert(
            node_id,
            BTreeSet::from(["alpha".to_string(), "beta".to_string()]),
        );
        app.init_membership_index(index);
        app.note_workspace_activated("beta", [key]);

        app.apply_intents([GraphIntent::OpenNodeWorkspaceRouted {
            key,
            prefer_workspace: Some("alpha".to_string()),
        }]);

        assert_eq!(
            app.take_pending_restore_workspace_snapshot_named(),
            Some("alpha".to_string())
        );
        assert_eq!(app.take_pending_workspace_restore_focus_node(), Some(key));
    }

    #[test]
    fn test_open_node_workspace_routed_falls_back_to_current_workspace_for_zero_membership() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));

        app.apply_intents([GraphIntent::OpenNodeWorkspaceRouted {
            key,
            prefer_workspace: None,
        }]);

        assert_eq!(app.get_single_selected_node(), Some(key));
        assert_eq!(
            app.take_pending_open_selected_tile_mode(),
            Some(PendingTileOpenMode::Tab)
        );
        assert!(app.current_workspace_is_synthesized);
        assert!(
            app.take_pending_restore_workspace_snapshot_named()
                .is_none()
        );
    }

    #[test]
    fn test_open_node_workspace_routed_preserves_unsaved_prompt_state_until_restore() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;

        let mut index = HashMap::new();
        index.insert(node_id, BTreeSet::from(["workspace-alpha".to_string()]));
        app.init_membership_index(index);
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = true;
        app.unsaved_workspace_prompt_warned = false;

        app.apply_intents([GraphIntent::OpenNodeWorkspaceRouted {
            key,
            prefer_workspace: None,
        }]);

        assert_eq!(
            app.take_pending_restore_workspace_snapshot_named(),
            Some("workspace-alpha".to_string())
        );
        assert!(app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_remove_selected_nodes_clears_workspace_membership_entry() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;

        let mut index = HashMap::new();
        index.insert(node_id, BTreeSet::from(["saved-workspace".to_string()]));
        app.init_membership_index(index);

        app.select_node(key, false);
        app.remove_selected_nodes();

        assert!(app.membership_for_node(node_id).is_empty());
    }

    #[test]
    fn test_set_node_url_preserves_workspace_membership() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://before.example".to_string(), Point2D::new(0.0, 0.0));
        let node_id = app.graph.get_node(key).unwrap().id;
        let mut index = HashMap::new();
        index.insert(
            node_id,
            BTreeSet::from(["workspace-alpha".to_string(), "workspace-beta".to_string()]),
        );
        app.init_membership_index(index);

        app.apply_intents([GraphIntent::SetNodeUrl {
            key,
            new_url: "https://after.example".to_string(),
        }]);

        assert_eq!(
            app.graph.get_node(key).unwrap().url,
            "https://after.example"
        );
        assert_eq!(
            app.membership_for_node(node_id),
            &BTreeSet::from(["workspace-alpha".to_string(), "workspace-beta".to_string(),])
        );
    }

    #[test]
    fn test_workspace_has_unsaved_changes_for_graph_mutations() {
        let mut app = GraphBrowserApp::new_for_testing();
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = false;

        app.apply_intents([GraphIntent::CreateNodeNearCenter]);

        assert!(app.workspace_has_unsaved_changes);
        assert!(app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_workspace_modified_for_graph_mutations_even_when_not_synthesized() {
        let mut app = GraphBrowserApp::new_for_testing();
        app.current_workspace_is_synthesized = false;
        app.workspace_has_unsaved_changes = false;

        app.apply_intents([GraphIntent::CreateNodeNearCenter]);

        assert!(app.workspace_has_unsaved_changes);
        assert!(app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_workspace_not_modified_for_non_graph_mutations() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = false;

        app.apply_intents([GraphIntent::SelectNode {
            key,
            multi_select: false,
        }]);

        assert!(!app.workspace_has_unsaved_changes);
        assert!(!app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_unsaved_prompt_warning_resets_on_additional_graph_mutation() {
        let mut app = GraphBrowserApp::new_for_testing();
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = true;
        app.unsaved_workspace_prompt_warned = true;

        app.apply_intents([GraphIntent::CreateNodeNearCenter]);

        assert!(app.workspace_has_unsaved_changes);
        assert!(!app.unsaved_workspace_prompt_warned);
    }

    #[test]
    fn test_workspace_not_modified_for_set_node_position() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = false;

        app.apply_intents([GraphIntent::SetNodePosition {
            key,
            position: Point2D::new(42.0, 24.0),
        }]);

        assert!(!app.workspace_has_unsaved_changes);
        assert!(!app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_workspace_has_unsaved_changes_for_set_node_pinned() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".to_string(), Point2D::new(0.0, 0.0));
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = false;

        app.apply_intents([GraphIntent::SetNodePinned {
            key,
            is_pinned: true,
        }]);

        assert!(app.workspace_has_unsaved_changes);
        assert!(app.should_prompt_unsaved_workspace_save());
    }

    #[test]
    fn test_save_named_workspace_clears_unsaved_prompt_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let mut app = GraphBrowserApp::new_from_dir(path);
        app.current_workspace_is_synthesized = true;
        app.workspace_has_unsaved_changes = true;
        app.unsaved_workspace_prompt_warned = true;

        app.save_workspace_layout_json("workspace:user-saved", "{\"root\":null}");

        assert!(!app.current_workspace_is_synthesized);
        assert!(!app.workspace_has_unsaved_changes);
        assert!(!app.unsaved_workspace_prompt_warned);
        assert!(!app.should_prompt_unsaved_workspace_save());
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
    fn test_new_from_dir_loads_persisted_toast_anchor_preference() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            store
                .save_workspace_layout_json(GraphBrowserApp::SETTINGS_TOAST_ANCHOR_NAME, "top-left")
                .unwrap();
        }

        let app = GraphBrowserApp::new_from_dir(path);
        assert_eq!(app.toast_anchor_preference, ToastAnchorPreference::TopLeft);
    }

    #[test]
    fn test_set_toast_anchor_preference_persists_across_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mut app = GraphBrowserApp::new_from_dir(path.clone());
        app.set_toast_anchor_preference(ToastAnchorPreference::TopRight);
        drop(app);

        let reopened = GraphBrowserApp::new_from_dir(path);
        assert_eq!(
            reopened.toast_anchor_preference,
            ToastAnchorPreference::TopRight
        );
    }

    #[test]
    fn test_set_lasso_mouse_binding_persists_across_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mut app = GraphBrowserApp::new_from_dir(path.clone());
        app.set_lasso_mouse_binding(LassoMouseBinding::ShiftLeftDrag);
        drop(app);

        let reopened = GraphBrowserApp::new_from_dir(path);
        assert_eq!(
            reopened.lasso_mouse_binding,
            LassoMouseBinding::ShiftLeftDrag
        );
    }

    #[test]
    fn test_set_shortcut_bindings_persist_across_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mut app = GraphBrowserApp::new_from_dir(path.clone());
        app.set_command_palette_shortcut(CommandPaletteShortcut::CtrlK);
        app.set_help_panel_shortcut(HelpPanelShortcut::H);
        app.set_radial_menu_shortcut(RadialMenuShortcut::R);
        drop(app);

        let reopened = GraphBrowserApp::new_from_dir(path);
        assert_eq!(
            reopened.command_palette_shortcut,
            CommandPaletteShortcut::CtrlK
        );
        assert_eq!(reopened.help_panel_shortcut, HelpPanelShortcut::H);
        assert_eq!(reopened.radial_menu_shortcut, RadialMenuShortcut::R);
    }

    #[test]
    fn test_set_omnibar_settings_persist_across_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mut app = GraphBrowserApp::new_from_dir(path.clone());
        app.set_omnibar_preferred_scope(OmnibarPreferredScope::ProviderDefault);
        app.set_omnibar_non_at_order(OmnibarNonAtOrderPreset::ProviderThenContextualThenGlobal);
        drop(app);

        let reopened = GraphBrowserApp::new_from_dir(path);
        assert_eq!(
            reopened.omnibar_preferred_scope,
            OmnibarPreferredScope::ProviderDefault
        );
        assert_eq!(
            reopened.omnibar_non_at_order,
            OmnibarNonAtOrderPreset::ProviderThenContextualThenGlobal
        );
    }

    #[test]
    fn test_set_scroll_zoom_settings_persist_across_restart() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mut app = GraphBrowserApp::new_from_dir(path.clone());
        app.set_scroll_zoom_impulse_scale(0.014);
        app.set_scroll_zoom_inertia_damping(0.81);
        app.set_scroll_zoom_inertia_min_abs(0.00042);
        drop(app);

        let reopened = GraphBrowserApp::new_from_dir(path);
        assert!((reopened.scroll_zoom_impulse_scale - 0.014).abs() < f32::EPSILON);
        assert!((reopened.scroll_zoom_inertia_damping - 0.81).abs() < f32::EPSILON);
        assert!((reopened.scroll_zoom_inertia_min_abs - 0.00042).abs() < f32::EPSILON);
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
