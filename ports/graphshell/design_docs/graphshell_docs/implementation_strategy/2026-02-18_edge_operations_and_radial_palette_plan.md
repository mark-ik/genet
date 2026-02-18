# Edge Operations and Radial Command Plan (2026-02-18)

## Status

Draft (critiqued Feb 18; see Code Audit and Revised Recommendations below).

## Purpose

Define a practical, architecture-aligned plan for:

- explicit edge creation and deletion UX,
- a radial command palette that routes through intents,
- multi-node selection semantics that simplify edge workflows.

This is a desktop-focused plan for graphshell prototype iteration.

## Relationship to Existing Plans

- Extends `2026-02-17_feature_priority_dependency_plan.md` follow-on UX work.
- Must remain consistent with explicit targeting in `2026-02-18_f6_explicit_targeting_plan.md`.
- Uses control-plane intent boundaries from `2026-02-16_architecture_and_navigation_plan.md`.
- Explicitly independent of deferred structural cleanup in `2026-02-18_single_window_active_obviation_plan.md`.

## Current Baseline (Code Truth)

- `RemoveEdge` intent and persistence replay are implemented (`app.rs`, `persistence/*`, `graph/mod.rs`).
- `CreateUserGroupedEdge` intent exists and is used for explicit grouping flows.
- Edge data model supports `Hyperlink`, `History`, `UserGrouped`.
- Multi-pane/focused-target routing is in place for desktop.

## Migration Note (Current -> Planned)

Current deterministic `UserGrouped` behavior is implemented for explicit split-open gesture (`Shift+Double-click` path). This plan extends that baseline with:

1. explicit drag-into-same-tab-group trigger semantics,
2. explicit "group with focused" command,
3. multi-select command flows and bulk operations.

## Problem Statement

Edge operations are available in code but not yet exposed as a coherent user-facing interaction model. Current gestures are hard to discover and do not scale cleanly to bulk graph operations.

## Design Goals

1. Keep all edge mutations intent-backed and deterministic.
2. Provide a discoverable command surface (radial palette + keyboard parity).
3. Make multi-select first-class for bulk edge creation/deletion.
4. Avoid reintroducing global-active targeting semantics.

## Non-Goals

1. Auto-creating semantic edges from ad hoc UI heuristics.
2. Reworking Servo runtime callbacks for edge features.
3. Replacing existing node lifecycle semantics.

## Out of Scope This Cycle

1. Full trait-based command registry abstraction.
2. Ordered multi-select (`Chain Selection`) data-model migration.
3. Bulk `N > 2` operations in default UX path (`Fully Connect Selection`, bulk remove).

## Edge Semantics

### Semantic (automatic, reducer-managed)

- `Hyperlink`: derived from navigation/link-follow semantics.
- `History`: derived from traversal transitions.

These should not be created by arbitrary user gesture paths unless explicitly requested as an advanced action.

### User (explicit)

- `UserGrouped`: created and removed only from explicit user action.
- Primary UX targets:
  - connect two selected nodes,
  - connect selection to target,
  - remove selected edge type between selected nodes.

## Deterministic Grouping Trigger Matrix

`UserGrouped` edge creation must be deterministic and tied to explicit grouping intent.

1. `Split open` (`Shift+Double-click` / split action): create `UserGrouped(from=previous_selection, to=target)` when both nodes exist and differ.
2. `Drag into same tab group` (tile grouping gesture): create `UserGrouped(a, b)` only when two previously separate node-backed detail panes become grouped by user drag/drop.
3. `Group with focused` (explicit command): create `UserGrouped(focused_node, selected_or_hovered_node)`.
4. Node focus change, tab switch, pane focus change: no edge creation.
5. Automatic navigation/history transitions: no `UserGrouped` edge creation.

Rules:

1. No self-edge.
2. Idempotent create (skip if edge already exists).
3. Emit only via intent (`CreateUserGroupedEdge`), never direct graph mutation.

### Trigger Semantics (Precise)

1. Directed edge policy:
- Default create is directed (`from -> to`), not bidirectional.
- Bidirectional creation is a separate explicit command (`Connect Both Directions`).

2. Split-open mapping:
- `from = previous selected node`
- `to = target node opened in split`

3. Drag-into-same-tab-group mapping:
- Fire only when operation transitions from separate containers to same tabs container.
- `from = dragged pane node`
- `to = first existing node in destination tabs container` (deterministic anchor used by current implementation).
- If either side has no resolvable node key, do not emit edge.

4. Cold-node behavior:
- Lifecycle (`Cold`/`Active`) does not block edge creation when node keys exist.
- Missing node key always blocks edge creation.

5. Existing-group no-op:
- Reordering tabs or dragging within the same existing tabs container must not create edges.

## Radial Command Model

## Command Context

Resolve command context each frame from:

- selected nodes (`Vec<NodeKey>`),
- hovered node/edge (optional),
- focused detail pane/webview (optional),
- current mode (`Graph`, `Detail`).

No command falls back to global-active authority.

## Command Registry

Use a single command registry (shared by radial UI and keyboard command palette):

- `id`,
- `label`,
- `category`,
- `is_enabled(context)`,
- `execute(context) -> Vec<GraphIntent>`.

## Initial Radial Commands (Edge-focused)

1. `Connect Selected Pair` (exactly 2 selected nodes) -> `CreateUserGroupedEdge { from, to }`.
2. `Connect Both Directions` (exactly 2 selected nodes) -> two intents.
3. `Connect Source -> Hovered` (1 selected + hovered) -> one intent.
4. `Remove User Edge` (2 selected/edge hovered) -> `RemoveEdge { edge_type: UserGrouped }`.
5. `Remove History Edge` (advanced/debug gated) -> `RemoveEdge { edge_type: History }`.
6. `Remove Hyperlink Edge` (advanced/debug gated) -> `RemoveEdge { edge_type: Hyperlink }`.

## Multi-Select Simplification

Multi-select reduces mode friction by removing "pending source" state for common workflows.

Recommended semantics:

1. Primary select: click node.
2. Add/remove select: `Ctrl+Click` (or platform equivalent).
3. Range add (optional later): `Shift+Click` nearest path/radius policy.
4. Clear selection: click empty graph space.

Bulk edge actions:

1. If `N == 2`, edge commands operate directly on pair.
2. If `N > 2`, provide:
  - `Fully Connect Selection` (pairwise `UserGrouped` creation, deduped),
  - `Chain Selection` (selection order dependent),
  - `Remove UserGrouped Among Selection` (pairwise removal).

Guardrails:

- no self-edge by default,
- idempotent creation (skip existing),
- removal reports count for confirmation/logging.

## Implementation Plan

### Phase A: Command Surface Plumbing

1. Add command context resolver in desktop UI layer.
2. Add edge command registry entries.
3. Wire radial invocation to emit intents only.

Exit criteria:

- command enable/disable matches context for 0/1/2/N selected nodes,
- no direct graph mutation in radial handlers.

### Phase B: Multi-Select Core

1. Normalize multi-select state ownership in graph view model.
2. Add deterministic selection gestures and visual affordance.
3. Expose selected-node list to command context.

Exit criteria:

- selected node set is stable across pane focus changes,
- toolbar/radial can read same selection state.

### Phase B1: Grouping Trigger Implementation

1. Implement missing trigger(s) in tile/grouping pipeline for "drag into same tab group".
2. Add explicit `Group with focused` command path and intent emission.
3. Enforce matrix no-trigger paths in UI plumbing.

Exit criteria:

- each trigger in the matrix maps to one clear intent path,
- non-trigger interactions cannot create `UserGrouped` edges.

### Phase C: Bulk Edge Operations

1. Implement pair and bulk edge intents emission helpers.
2. Reuse existing reducer idempotency rules.
3. Add operation feedback (counts/errors) in UI status area/log.

Exit criteria:

- create/remove commands are deterministic and persisted,
- replay reproduces final edge state.

### Phase D: Validation and Hardening

1. Unit tests for command context and enabled-state matrix.
2. Reducer + persistence tests for each trigger and no-trigger path.
3. Manual UX checklist for radial + keyboard parity.

Exit criteria:

- no regression to focused-pane navigation behavior,
- edge operations work with multiple visible detail panes.

## Immediate Next Slice

This is the concrete next implementation slice:

1. Define and lock the deterministic trigger matrix (split, drag-into-same-tab-group, explicit group-with-focused, no-trigger cases).
2. Implement missing trigger(s) in tile/grouping pipeline.
3. Add reducer + persistence tests per trigger/no-trigger path.
4. Add a short headed-window manual checklist section for grouping behavior validation.

Primary likely touchpoints:

1. `ports/graphshell/desktop/tile_grouping.rs`
2. `ports/graphshell/desktop/gui.rs`
3. `ports/graphshell/desktop/tile_post_render.rs`
4. `ports/graphshell/app.rs`
5. `ports/graphshell/persistence/mod.rs`

## Test Matrix (Required)

| Case | Expected | Suggested Test Location |
| --- | --- | --- |
| Split-open trigger emits one `CreateUserGroupedEdge` | edge exists once | `ports/graphshell/app.rs` reducer test |
| Split-open repeated on same pair | idempotent (still one edge) | `ports/graphshell/app.rs` reducer test |
| Drag separate panes into same tab group | one edge created | `ports/graphshell/desktop/tile_grouping.rs` unit/integration helper test |
| Drag within same tabs container (reorder only) | no edge | `ports/graphshell/desktop/tile_grouping.rs` test |
| Group-with-focused command | one directed edge | command-context/GUI test + reducer test (`ports/graphshell/app.rs`) |
| Focus/tab switch/navigation only | no `UserGrouped` edge | `ports/graphshell/app.rs` no-trigger test |
| Persistence replay after create/remove | final edge set preserved | `ports/graphshell/persistence/mod.rs` test |

## Risks and Mitigations

1. Selection ambiguity across graph/detail views.
- Mitigation: explicit context precedence (`Graph selection` > `hover` > `focused pane node`).

2. Bulk operation surprise for large selections.
- Mitigation: confirmation threshold above configurable `N`.

3. Edge-type misuse by non-debug users.
- Mitigation: keep non-`UserGrouped` edge commands behind advanced/debug affordance.

## Validation Checklist (Initial)

1. Select two nodes, run `Connect Selected Pair`, confirm one `UserGrouped` edge added.
2. Repeat command, confirm idempotent result.
3. Run `Remove User Edge`, confirm edge removed and persisted.
4. Select three nodes, run `Fully Connect Selection`, confirm expected pair count.
5. Reload from persistence, confirm created/removed edges replay correctly.

## Critique Resolution Note

Critique-era open questions were resolved on 2026-02-18 and are recorded in the Decision Log below.

## Decision Log

Use this section to close open questions before implementation starts.

| Date | Decision | Rationale | Owner |
| --- | --- | --- | --- |
| 2026-02-18 | Defer `Fully Connect Selection` from this cycle | Bulk operations are quadratic and need separate UX guardrails; pair operations validate model first | Graphshell team |
| 2026-02-18 | Do not track selection order in this phase | `SelectionState` is unordered (`HashSet`); ordered model migration is separate work | Graphshell team |
| 2026-02-18 | Keyboard-first command parity, radial UI second | Lowest implementation cost and fastest validation while preserving shared dispatch path | Graphshell team |

---

## Code Audit (Feb 18)

Full codebase audit of edge operations, selection, grouping triggers, and command patterns.

### Implementation Inventory

| Capability | Status | Location |
| --- | --- | --- |
| `SelectionState` with `multi_select: bool` parameter | Struct implemented; `multi_select: true` **never used** in any production call site | `app.rs:54-117` |
| `Ctrl+Click` toggle-select in graph view | **Not implemented** — all 6 call sites pass `multi_select: false` | `tile_behavior.rs:165,185,245`; `render/mod.rs:293,299,315`; `webview_controller.rs:33,88`; `graph_search_flow.rs:106` |
| `CreateUserGroupedEdge` intent + reducer | **Implemented** and tested (idempotent, no self-edge) | `app.rs:419,936-949` |
| Split-open trigger (`Shift+Double-click`) | **Implemented** — reads `selected_nodes.primary()` as `from`, target as `to` | `tile_behavior.rs:172-191` |
| Drag-into-same-tab-group trigger | **Implemented** — compares tab-group membership before/after tile render, emits edge for moved nodes | `tile_grouping.rs:55-79`, orchestrated by `tile_post_render.rs:47-49` |
| "Group with focused" explicit command | **Not implemented** | — |
| `RemoveEdge` intent + reducer | **Implemented** and tested (type-specific, returns removed count) | `app.rs:422-428,636-648` |
| Radial command palette UI | **Not implemented** (design only) | — |
| Command registry pattern | **Not implemented** — inline match dispatch | `tile_behavior.rs`, `render/mod.rs:286`, `input/mod.rs:95` |
| Bulk edge operations (N > 2) | **Not implemented** | — |
| Persistence replay for edge create/remove | **Implemented** — `LogEntry::AddEdge`, `LogEntry::RemoveEdge` with `PersistedEdgeType` | `app.rs:651-700`, `persistence/mod.rs`, `persistence/types.rs` |

### Trigger Matrix vs Code Truth

| Trigger | Plan Description | Code Reality | Discrepancy |
| --- | --- | --- | --- |
| Split-open | `from = previous selected node, to = target` | `from = selected_nodes.primary(), to = key` from `FocusNodeSplit(key)` | **Match** |
| Drag-into-same-tab-group | `from = dragged pane node, to = destination focused tab node` | `from = moved_node, to = first peer in new group` (arbitrary anchor, not focused tab) | **Mismatch** — code picks first node in destination group, not focused tab |
| Existing-group no-op (reorder within tabs) | No edge | `user_grouped_intents_for_tab_group_moves` only fires when group TileId changes, not on reorder | **Match** |
| Focus/tab switch/navigation | No edge | No `CreateUserGroupedEdge` emitted from these paths | **Match** |

### Selection State Architecture

`SelectionState` uses `HashSet<NodeKey>` for the node set and `Option<NodeKey>` for primary. Key observations:

1. **No insertion order tracking.** `HashSet` is unordered. The plan's `Chain Selection` (line 161) is "selection order dependent" but the data structure cannot provide order. Would need `IndexSet` or `Vec<NodeKey>` with dedup to support this.

2. **`primary()` tracks most-recently-selected only.** Useful for pair operations (`from = primary, to = new_selection`) but not for ordered chains.

3. **`Deref<Target = HashSet<NodeKey>>`** exposes read-only set access — command context can read `.len()`, `.contains()`, `.iter()` without mutation.

4. **Revision counter** (`u64`) enables cheap change detection for UI refresh.

### Existing Action/Intent Patterns

The codebase has three action-to-intent conversion layers, none using a registry:

1. **`GraphAction` enum** (7 variants in `render/mod.rs`) — graph-view UI events. Converted to intents via `intents_from_graph_actions()`.
2. **`KeyboardActions` struct** (boolean flags in `input/mod.rs`) — keyboard state. Converted via `intents_from_actions()`.
3. **Inline match in `tile_behavior.rs:pane_ui()`** — intercepts `FocusNode`/`FocusNodeSplit` before they reach generic conversion, adding tile-specific logic (pending opens, edge creation).

All three ultimately produce `Vec<GraphIntent>` applied through `app.apply_intents()`.

### Upstream Impact

None. This plan is entirely graphshell-local. Edge types, selection, command dispatch, and UI rendering are all in graphshell-owned code. No servo core API changes needed. No compatibility concerns with servoshell.

---

## Revised Recommendations (Feb 18)

### Phase Restructuring

The original plan's phase ordering (A -> B -> B1 -> C -> D) front-loads abstraction (command registry) before the prerequisite it depends on (multi-select). The triggers it identifies as Phase B1 are already ~90% implemented. Recommended restructuring:

**Step 1: Wire `Ctrl+Click` multi-select** (trivial — ~5 lines in `render/mod.rs` to read `ui.input(|i| i.modifiers.ctrl)` and pass through to `GraphAction::SelectNode`). Unblocks all pair and bulk commands.

**Step 2: Add "Group with focused" command** (the one missing trigger from the matrix). Single intent emission, same pattern as split-open in `tile_behavior.rs`.

**Step 3: Add edge commands with simple match dispatch** — `Connect Selected Pair`, `Remove User Edge`, `Connect Both Directions`. Use an `enum EdgeCommand` with match-based dispatch, not a trait-based registry. 6 commands in a match statement is simpler and more debuggable than 6 commands in a registry with dynamic dispatch. The registry pattern can be introduced later when command count exceeds ~15.

**Step 4: Add radial/palette UI** to invoke commands from Step 3. Ship keyboard shortcuts first (cheaper — wire into existing input handler), radial UI second. Both invoke the same match dispatch.

**Step 5 (deferred): Bulk N > 2 operations** — `Fully Connect Selection` creates N*(N-1)/2 edges (quadratic). Needs real UX guardrails (confirmation dialog, max-N threshold). Defer to separate plan when pair operations have validated the interaction model.

### Trigger Semantics Correction

The drag-into-same-tab-group trigger description (line 102) says `to = destination focused tab node`. The code (`tile_grouping.rs:72`) uses `to = first peer in new group` (arbitrary anchor from the destination group). Either:

- Update the plan to match code: `to = first existing node in destination tabs container`.
- Or change the code to resolve the focused/active tab in the destination container.

The current code behavior is deterministic and correct — it just doesn't match the plan's language.

### `SelectionState` Data Structure Note

If `Chain Selection` (ordered multi-select) is a planned feature, `SelectionState.nodes` must change from `HashSet<NodeKey>` to an ordered set (`IndexSet<NodeKey>` from the `indexmap` crate, or `Vec<NodeKey>` with dedup). This is a breaking change to the `Deref<Target = HashSet<NodeKey>>` impl. Flag this as a known prerequisite before implementing chain semantics — don't discover it during implementation.

### Command Registry Deferral Rationale

The plan proposes `id, label, category, is_enabled(context), execute(context) -> Vec<GraphIntent>`. This is a trait-based abstraction for 6 commands. The current codebase manages 26 `GraphIntent` variants, 7 `GraphAction` variants, and ~10 keyboard actions all through enum+match dispatch without a registry. Adding a registry now would:

- Create a second dispatch layer alongside the existing `GraphIntent` reducer.
- Require dynamic dispatch or trait objects for `is_enabled`/`execute`.
- Add indirection that makes debugging harder for no current benefit.

The registry becomes justified when: (a) command count exceeds ~15, (b) a searchable keyboard command palette ships, or (c) commands need runtime registration (plugins/extensions). None of these apply to the current prototype scope.

### Answered Open Questions

**Q1: Should `Fully Connect Selection` be shipped now or deferred?**
Defer. Quadratic edge creation needs UX guardrails not worth designing for prototype. Ship pair operations first, validate the interaction model, then plan bulk operations.

**Q2: Should selection order be explicitly tracked for `Chain Selection`?**
Not in this phase. `SelectionState` uses `HashSet` — adding order tracking requires changing the data structure and breaking `Deref` impl. Only invest when chain selection is confirmed as needed UX.

**Q3: Radial-only first, or simultaneous keyboard command palette parity?**
Keyboard shortcuts first (cheaper to wire into existing `input/mod.rs` handler). Radial UI second. Both invoke the same command dispatch, so parity is structural, not additional work.

## Execution Plan (Canonical)

This section supersedes the original phased draft for implementation order.

### Step 1: Wire `Ctrl+Click` Multi-Select

Work:

1. Read ctrl modifier from graph interaction input path.
2. Pass `multi_select: true` on ctrl-modified select-node actions.
3. Keep non-modified click behavior unchanged.

Done criteria:

1. Ctrl-click toggles membership in selected node set.
2. Plain click still sets single primary selection.
3. Existing selection-related tests remain green.

### Step 2: Add Explicit `Group With Focused` Command

Work:

1. Add command action and intent emission path (`CreateUserGroupedEdge`).
2. Resolve `from = focused/primary`, `to = selected_or_hovered`.
3. Reuse reducer idempotency and self-edge guards.

Done criteria:

1. Command creates exactly one directed `UserGrouped` edge for valid pair.
2. Invalid/missing endpoints emit no mutation.
3. Repeating command on same pair is idempotent.

### Step 3: Add Pair Edge Commands With Enum+Match Dispatch

Work:

1. Add `EdgeCommand` enum for initial pair operations:
   - `ConnectSelectedPair`
   - `ConnectBothDirections`
   - `RemoveUserEdge`
2. Wire command handlers to emit `GraphIntent` only.
3. Reuse same dispatch path for keyboard and later radial UI.

Done criteria:

1. All pair commands execute through intent pipeline only.
2. No direct graph mutation from UI handlers.
3. Reducer and persistence tests pass for create/remove paths.

### Step 4: Add Radial/Palette UI Invocation

Work:

1. Add radial/palette entrypoint using existing command dispatch.
2. Gate command availability by context (`is_enabled` logic in match path).
3. Keep keyboard parity by using same command execution function.

Done criteria:

1. Radial actions and keyboard actions produce identical intent outputs.
2. Context-disabled commands are not invokable.
3. No regressions to focused-pane navigation/focus behavior.

### Step 5: Defer Bulk `N > 2` Operations

Work:

1. Document deferred scope and prerequisites (confirmation UX, max-N threshold).
2. Re-evaluate after pair operations validate user workflow.

Done criteria:

1. Bulk operations are explicitly deferred in plan and decision log.
2. No accidental bulk behavior exposed in this cycle.

---

## Appendix: Original Implementation Plan (Pre-Audit)

The phases below are the original plan as drafted. See Revised Recommendations above for the restructured approach.

### Original Phase A: Command Surface Plumbing

1. Add command context resolver in desktop UI layer.
2. Add edge command registry entries.
3. Wire radial invocation to emit intents only.

Exit criteria:

- command enable/disable matches context for 0/1/2/N selected nodes,
- no direct graph mutation in radial handlers.

### Original Phase B: Multi-Select Core

1. Normalize multi-select state ownership in graph view model.
2. Add deterministic selection gestures and visual affordance.
3. Expose selected-node list to command context.

Exit criteria:

- selected node set is stable across pane focus changes,
- toolbar/radial can read same selection state.

### Original Phase B1: Grouping Trigger Implementation

1. Implement missing trigger(s) in tile/grouping pipeline for "drag into same tab group".
2. Add explicit `Group with focused` command path and intent emission.
3. Enforce matrix no-trigger paths in UI plumbing.

Exit criteria:

- each trigger in the matrix maps to one clear intent path,
- non-trigger interactions cannot create `UserGrouped` edges.

### Original Phase C: Bulk Edge Operations

1. Implement pair and bulk edge intents emission helpers.
2. Reuse existing reducer idempotency rules.
3. Add operation feedback (counts/errors) in UI status area/log.

Exit criteria:

- create/remove commands are deterministic and persisted,
- replay reproduces final edge state.

### Original Phase D: Validation and Hardening

1. Unit tests for command context and enabled-state matrix.
2. Reducer + persistence tests for each trigger and no-trigger path.
3. Manual UX checklist for radial + keyboard parity.

Exit criteria:

- no regression to focused-pane navigation behavior,
- edge operations work with multiple visible detail panes.
