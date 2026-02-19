# Selection Semantics Plan (2026-02-14)

## Selection Semantics Plan

- Goal: eliminate duplicated selection state and make selection a single, deterministic app-level source of truth.
- Scope: `app.rs`, `graph/mod.rs`, `graph/egui_adapter.rs`, `render/mod.rs`, and impacted tests.
- Non-goal: full intent-pipeline rollout in this slice; this change prepares for that boundary.

### Phase 1: Single Source Selection

1. Replace ad-hoc selection containers with explicit `SelectionState` in `GraphBrowserApp`.
2. Remove `Node.is_selected` from the graph model.
3. Update `select_node()` to set semantics only (`HashSet` operations).
4. Update remove/clear/single-selected helpers for set semantics.

### Phase 2: Visual Projection

1. Stop reading selection from node payload in `egui_adapter`.
2. Keep adapter lifecycle color defaults only.
3. In render flow, project selection from app set onto egui nodes (`selected` + selected color).

### Phase 3: Validation

1. Update tests in `app.rs`, `graph/mod.rs`, `graph/egui_adapter.rs`, and `render/mod.rs`.
2. Run `cargo test -p graphshell --lib`.
3. Confirm no references to `Node.is_selected` remain in code.

## Findings

- Current code previously kept duplicate selection state in both `app.selected_nodes` and `Node.is_selected`.
- Adapter currently sources selection from node payload, which couples semantic model and render projection.
- Existing behavior can be preserved while removing duplication by projecting selection in render.
- Rebuilding `EguiGraphState` on selection change is sufficient for this slice and keeps the change low-risk ahead of mutation-pipeline work.

## Progress

- 2026-02-14: Plan created.
- 2026-02-14: Implemented Phase 1 + Phase 2 core changes:
  - `GraphBrowserApp.selected_nodes` migrated to `HashSet<NodeKey>`.
  - `Node.is_selected` removed from `graph/mod.rs`.
  - `EguiGraphState::from_graph` now takes app selection set and projects selection/color there.
  - Render path updated to pass selection set into adapter rebuild.
  - `select_node()` now performs set semantics and marks egui state dirty.
  - Headed GUI delete path updated to convert selection set to `Vec<NodeKey>` for webview close helper.
- 2026-02-14: Updated tests across app/graph/adapter/render to remove `Node.is_selected` assumptions.
- 2026-02-14: Validation complete: `cargo test -p graphshell --lib` passed (current suite green).
- 2026-02-14: Selection semantics hardened with explicit `SelectionState` (set + primary + monotonic revision).
  - `selected_nodes` is now a first-class state object, not a raw set.
  - Selection revision is available for deterministic cache invalidation and future event/version tracking.
