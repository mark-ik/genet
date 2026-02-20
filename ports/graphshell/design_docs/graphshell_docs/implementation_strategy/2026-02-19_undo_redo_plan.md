<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Undo/Redo Plan (2026-02-19)

**Status**: Implementation complete; gaps: no unit tests, spec clarification pending for persistence-surface boundary.

---

## Plan

### Context

Undo/redo is functional. `GraphBrowserApp` holds `undo_stack: Vec<UndoRedoSnapshot>` and
`redo_stack: Vec<UndoRedoSnapshot>` (max 128 steps). `UndoRedoSnapshot` captures `graph: Graph`,
`selected_nodes: SelectionState`, `highlighted_graph_edge`, and `workspace_layout_json: Option<String>`.

The spec was written in `2026-02-18_edge_operations_and_cmd_palette_plan.md §Global Undo/Redo
Boundary` as part of the command dispatch redesign. This plan extracts and formalizes that spec
with implementation gaps identified and addressed.

---

### Implementation Inventory

**Core mechanics** (`app.rs`):

- `capture_undo_checkpoint(workspace_layout_json)` — pushes snapshot, clears redo stack, trims at 128.
- `perform_undo(current_layout_json) -> bool` — pops undo stack, pushes current state to redo; applies
  graph via `apply_loaded_graph()`; sets `pending_history_workspace_layout_json`.
- `perform_redo(current_layout_json) -> bool` — pops redo stack, pushes current state to undo; same apply path.
- `take_pending_history_workspace_layout_json()` — consumed by `gui_frame.rs` next frame to restore tile tree.

**`is_user_undoable_intent()` guard** (`desktop/gui_frame.rs`):

Captures a checkpoint before `apply_intents()` when any intent in the batch matches:

- `CreateNodeNearCenter`, `CreateNodeAtUrl`
- `RemoveSelectedNodes`, `ClearGraph`
- `SetNodePosition`, `SetNodeUrl`
- `CreateUserGroupedEdge`, `RemoveEdge`, `ExecuteEdgeCommand`
- `SetNodePinned`, `PromoteNodeToActive`, `DemoteNodeToCold`

**Additional capture sites** (`desktop/gui_frame.rs`, `render/mod.rs`):

- Named workspace snapshot restore — captures before restoring tile tree.
- Named graph snapshot restore — captures before closing webviews and applying graph.
- Latest graph snapshot restore — same.
- Detach node to split — captures before `tile_view_ops::detach_webview_tile_to_split()`.
- Connected-open expansion — captures before expanding neighbor tiles.
- Render-layer graph actions (`render/mod.rs:933`) — captures before applying graph-action mutations.

---

### Spec Clarification: Persistence-Surface Boundary

The original spec (edge plan §Global Undo/Redo Boundary) included "save/delete named
workspace/graph snapshots" as undoable. After reviewing the implementation this boundary is
narrowed as follows.

**Included in undo/redo history** (implementation-correct):

1. Graph model mutations — node/edge create/remove/update, pin state, active/cold promotion.
2. Workspace layout mutations — tile open/close/detach/split operations.
3. Restore/load operations — workspace and graph snapshot restore (named and latest).
   These are recorded as atomic reversible transactions.

**Excluded** (narrowed from original spec):

- `save named workspace/graph snapshot` — creating a named external artifact is not a reversible
  UI state transition. Undoing a "save" would require deleting the saved artifact (a destructive I/O
  operation). Excluded.
- `delete named workspace/graph snapshot` — would require storing deleted data in memory to replay.
  Excluded.
- `explicit prune/maintenance mutations` — session workspace prune is a maintenance operation,
  not a user-navigable state transition. Excluded.

---

### Gap: No Unit Tests

The undo/redo mechanics have no automated unit test coverage. The validation items in
`tests/VALIDATION_TESTING.md §Undo/Redo Validation` are all headed/integration tests.

**Tasks**:

- [ ] `test_capture_undo_checkpoint_pushes_and_clears_redo` — after capture, undo_stack grows by
  one; redo_stack is empty.
- [ ] `test_perform_undo_reverts_to_previous_graph` — add node A; capture; add node B; perform_undo
  → graph contains only node A.
- [ ] `test_perform_redo_reapplies_after_undo` — perform_undo then perform_redo → back to
  post-node-B state.
- [ ] `test_undo_stack_trimmed_at_max` — push 129 checkpoints; `undo_stack.len() <= 128`.
- [ ] `test_undo_returns_false_when_stack_empty` — `perform_undo` on empty stack returns false;
  graph unchanged.
- [ ] `test_redo_returns_false_when_stack_empty` — `perform_redo` on empty stack returns false;
  graph unchanged.
- [ ] `test_new_action_clears_redo_stack` — undo; then capture a new checkpoint; redo_stack
  should be empty.

---

### Gap: IMPLEMENTATION_ROADMAP Entry

Undo/redo has no feature entry in `IMPLEMENTATION_ROADMAP.md`. Should be added when the roadmap
is next updated as a completed M2 feature.

**Task**:

- [ ] Add undo/redo feature entry to `IMPLEMENTATION_ROADMAP.md` (completed, under M2 section).

---

## Findings

### Snapshot Architecture

`UndoRedoSnapshot` is a full clone of `Graph` + `SelectionState` + one edge highlight + workspace
JSON string. For ≤128 steps with graphs of moderate size (≤100 nodes) this is acceptable. At large
node counts (500+), snapshot memory cost becomes O(N × 128). Defer optimization unless profiling
shows pressure.

### Workspace Restore via Pending Field

`perform_undo`/`perform_redo` writes workspace layout JSON to `pending_history_workspace_layout_json`
rather than applying it directly. `gui_frame.rs` consumes this on the next frame to restore
`tiles_tree`. This decouples undo mechanics from the tile runtime lifecycle.

### Integration Tests

Full integration tests (persistence parity after undo/redo cycle, multi-intent atomic step,
webview navigation exclusion) are tracked in `tests/VALIDATION_TESTING.md §Undo/Redo Validation`.

---

## Progress

### 2026-02-19 — Session 1

- Plan created from implementation inventory and edge plan §Global Undo/Redo Boundary spec.
- Narrowed persistence-surface boundary; added 7 unit test tasks and roadmap update task.
- Integration validation already tracked in `tests/VALIDATION_TESTING.md §Undo/Redo Validation`.
