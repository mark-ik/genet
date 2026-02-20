<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Workspace Routing and Membership Plan

**Date**: 2026-02-19
**Status**: Draft — post-second-critique revision

---

## Plan

### Context

Node-open behavior is currently workspace-agnostic: double-click always opens the node as a tab in
the current layout. The goal is a workspace-first routing model where opening a node predictably
returns the user to the expected workspace context, and where membership relationships (node ↔
workspaces) are first-class queryable state.

### Behavioral Rules (Invariants)

1. Opening a node never creates fanout edges or modifies the graph.
2. Routing is context-preserving: restore an existing workspace when possible.
3. Workspace generation is an explicit fallback only for zero-membership nodes.
4. Generated fallback workspaces are **unsaved** (not auto-persisted); user must save explicitly.
   - *Refinement*: If the user applies a graph-mutating action (AddNode, AddEdge, RemoveNode,
     ClearGraph) while the workspace is unsaved, set `unsaved_workspace_modified = true`.
     On the next workspace switch or session autosave, prompt to save if this flag is set.
     "Modified" is intentionally narrow: tile re-ordering and zoom do not count.
5. Deleting a workspace removes it from the membership index and recency candidates immediately.
6. The routing resolver is a single authority function — double-click, omnibar open, and radial
   commands all call the same path; no direct tile mutation bypasses.
7. If workspace restore produces an empty tile tree after pruning stale keys, the router falls
   back to opening the node as a tab in the current workspace (not an error).

---

### Phase 1: Membership Index

#### Data Model

The membership index maps **Stable Node UUID** (`Node.id`) to the set of workspace names containing
that node. `NodeKey` (petgraph `NodeIndex`) is not stable across sessions; `Node.id` is stable
and independent of URL changes (handling the "same URL, different tab" case correctly).

```text
node_workspace_membership: HashMap<Uuid, BTreeSet<WorkspaceName>>
```

This is a runtime-only structure — the source of truth is the persisted workspace layout JSONs.
The index is derived from those JSONs and maintained incrementally.

#### Build Strategy

**On startup**: The startup scan lives in the **desktop layer**, not in `app.rs`. `TileKind` and
`prune_stale_webview_tile_keys_only` are desktop-layer types; `app.rs` does not import them.

New function in `desktop/persistence_ops.rs`:

```rust
pub(crate) fn build_membership_index_from_layouts(
    persistence: &GraphPersistence,
    graph: &Graph,
) -> HashMap<Uuid, BTreeSet<String>>
```

Algorithm for each workspace:

1. Deserialize `Tree<TileKind>` from JSON.
2. Call `tile_runtime::prune_stale_webview_tile_keys_only(&mut tree, graph_app)` to get valid NodeKeys.
3. For each surviving NodeKey, look up `graph.inner.node_weight(key).unwrap().id` to get UUID.
4. Insert `(uuid, workspace_name)` into the accumulator map.

The caller (desktop startup path) then calls `graph_app.init_membership_index(map)`.

This is a one-time O(N workspaces × M nodes/workspace) scan. Acceptable at startup.

**Incremental maintenance** (these operate within `app.rs` since they have NodeKeys in hand):

- **Workspace restore** (`note_workspace_activated`): after pruning, add each surviving node's UUID
  to the index under the restored workspace name; clear `current_workspace_is_unsaved`.
- **Workspace save**: scan saved tree for NodeKeys, map to UUIDs, update index.
- **Workspace delete** (`delete_workspace_layout`): remove workspace name from all membership sets;
  drop any UUID entry whose set becomes empty. Already prunes `node_last_active_workspace`;
  same path updates the membership index.
- **Node removed** (`RemoveNode`): remove UUID entry from index entirely.
- **Note**: `SetNodeUrl` does *not* affect membership, as the UUID remains constant.

#### New App Fields and Methods

```rust
/// UUID-keyed workspace membership index (runtime-derived from persisted layouts).
node_workspace_membership: HashMap<Uuid, BTreeSet<String>>,
/// True while the current tile tree was synthesized without a named workspace save.
current_workspace_is_unsaved: bool,
/// True if a graph-mutating action occurred while the workspace was unsaved.
unsaved_workspace_modified: bool,
```

New methods on `GraphBrowserApp`:

- `fn init_membership_index(&mut self, index: HashMap<Uuid, BTreeSet<String>>)` — called from
  desktop startup and after batch prune; replaces the index wholesale.
- `fn membership_for_node(&self, uuid: Uuid) -> &BTreeSet<String>`
- `fn workspaces_for_node_key(&self, key: NodeKey) -> &BTreeSet<String>` (maps key → uuid → set)

#### Phase 1 Task List

- [x] Add `node_workspace_membership`, `current_workspace_is_unsaved`,
  `unsaved_workspace_modified` fields to `GraphBrowserApp` (app.rs).
- [x] Add `init_membership_index`, `membership_for_node`, `workspaces_for_node_key` methods to
  `GraphBrowserApp`.
- [x] Extend `note_workspace_activated` to insert surviving node UUIDs into membership index and
  clear `current_workspace_is_unsaved`.
- [x] Extend `delete_workspace_layout` to remove workspace name from all membership sets and drop
  empty UUID entries.
- [x] Handle `RemoveNode` in `apply_intent` to remove UUID entry from membership index.
- [x] New fn `build_membership_index_from_layouts(persistence, graph)` in
  `desktop/persistence_ops.rs`.
- [x] Call `build_membership_index_from_layouts` at startup (after graph is loaded but before first
  frame), call `init_membership_index` with result.

---

### Phase 2: Routing Resolver

Single authority function consumed by all open-node paths.

```rust
pub enum WorkspaceOpenAction {
    /// Restore an existing workspace and focus the node's tab.
    RestoreWorkspace { name: String, node: NodeKey },
    /// Node has no membership: open node as tab in current workspace (unsaved).
    OpenInCurrentWorkspace { node: NodeKey },
}

pub fn resolve_workspace_open(
    app: &GraphBrowserApp,
    node: NodeKey,
    prefer_workspace: Option<&str>,  // explicit choice (e.g. from "Choose Workspace...")
) -> WorkspaceOpenAction
```

Resolution order:

1. Obtain `uuid` for `node` via `app.graph.inner.node_weight(node).map(|n| n.id)`. If the node
   doesn't exist, return `OpenInCurrentWorkspace { node }`.
2. If `prefer_workspace` is `Some(name)` and name is in the membership set → `RestoreWorkspace(name)`.
3. If membership set is non-empty → pick workspace by recency. Translate uuid → NodeKey via
   `app.graph.id_to_node[&uuid]`, then look up `app.node_last_active_workspace.get(&key)` to
   get `(seq, workspace_name)`. If no recency record (node loaded from a prior session but not
   yet activated in this one), fall back to `BTreeSet::iter().next()` (stable alphabetical order).
4. If membership set is empty → `OpenInCurrentWorkspace`.

Fallback if `RestoreWorkspace` produces an empty tree after pruning:
→ fall through to `OpenInCurrentWorkspace` (log a warning, do not panic).

**Note**: `node_last_active_workspace` is currently `HashMap<NodeKey, (u64, String)>` and stores
only the *most-recently-activated workspace for that node*. This is the correct data for choosing
"which workspace does the user most associate with this node." A follow-on improvement: change the
key to `Uuid` so recency data survives session restarts and the NodeKey translation is unnecessary.

#### Wiring Double-Click

Current path: `GraphAction::FocusNode(key)` → `GraphIntent::SelectNode` → tile_behavior opens tab.

New path: `GraphAction::FocusNode(key)` → `GraphIntent::OpenNodeWorkspaceRouted { key }` →
`apply_intent` calls `resolve_workspace_open` and enqueues either
`pending_restore_workspace_snapshot_named` or `pending_open_selected_tile_mode`.

When `OpenInCurrentWorkspace` is the result, set `current_workspace_is_unsaved = true`.

`GraphAction::FocusNodeSplit` retains existing split-open behavior (not workspace-routed).

#### Phase 2 Task List

- [x] Add `GraphIntent::OpenNodeWorkspaceRouted { key: NodeKey }` variant to the `GraphIntent`
  enum in `app.rs`.
- [x] Wire the arm in `apply_intent()`: call `resolve_workspace_open`, enqueue the appropriate
  pending action, set `current_workspace_is_unsaved` as needed.
- [x] Add `WorkspaceOpenAction` enum and `resolve_workspace_open` fn (new file
  `desktop/workspace_routing.rs` or inline in `app.rs` — prefer separate file for testability).
- [x] Change `GraphAction::FocusNode` handler in `render/mod.rs` to emit
  `GraphIntent::OpenNodeWorkspaceRouted` instead of `GraphIntent::SelectNode`.

---

### Phase 3: Open Mode Commands

Three open modes surfaced in both the radial menu and the command palette:

| Mode | Behavior |
| ---- | -------- |
| Open in Workspace | Workspace-first routing (calls resolver) |
| Open with Neighbors | Synthesize unsaved workspace: node + 1-hop undirected neighbors (max 12 nodes, matching `MAX_CONNECTED_SPLIT_PANES`) |
| Open with Connected | Same as above but includes 2-hop undirected neighbors; still capped at 12 nodes |

"Open in Workspace" is the new default for double-click.
"Open with Neighbors" replaces "Open Connected as Tabs" (same semantics, renamed for clarity).

Both synthesized modes set `current_workspace_is_unsaved = true` on the resulting workspace.

#### Right-Click Detection

egui_graphs 0.29 does not emit a right-click node event. Detection path:

- After `GraphView` renders, check `ui.input(|i| i.pointer.secondary_clicked())`.
- Target node = `app.hovered_graph_node` (set from `egui_state.graph.hovered_node()` each frame).
- If both are `Some`, set `app.pending_node_context_target = Some(key)` and open context menu.

Context menu: a small `egui::popup_below_widget` or `egui::Window` containing the three open modes
plus "Choose Workspace..." (which opens a submenu/list of containing workspaces from membership index).

These commands also route through `resolve_workspace_open` and the standard intent path.

**Preferred approach**: integrate workspace open modes into the **existing radial menu** `Node`
domain (add `NodeOpenWorkspace`, `NodeOpenNeighbors`, `NodeOpenConnected` as radial commands).
This avoids a second modal surface.

#### "Choose Workspace..." Picker

Appears in both the right-click context (if implemented) and the command palette.
Rendered as a list of workspace names from `workspaces_for_node_key(key)`, sorted by recency.
Clicking a name calls `resolve_workspace_open(app, key, Some(name))`.

#### Phase 3 Task List

- [x] Add `pending_node_context_target: Option<NodeKey>` field to `GraphBrowserApp`.
- [x] Add `NodeOpenWorkspace`, `NodeOpenNeighbors`, `NodeOpenConnected` variants to `RadialCommand`
  (render/mod.rs).
- [x] Implement synthesized workspace builder (node + N-hop undirected BFS, capped at
  `MAX_CONNECTED_SPLIT_PANES`, excess truncated by `node_last_active_workspace` recency).
- [x] Wire unsaved modification tracking: in `apply_intent`, if `current_workspace_is_unsaved`
  and intent is `AddNode | AddEdge | RemoveNode | ClearGraph`, set
  `unsaved_workspace_modified = true`.
- [x] On workspace switch while `unsaved_workspace_modified`, show save-prompt modal before
  proceeding.

---

### Phase 4: Membership Visibility in Graph View

- Add node label suffix `[N]` where N = `workspaces_for_node_key(key).len()`, shown only when N > 0.
- Alternatively: a small badge rendered alongside the node shape in `GraphNodeShape::ui()`.
- Hover on badge: show tooltip listing workspace names.
- Click on badge: open the "Choose Workspace..." picker.

Badge rendering is done in `graph/egui_adapter.rs` `GraphNodeShape` implementation.
Requires access to the membership index at render time — pass as a parameter to `from_graph()`
or read from an `Arc<HashMap>` stored on `GraphBrowserApp`.

---

### Phase 5: Workspace Retention Settings

Extend the existing Persistence Hub with batch maintenance actions:

- "Prune empty workspaces" — remove workspaces with no surviving nodes after stale-NodeKey pruning.
- "Keep latest N named" — delete all named workspaces beyond N (sorted by activation recency).
- Retention policy: autosaved session workspaces (reserved prefix) are always exempt from batch prune.

After batch mutations complete, rebuild the membership index by calling
`build_membership_index_from_layouts` (desktop layer) and `init_membership_index` with the result.
Incremental update during batch delete would be complex; full rebuild is safe and correct here.

---

### Validation Checklist

1. **Node in 1 workspace**: double-click → that workspace restores; no fallback workspace created.
2. **Node in N workspaces**: default open picks highest-recency workspace; `Choose Workspace...`
   opens a specific one.
3. **Node in 0 workspaces**: default open → `OpenInCurrentWorkspace`; no workspace auto-persisted.
4. **Open with Neighbors**: synthesized workspace contains node + direct neighbors, max 12 tiles.
5. **Workspace restore produces empty tree**: falls back to `OpenInCurrentWorkspace`, logs warning.
6. **Workspace delete**: immediately removed from membership sets and recency candidates; resolver
   never returns the deleted name.
7. **Node URL change**: membership index unchanged (UUID is stable; `SetNodeUrl` does not affect
   membership).
8. **Node removed**: UUID entry removed from membership index entirely.
9. **Startup scan**: membership index populated before first frame renders.
10. **Batch prune**: membership index rebuilt after completion; no stale entries remain.
11. **Resolver determinism**: identical inputs always produce the same `WorkspaceOpenAction`.
12. **Unsaved modification**: graph-mutating action while unsaved sets
    `unsaved_workspace_modified`; non-graph actions (zoom, tile reorder) do not.

Automated coverage added (2026-02-19):
- Item 7: `app::tests::test_set_node_url_preserves_workspace_membership`
- Item 10: `desktop::persistence_ops::tests::test_prune_empty_named_workspaces_rebuilds_membership_index`
  and `desktop::persistence_ops::tests::test_keep_latest_named_workspaces_rebuilds_membership_index`
- Item 11: `app::tests::test_resolve_workspace_open_deterministic_fallback_without_recency_match`
- Supporting index-scan behavior: `desktop::persistence_ops::tests::test_build_membership_index_from_layouts_skips_reserved_and_stale_nodes`

Headed-manual execution tracking:
- Remaining manual validations are tracked in
  `ports/graphshell/design_docs/graphshell_docs/tests/VALIDATION_TESTING.md`
  under `Workspace Routing and Membership (Headed Manual)`.

---

### Out of Scope (This Doc)

1. Full multi-window architecture changes.
2. Non-workspace graph semantics (edge taxonomy changes).
3. Command palette redesign beyond wiring to open intents.
4. Bookmarks, node versioning/history, and Persistence Hub expansion — tracked separately.
5. Changing `node_last_active_workspace` key type to `Uuid` (noted as a follow-on in Phase 2).

---

## Findings

### Existing Workspace Infrastructure (as of 2026-02-19)

| Mechanism | State |
| --------- | ----- |
| `workspace_activation_seq: u64` | Exists in `GraphBrowserApp`; monotonic counter |
| `node_last_active_workspace: HashMap<NodeKey, (u64, String)>` | Exists; recency per-node (most-recent workspace only); NodeKey-keyed |
| `workspace_nodes_from_tree(tree)` | Exists in `gui_frame.rs`; extracts NodeKeys from live tile tree |
| `note_workspace_activated(name, nodes)` | Exists; fires on restore; updates recency map |
| `delete_workspace_layout(name)` | Exists; prunes `node_last_active_workspace` entries |
| `list_workspace_layout_names()` | Exists; returns all persisted workspace names |
| `prune_stale_webview_tile_keys_only(tree, app)` | Exists in `desktop/tile_runtime.rs`; pub(crate) |
| Membership index (`uuid → [workspace names]`) | **Does not exist** — gap to be filled by Phase 1 |
| Routing resolver | **Does not exist** — gap to be filled by Phase 2 |

### NodeKey Instability

`NodeKey = NodeIndex` from petgraph `StableGraph`. NodeIndex values are stable within a session but
are **not persisted** and **not stable across sessions**. Workspace layout JSONs embed NodeKeys
(`TileKind::WebView(NodeKey)`). On restore, `prune_stale_webview_tile_keys_only` removes tiles
whose embedded NodeKey does not match any current-session node.

This means: scanning raw workspace JSONs for NodeKeys and comparing them to current-session NodeKeys
is invalid. The membership index must be built **after pruning** (using surviving NodeKeys) and keyed
by **Stable Node UUID** (`Node.id`, which is session-independent and unique). On lookup, checking
`node.id` is O(1) via `node.id` field on the `Node` struct.

### Layer Constraint: `TileKind` is Desktop-Only

`TileKind` (which wraps `NodeKey` in `TileKind::WebView(NodeKey)`) is defined in the desktop layer.
`app.rs` does not and should not import it. Therefore:

- `GraphBrowserApp` cannot deserialize workspace layout JSONs.
- The startup membership index scan (`build_membership_index_from_layouts`) must live in
  `desktop/persistence_ops.rs` alongside the existing workspace restore logic.
- `GraphBrowserApp` exposes `init_membership_index(map)` as a pure setter; the desktop layer does
  the scanning and passes the result in.
- Incremental maintenance (restore, delete, remove-node) does not need `TileKind` — it operates on
  NodeKeys and UUIDs already available in the app layer, so those paths remain on `GraphBrowserApp`.

### Recency Map Key Type

`node_last_active_workspace: HashMap<NodeKey, (u64, String)>` is NodeKey-keyed. The resolver
needs `uuid → NodeKey` translation (via `graph.id_to_node`) before a recency lookup. This is O(1)
and safe for now. A cleaner follow-on: change the key to `Uuid` so that per-node recency data
survives across session restarts (currently lost each session since NodeKeys are session-local).

### Right-Click in egui_graphs 0.29

`Event` variants: `NodeDoubleClick`, `NodeDragStart`, `NodeDragEnd`, `NodeMove`, `NodeSelect`,
`NodeDeselect`, `Zoom`. No right-click event. Right-click must be detected via:

```rust
ui.input(|i| i.pointer.secondary_clicked())
```

combined with `app.hovered_graph_node` (authoritative per-frame hovered node from egui_state).

### Unsaved Workspace Semantics

Synthesized workspaces (for zero-membership nodes) do not call
`save_workspace_layout_json`. They switch the tile tree without persisting. The session autosave
will capture the synthesized layout on the next autosave tick, but the layout is not given a named
workspace entry. User must explicitly save to create a membership record.

### "Open with Neighbors" Bound

`MAX_CONNECTED_SPLIT_PANES` (existing constant) caps connected-open at N tiles. One-hop neighbor
count for a well-connected node in a typical GraphShell session is O(3–8). Two-hop can exceed 50.
Default bound for "Open with Connected": same cap, 2-hop BFS, truncated by activation recency if
over limit (most recently activated neighbors kept).

---

## Progress

### 2026-02-19 — Session 1

- Initial draft written.
- Critique performed: identified NodeKey instability, missing membership index spec, right-click
  implementation basis, double-click UX fallback, unsaved workspace semantics, Phase 4 scope,
  and traversal bound gaps.
- Plan revised to address all critique points.
- Phase 4 (Persistence Hub expansion, bookmarks, node history) extracted to a separate future doc.
- **Status**: Revised draft. Implementation not started.

### 2026-02-19 — Session 2

- Second critique performed: identified `TileKind` layer constraint blocking `rebuild_membership_index`
  as an app method; recency map key type split; stale "URL-key" wording in Phase 5; wrong validation
  items 7–8; unspecified unsaved modification semantics; missing task-list entries for
  `OpenNodeWorkspaceRouted`, `pending_node_context_target`, and `current_workspace_is_unsaved`.
- Plan revised: startup scan moved to desktop layer (`build_membership_index_from_layouts`);
  `init_membership_index` setter added; UUID→NodeKey recency translation made explicit; Invariant 4
  refined with specific fields and narrow "modified" definition; task lists added to all phases;
  validation items 7–8 corrected; "URL-key pruning" corrected to "stale-NodeKey pruning".
- **Status**: Revised draft. Implementation not started.
