# Edge Operations and Radial Command Plan (2026-02-18)

## Status

Draft (ready for implementation critique).

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
- `to = destination focused tab node` (or destination drop-target node if available).
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
|---|---|---|
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

## Open Questions for Critique

1. Should `Fully Connect Selection` be shipped now or deferred behind a feature flag?
2. Should selection order be explicitly tracked for `Chain Selection` in this phase?
3. Do we want radial-only first, or simultaneous keyboard command palette parity in Phase A?

## Decision Log

Use this section to close open questions before implementation starts.

| Date | Decision | Rationale | Owner |
|---|---|---|---|
| TBD | TBD | TBD | TBD |
