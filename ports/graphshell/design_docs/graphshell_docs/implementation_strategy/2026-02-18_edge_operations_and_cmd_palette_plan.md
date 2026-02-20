# Edge Operations and Command Palette Plan (2026-02-18)

## Status

Draft (critiqued Feb 18; see Code Audit and canonical execution plan below).

Implementation progress (2026-02-18):

- Step 1 complete: `Ctrl+Click` multi-select is wired through render -> intent (`SelectNode { multi_select }`).
- Step 2 complete: explicit group command path added via keyboard (`G`) using shared edge-command dispatch.
- Step 3 in progress: enum+match dispatch is implemented in reducer (`ExecuteEdgeCommand` + `EdgeCommand`), with keyboard parity for pair connect/remove and pin/unpin shortcuts.
- Step 4 partial: palette UI invocation exists as a keyboard-invoked panel (`F2`) reusing the same command dispatch path; visual context menu presentation remains pending.
- Context routing now uses deterministic precedence in palette targeting: `selected pair` > `selected primary + hovered node` > `selected primary + focused pane node`.
- Step 4b started: `Open Connected as Tabs` is available in the command palette and expands neighbors deterministically via `in_neighbors + out_neighbors`.
- Step 5b progress: split-connected layout engine uses compact 2-up/2x2 packing (max `MAX_CONNECTED_SPLIT_PANES`) with overflow grouped into tabs.
- Step 5b scope: connected-open is intentionally one-hop (`in_neighbors + out_neighbors`) for this cycle.
- Omnibar `@` search now supports iterative Enter-cycling with match counter and detail-mode focus/open behavior for matched nodes.
- Omnibar `@` search currently supports mode targeting and tab-first mixed ranking in detail/workbench.
- Scope expansion update: explicit `@` scopes are implemented end-to-end:
  - `@n <query>` = nodes in active graph context,
  - `@N <query>` = nodes across active + saved graphs,
  - `@e <query>` = edges with searchable payload in active graph context,
  - `@E <query>` = edges with searchable payload across active + saved graphs,
  - `@t <query>` = tabs in active workspace context,
  - `@T <query>` = tabs across active + saved workspaces,
  - `@<query>` = mixed mode with context-priority ordering.
- Palette close reliability improved (Escape support, explicit Close button, auto-close on command execution).
- Step 4c started: toolbar graph-edit buttons are reduced in favor of a single `Cmd` palette entrypoint; toolbar remains nav-first.
- Step 4c progress: removed toolbar graph-edit controls now have palette parity (`Toggle Physics Panel`, `Create Node`, `Create Node as Tab`).
- Step 4c progress: graph runtime controls are also surfaced in palette (`Toggle Physics Simulation`, `Fit Graph to Screen`) to reduce dependence on scattered shortcuts.
- Step 4d progress: `@` omnibar behavior is implemented (iterative Enter cycling, counter, detail-mode target open/focus); full headed-manual focus-transition validation remains required.
- Step 4d progress: automated matching coverage now includes `@t` tab-only filtering and mixed tab-priority ordering; headed focus-transition validation remains required.
- Step 6 started: command-palette workspace snapshot action is implemented (`Pin Workspace Snapshot`) using existing tile-layout persistence.
- Step 6 progress: named workspace management is implemented in palette UI (save/list/restore/delete).
- Persistence parity update: named graph snapshot management is implemented in palette UI (save/list/load/delete) with runtime-safe restore (close old webviews, reset tile runtime, preserve graph integrity on restore failure).
- Workspace persistence update: reserved session workspace key (`workspace:session-latest`) is now auto-restored on startup (before `latest` fallback) and auto-saved on layout change.
- Persistence UX update: command palette now opens a dedicated Persistence Hub panel for workspace/graph save-load-delete flows; autosaved entries are shown in the same load lists (no separate autosave buttons).
- Workspace pruning update: explicit `Prune Session Workspace` control is available in Persistence Hub.
- Current `@n` behavior: in detail/workbench mode, `@n` resolves local workspace nodes only (context-local by default); graph mode remains broad until `@N`/`@T` scope split is completed.
- Focus UX update: pane focus ring is now transient (short fade pulse on focus switch) rather than a persistent overlay.
- Persistence UX update: toolbar now has a direct `Persist` entrypoint, and Persistence Hub exposes workspace autosave cadence/retention controls.
- Workspace autosave update: session autosave now respects configurable cadence and keeps configurable rolling `session-prev-N` retained revisions.
- Tab/pane management progress: `Detach Focused to Split` is implemented as an explicit per-pane action in the command palette.
- Tab/pane management progress: dragging a tab outside the tab-strip band now requests detach-to-split (command-based detach remains as fallback).
- Split leaf parity update: split-created detail leaves now use single-tab `Tabs` containers so each pane exposes a tab bar/drag handle.
- Workspace-routing handoff: workspace-first open policy and multi-membership routing are tracked in `2026-02-19_workspace_routing_and_membership_plan.md`.

## Resolved vs Remaining (Conversation Audit)

Resolved:

1. Palette close reliability improved (Escape, explicit Close, close-on-command).
2. Connected-open primary path is tabs-first.
3. Split-connected expansion is capped and overflows to tabs.
4. Omnibar `@` search supports iterative Enter-cycling with counter.
5. Detail-mode `@` search requests tab/pane focus/open for matched node.
6. Nucleo fuzzy matching now uses direct case-insensitive matching without manual lowercase preprocessing.
7. Graph hover cue is explicit (hover-highlighted node for command-target disambiguation).
8. Pair edge commands now use deterministic first-selected -> second-selected ordering for selected-pair context.
9. Connected-open behavior is explicitly one-hop for now (no transitive expansion).
10. Split-connected layout engine uses compact 2-up/2x2 packing before overflow-to-tabs.
11. Workspace snapshot save is exposed as an explicit palette action.
12. Named workspace management is implemented (`save/list/restore/delete`).
13. Focused pane can be detached into split layout via explicit palette action.
14. Session-persistent-by-default workspace behavior is implemented (startup restore + change-detected autosave + reserved-key protections).

Remaining / Follow-up:

1. Complete headed manual validation for `@` mode focus-transition paths. → Tracking in `tests/VALIDATION_TESTING.md` §Step 4d.
2. Extend workspace pruning/retention beyond session prune (batch named-workspace maintenance + retention policy).
3. Bookmarks and Node History are delivered via Settings Architecture (`graphshell://settings/bookmarks`, `graphshell://settings/history`) per `2026-02-20_settings_architecture_plan.md`. Edge Traversal Plan Phase 2 will deliver history integration via `EdgePayload` traversal logs.
4. Workspace-first node-open routing and membership UX are tracked in `2026-02-19_workspace_routing_and_membership_plan.md` (deferred from this plan).
5. Validate and tune uppercase/lowercase scope ergonomics (`@N/@T` vs `@n/@t`) based on headed usage.
6. Edge search scopes (`@e/@E`) are implemented; follow-on is UX polish for highlight readability and result affordances.

---

## Cross-References to Related Plans

**Workspace routing**: Workspace-first node-open routing and multi-membership index are tracked in [2026-02-19_workspace_routing_and_membership_plan.md](2026-02-19_workspace_routing_and_membership_plan.md). Session-persistence work in this plan (`workspace:session-latest`, autosave cadence, retention) is complete; workspace membership routing remains in that plan.

**Persistence UI architecture**: Workspace/graph persistence UI is delivered via the Persistence Hub panel. See [2026-02-19_persistence_hub_plan.md](2026-02-19_persistence_hub_plan.md) for bookmarks (tags), node history, maintenance controls, and LRU lifecycle budget configuration.

**Graph search architecture**: Faceted search dimensions (lifecycle state, edge type, traversal recency, tags, visit count) and DOI/relevance weighting are tracked in [2026-02-19_graph_ux_polish_plan.md](2026-02-19_graph_ux_polish_plan.md). Omnibar `@` scope implementation remains in this plan; integration with DOI-based ranking is follow-on work.

## Purpose

Define a practical, architecture-aligned plan for:

- explicit edge creation and deletion UX,
- a command palette (context menu + command registry) that routes through intents,
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
3. multi-select command flows and deferred bulk-operation design.

## Problem Statement

Edge operations are available in code but not yet exposed as a coherent user-facing interaction model. Current gestures are hard to discover and do not scale cleanly to bulk graph operations.

## Design Goals

1. Keep all edge mutations intent-backed and deterministic.
2. Provide a discoverable command surface (command palette + keyboard parity).
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

> **Note**: The Edge Traversal Model migration (tracked in `2026-02-20_edge_traversal_impl_plan.md`) will replace the `EdgeType` enum with `EdgePayload { traversals: Vec<Traversal>, user_asserted: bool }`. The `user_asserted` flag will replace the `UserGrouped` edge type, enabling user-created edges to coexist with navigation-derived traversals while preserving P2P sync commutativity.

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

4. Lifecycle behavior:
- Node lifecycle state (`Active`/`Warm`/`Cold`) does not block edge creation when node keys exist.
- Missing node key always blocks edge creation.

5. Existing-group no-op:
- Reordering tabs or dragging within the same existing tabs container must not create edges.

6. Physics Wake:
- Edge creation/removal intents should wake physics (`is_running = true`) so layout can adapt, except when user is explicitly in Frozen/manual layout mode.


## Command Palette Model

### Command Context

Resolve command context each frame from:

- selected nodes (`Vec<NodeKey>`),
- hovered node/edge (optional),
- focused detail pane/webview (optional),
- current mode (`Graph`, `Detail`).

No command falls back to global-active authority.

### Command Dispatch (Current and Future)

Current-cycle implementation should use enum + match dispatch for simplicity and debuggability.

A future registry (deferred) can be added when command count/extension needs justify it:

- `id`,
- `label`,
- `category`,
- `is_enabled(context)`,
- `execute(context) -> Vec<GraphIntent>`.

### Initial Palette Commands (Edge-focused & Layout)

1. `Connect Selected Pair` (exactly 2 selected nodes) -> `CreateUserGroupedEdge { from, to }`.
2. `Connect Both Directions` (exactly 2 selected nodes) -> two intents.
3. `Connect Source -> Hovered` (1 selected + hovered) -> one intent.
4. `Remove User Edge` (2 selected/edge hovered) -> `RemoveEdge { edge_type: UserGrouped }`.
5. `Pin/Unpin Selected` (1+ selected) -> Toggle `node.is_pinned`.
6. `Remove History Edge` (advanced/debug gated) -> `RemoveEdge { edge_type: History }`.
7. `Remove Hyperlink Edge` (advanced/debug gated) -> `RemoveEdge { edge_type: Hyperlink }`.

### Global Undo/Redo Boundary

Undo/redo is global and shared across graph, workspace, and persistence-facing user commands.

Included in undo/redo history:

1. Graph intents that mutate model state:
- node create/delete/url change/position pinning/selection-affecting structural actions,
- edge create/remove (`UserGrouped`, and explicit advanced edge removals),
- explicit command-triggered graph transforms.

2. Workspace/layout intents that mutate user-visible organization:
- open tab/split/move-to-pane actions,
- detach-to-split and grouping operations,
- workspace restore/switch actions that change current layout context.

3. Persistence-surface commands that mutate named state:
- save/delete named workspace snapshots,
- save/delete named graph snapshots,
- explicit prune/maintenance mutations.

Excluded from undo/redo history:

1. Non-deterministic runtime callbacks/events:
- raw webview lifecycle callbacks (`created/url/title/history changed/crashed`),
- transient network/runtime errors.

2. Purely transient UI state:
- hover/focus ring visuals, panel open/close state, temporary selection hover targets,
- non-mutating search session state (`@` query index/counter).

3. Continuous simulation updates:
- physics frame-by-frame position integration.
- Only explicit user-triggered layout commands (for example `Fit`) are undoable.

Command grouping rules:

1. Multi-intent command actions execute as one undo step:
- examples: `ConnectBothDirections`, split-open + explicit grouping side effect.

2. Batch operations are grouped by originating command:
- one user command = one undo entry, even if multiple node/edge mutations occur.

3. Restore/load actions are atomic:
- graph/workspace restore is recorded as a single reversible transaction.

Failure/consistency requirements:

1. If part of a grouped undoable command fails, the command should roll back or no-op as a unit.
2. Undo/redo replay must remain deterministic against persisted snapshots/logs.
3. Undo/redo stack entries must survive routine UI mode switches (graph/detail/workbench).

## Multi-Select Simplification

Multi-select reduces mode friction by removing "pending source" state for common workflows.

Recommended semantics:

1. Primary select: click node.
2. Add/remove select: `Ctrl+Click` (or platform equivalent) — ✅ implemented.
3. Range add (optional later): `Shift+Click` nearest path/radius policy.
4. Lasso select: `Right+Drag` rectangle selection with spatial index routing — ✅ implemented via `GraphAction::LassoSelect`.
5. Clear selection: click empty graph space.

Bulk edge actions (deferred this cycle):

1. If `N == 2`, edge commands operate directly on pair.
2. If `N > 2`, provide:
  - `Fully Connect Selection` (pairwise `UserGrouped` creation, deduped),
  - `Chain Selection` (selection order dependent),
  - `Remove UserGrouped Among Selection` (pairwise removal).

Guardrails:

- no self-edge by default,
- idempotent creation (skip existing),
- removal reports count for confirmation/logging.

## Immediate Touchpoints

1. ports/graphshell/desktop/tile_grouping.rs
2. ports/graphshell/desktop/gui.rs
3. ports/graphshell/desktop/tile_post_render.rs
4. ports/graphshell/app.rs
5. ports/graphshell/persistence/mod.rs
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
4. Deferred in this cycle: bulk-selection operations validated in follow-on plan.
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

## Execution Plan (Canonical)

This is the authoritative implementation order for this cycle.

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

1. Add `ConnectSelectedPair`, `ConnectBothDirections`, `RemoveUserEdge`, `PinSelected`/`UnpinSelected` as enum variants with match-based dispatch.
2. Wire command handlers to emit `GraphIntent` only; no direct graph mutation.
3. Reuse same dispatch path for keyboard shortcuts and later palette UI.
4. Ensure all edge create/remove commands wake physics (`is_running = true`).

Done criteria:

1. All pair commands execute through intent pipeline only.
2. No direct graph mutation from UI handlers.
3. Reducer and persistence tests pass for create/remove paths.
4. Physics simulation wakes (`is_running = true`) after any edge create or remove command.

### Step 4: Add Command Palette UI Invocation

Work:

1. Add palette entrypoint using existing command dispatch.
2. Gate command availability by context (`is_enabled` logic in match path).
3. Keep keyboard parity by using same command execution function.

Done criteria:

1. Palette actions and keyboard actions produce identical intent outputs.
2. Context-disabled commands are not invokable.
3. No regressions to focused-pane navigation/focus behavior.

### Step 5: Defer Bulk `N > 2` Operations

Work:

1. Document deferred scope and prerequisites (confirmation UX, max-N threshold).
2. Re-evaluate after pair operations validate user workflow.

Done criteria:

1. Bulk operations are explicitly deferred in plan and decision log.
2. No accidental bulk behavior exposed in this cycle.

### Step 4b: Palette Expansion (User-Visible ROI)

Work:

1. Add command(s) for opening connected nodes as tabs from current context.
2. Add command(s) for opening connected nodes in split layout (optional follow-up).
3. Keep execution routed through existing command dispatch path.

Done criteria:

1. User can open neighbors of selected/focused node in tabbed detail view via command palette.
2. Behavior is deterministic (stable open set and ordering policy documented).
3. No toolbar-only dependency for this workflow.

### Step 4c: Toolbar Decomposition (Nav-First)

Work:

1. Move graph-edit/layout commands from top toolbar into palette command surfaces.
2. Keep toolbar focused on browser primitives (back/forward/reload + omnibar + minimal settings).

Done criteria:

1. Graph controls are discoverable in palette surface.
2. Toolbar density is reduced without loss of functionality.

### Step 4d: `@` Omnibar Behavior Polish and Validation

**Validation checklist moved to `tests/VALIDATION_TESTING.md` §Step 4d.**

Work:

1. Keep `@query` path deterministic: Enter advances to next match, wraps at end, and shows `current/total` counter in the bar.
2. Support explicit query scopes:
  - `@n <query>` for nodes in active graph context,
  - `@N <query>` for nodes across active + saved graphs,
  - `@e <query>` for searchable edges in active graph context,
  - `@E <query>` for searchable edges across active + saved graphs,
  - `@t <query>` for tabs in active workspace context,
  - `@T <query>` for tabs across active + saved workspaces,
  - default `@<query>` mixed mode.
3. In detail mode mixed mode should prioritize active-workspace tab matches before non-tab node matches.
4. In detail mode, selecting a match should open/focus the matched node tab/pane (without creating duplicate node entries).
5. Ensure `@` mode exits cleanly when query is cleared or no matches exist.
6. Validate focus transitions across graph/detail modes and multi-pane layouts.

Done criteria:

1. Enter cycling is stable and repeatable for the same query.
2. Counter always matches internal match list length/index.
3. `@t` returns only active-workspace tab matches; `@T` can return saved-workspace tab matches.
4. `@n` returns only active-graph-context node matches; `@N` can return saved-graph node matches.
5. `@e` returns only active-graph-context edge matches with searchable payload; `@E` can return saved-graph edge matches.
6. In detail/workbench, default mixed mode returns active tab matches before non-tab node matches.
7. Detail-mode selection always targets the matched node/tab, not unrelated focused panes.
8. In graph mode, selecting an edge search result selects/highlights that edge deterministically.
9. No stale search-session state after clearing query or switching mode.

Headed validation checklist (required):

1. In graph mode, type `@term` and press Enter repeatedly: active match cycles through all results and wraps.
2. In detail mode with multiple panes, type `@term` and press Enter: each press focuses/opens the matched node in the correct pane/tab context.
3. In detail mode, run `@t term`: only currently open tab/pane-backed nodes are cycled.
4. In detail mode, run `@T term`: active and saved workspace tab matches are cycled deterministically.
5. In graph mode, run `@n term`: only active-graph-context matches are cycled.
6. In graph mode, run `@N term`: active + saved graph matches are cycled deterministically.
7. In graph mode, run `@e term`: only active-graph searchable edge matches are cycled and selected.
8. In graph mode, run `@E term`: active + saved-graph searchable edge matches are cycled deterministically.
9. Clear query after cycling: counter/session reset and normal URL submit behavior resumes.
10. Switch graph <-> detail while query is active: no panic, no stale focus target, no incorrect pane navigation.

### Step 5a: Layout Stability Tuning

Work:

1. Bias new-node spawn near source/focused context instead of hardcoded center.
2. Keep short-range separation while reducing long-range runaway spread.
3. Preserve anti-overlap behavior and deterministic positioning.

Done criteria:

1. New nodes appear near relevant context in normal navigation flows.
2. Layout no longer explodes outward immediately on common node creation flows.
3. Existing physics controls/tests remain valid.

### Step 5b: Multi-Node Open Workflows

Work:

1. Use graph neighbors (`in_neighbors` + `out_neighbors`) to support open-connected workflows.
2. Default to opening connected nodes as tabs in detail view.

Done criteria:

1. Connected-node expansion is available as explicit command.
2. Opened panes/tabs map cleanly to existing tile runtime semantics.

### Step 6: Workspace Pinning (Tile Snapshot Track)

Work:

1. Add optional persistence for tile/workspace layout keyed by stable node identity.
2. Keep workspace snapshot storage separate from core graph log semantics.

Done criteria:

1. User can pin and restore workspace layout independently of graph data.
2. Failure to restore workspace does not affect graph integrity.

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
| Command palette UI | **Not implemented** (design only) | — |
| Command registry pattern | **Not implemented** — inline match dispatch | `tile_behavior.rs`, `render/mod.rs:286`, `input/mod.rs:95` |
| Bulk edge operations (N > 2) | **Not implemented** | — |
| Persistence replay for edge create/remove | **Implemented** — `LogEntry::AddEdge`, `LogEntry::RemoveEdge` with `PersistedEdgeType` | `app.rs:651-700`, `persistence/mod.rs`, `persistence/types.rs` |

### Trigger Matrix vs Code Truth

| Trigger | Plan Description | Code Reality | Discrepancy |
| --- | --- | --- | --- |
| Split-open | `from = previous selected node, to = target` | `from = selected_nodes.primary(), to = key` from `FocusNodeSplit(key)` | **Match** |
| Drag-into-same-tab-group | `from = dragged pane node, to = first existing node in destination tabs container` | `from = moved_node, to = first peer in new group` | **Match** |
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
