# Feature Priority and Dependency Plan (2026-02-17)

**Status (2026-02-20): All F1–F7 features are complete. This plan is a historical record.**
Follow-on work lives in the 2026-02-18+ plans (edge operations, persistence hub, badge/tagging,
edge traversal, settings architecture, etc.).

## Purpose

This replaced day-by-day sequencing with feature-priority sequencing.
Each feature is gated by explicit dependencies and exit criteria.

## Priority Model

- **P0**: Highest user-visible value and architecture-risk reduction.
- **P1**: Important correctness/spec-alignment work that depends on P0 or can run in parallel.
- **P2**: Medium-term cleanup and extensibility.

## Dependency Graph

1. **F2 Remove Global-Active Authority Leakage** -> prerequisite for clean F1 routing.
2. **F1 Multi-WebView Visible Panes** -> validated by F2 routing fixes.
3. **F3 Crash Docs Status Alignment** -> independent (can run anytime).
4. **F4 Source-of-Truth Spec Alignment** -> independent (can run anytime).
5. **F5 UserGrouped Edge Semantics** -> depends on F1 interaction model clarity.
6. **F6 EGL/WebDriver Explicit Targeting** -> important but out of focus this cycle; scope note required.
7. **F7 GUI Decomposition** -> depends on F1/F2 behavior being stable to avoid moving targets.

---

## P0 Features

### F2: Remove Remaining Global-Active Authority Leakage

- **Goal**: Ensure global active webview is input hint only, not routing/render authority.
- **Status**: Implemented for current desktop scope (Feb 17).
- **Depends on**: None (implementation can proceed with single-pane model).
- **Gates**:
  - Focus switch between tiles does not hide or de-prioritize non-focused visible webviews.
  - Back/forward/reload target focused tile webview only, resolving target via `egui_tiles` tree state.
  - Dialog association no longer depends on window-global active where tile focus is available.
- **Implementation shape**:
  - **Add**: `focused_tile_webview_id()` to `Gui` struct.
  - **Refactor**: update `HeadedWindow` to delegate `preferred_input_webview_id` to `gui.focused_tile_webview_id()` when running in graphshell mode.
  - **Refactor**: dialog and routing lookups in `headed_window.rs` to use focused tile webview ID where possible.
  - **Delete**: non-input decisions keyed from `active_id()` in desktop tile flow.
- **Primary files**:
  - `ports/graphshell/desktop/headed_window.rs`
  - `ports/graphshell/desktop/gui.rs`
  - `ports/graphshell/window.rs`
- **Implemented notes**:
  - Desktop preferred-input routing resolves from tile-focused webview ID.
  - Dialog update path uses focused tile webview targeting in GUI-driven flow.
  - Global active ID is no longer used as desktop tile-flow authority for navigation/reconciliation targeting.
  - Remaining fallback/global-active usage is retained only for non-desktop or generic scaffold behavior outside this cycle's desktop scope.

### F1: Multi-WebView Simultaneous Visibility

- **Goal**: Make opening/viewing webviews support split-pane visibility (`Linear` containers), not only tabs.
- **Status**: Implemented for current desktop scope (Feb 17): split panes, focused-pane routing (omnibar + toolbar), frame activation focus retention, close-to-cold semantics, and baseline headed validation pass.
- **Depends on**: F2 (soft dependency for routing correctness, hard dependency for clean implementation).
- **Gates (must pass before next dependent features)**:
  - Two webview panes can be visible and painted in one frame.
  - Opening a node can choose tab-focus vs split-open semantics.
  - Existing tab behavior remains intact.
- **Implementation shape**:
  - **Refactor**: extend tile open helpers in `desktop/gui.rs` to support both `Container::Tabs` and `Container::Linear`.
  - **Refactor**: verify and align `repaint_webviews()` in `window.rs` with multi-pane repaint expectations, while preserving tile-driven compositing in `gui.rs`.
  - **Add**: explicit split-open actions (toolbar + graph action plumbing).
  - **Delete**: implicit assumption that every open action should focus a tabs container.
- **Primary files**:
  - `ports/graphshell/desktop/gui.rs`
  - `ports/graphshell/desktop/tile_behavior.rs`
  - `ports/graphshell/window.rs`
- **Validation checklist**:
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-17_f1_multi_pane_validation_checklist.md`

---

## P1 Features

### F3: Crash Handling Status Alignment in Docs

- **Goal**: Align documentation with current desktop implementation status.
- **Status**: Implemented (doc alignment pass complete).
- **Depends on**: None.
- **Gates**:
  - No contradictory "pending" crash-handling status remains for desktop path.
  - Crash policy doc and concerns doc agree on current state plus known limits.
- **Implementation shape**:
  - **Refactor**: status language only.
  - **Add**: explicit note separating desktop implementation from upstream accessibility/API blockers.
  - **Delete**: stale "implementation pending" text for already-landed pieces.
- **Primary files**:
  - `ports/graphshell/design_docs/graphshell_docs/ARCHITECTURAL_CONCERNS.md`
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-16_architecture_and_navigation_plan.md`

### F4: Source-of-Truth Spec Alignment

- **Goal**: Remove contradiction between browser behavior spec, architectural overview, and architecture plan.
- **Status**: Implemented (doc alignment pass complete).
- **Depends on**: None.
- **Gates**:
  - Browser spec uses three-domain authority model (graph/tile/webview runtime).
  - Architectural overview vision language uses the same three-domain model.
  - Terminology matches architecture plan across all docs.
- **Implementation shape**:
  - **Refactor**: "single source of truth = webview set" language in browser spec and architectural overview.
  - **Add**: explicit authority-table language aligned to current architecture.
  - **Delete**: contradictory invariants.
- **Primary files**:
  - `ports/graphshell/design_docs/graphshell_docs/GRAPHSHELL_AS_BROWSER.md`
  - `ports/graphshell/design_docs/graphshell_docs/ARCHITECTURAL_OVERVIEW.md`

### F5: UserGrouped Edge Semantics (Implemented for explicit split-open gesture)

- **Goal**: Define and implement when `UserGrouped` edges are created.
- **Status**: Implemented (initial deterministic trigger).
- **Depends on**: F1 (pane/group interaction behavior clarity).
- **Gates**:
  - Edge creation triggers are deterministic and documented.
  - Explicit grouping action creates expected `UserGrouped` edges exactly once.
  - Tests cover edge creation and non-creation paths.
- **Implementation shape**:
  - **Refactor**: split-open grouping action into explicit intent.
  - **Add**: `UserGrouped` variant to `EdgeType` in `graph/mod.rs` and persistence types.
    *(Note 2026-02-20: `EdgeType` is being replaced by `EdgePayload` per the edge traversal
    plan. The `UserGrouped` concept survives as `EdgePayload { user_asserted: true }`. The
    F5 follow-on work in `2026-02-18_edge_operations_and_radial_palette_plan.md` owns the
    migration.)*
  - **Add**: reducer behavior for `UserGrouped` edges in `app.rs`.
  - **Add**: deterministic first trigger: `Shift + Double-click` graph action (`FocusNodeSplit`) emits `CreateUserGroupedEdge { from: previous_selection, to: target }`.
  - **Delete**: ambiguous "planned/not yet implemented" behavior where implementation is complete for the explicit split-open path.
- **Primary files**:
  - `ports/graphshell/app.rs`
  - `ports/graphshell/graph/mod.rs`
  - `ports/graphshell/desktop/tile_behavior.rs`
  - `ports/graphshell/desktop/gui.rs`
- **Follow-on UX/operations plan**:
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-18_edge_operations_and_radial_palette_plan.md`
  - Covers explicit edge create/remove command surface, radial command model, and multi-select-based simplification.

---

## P2 Features

### F6: EGL/WebDriver Explicit Targeting Semantics

- **Goal**: Converge EGL/WebDriver routing on explicit webview targeting while preserving Servo/servoshell compatibility.
- **Status**: Implemented for scoped cycle (explicit EGL `_for_webview` overloads + centralized fallback warning landed); structural single-window/single-active obviation is deferred to follow-on plan.
- **Depends on**: Scope decision checkpoint.
- **Gate 0 (decision gate)**:
  - **Decision Made**: Desktop-only scope for this cycle.
- **Gates**:
  - Local-first implementation path is exhausted before proposing upstream changes.
  - Any upstream ask is backed by reproduced hard gaps and minimal additive API proposals.
  - Fallback-to-active/newest semantics are isolated to explicit compatibility boundaries.
- **Implementation shape**:
  - **Add**: dedicated F6 phased plan with explicit critique checklist and escalation gate.
  - **Refactor**: EGL/WebDriver routing toward explicit ID-targeted helpers behind compatibility wrappers.
  - **Delete**: distributed ad hoc fallback targeting where graphshell owns call paths.
- **Primary files**:
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-16_architecture_and_navigation_plan.md`
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-18_f6_explicit_targeting_plan.md`
  - `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/2026-02-18_single_window_active_obviation_plan.md` (deferred follow-on inventory)

### F7: GUI Decomposition for Maintainability

- **Goal**: Reduce monolithic complexity in `gui.rs` (~2,840 lines as of Feb 17) without behavior change.
- **Status**: Implemented for current desktop scope (Feb 18): frame pre-ingest/apply + keyboard action + toolbar/dialog + lifecycle/post-render orchestration in `desktop/gui_frame.rs`, tile-group move detection in `desktop/tile_grouping.rs`, navigation target resolution in `desktop/nav_targeting.rs`, semantic-event to intent pipeline in `desktop/semantic_event_pipeline.rs`, thumbnail/favicon pipeline in `desktop/thumbnail_pipeline.rs`, webview status/toolbar sync in `desktop/webview_status_sync.rs`, webview creation backpressure/probe logic in `desktop/webview_backpressure.rs`, toolbar navigation/submit routing in `desktop/toolbar_routing.rs`, tile/webview runtime lifecycle utilities in `desktop/tile_runtime.rs`, pre-render runtime reconciliation in `desktop/lifecycle_reconcile.rs`, tile activation/compositing helpers in `desktop/tile_compositor.rs`, tile post-render behavior/reconciliation helpers in `desktop/tile_post_render.rs`, tile render/composite pass orchestration in `desktop/tile_render_pass.rs`, tile view toggle/open-mode operations in `desktop/tile_view_ops.rs`, invariant validation helpers in `desktop/tile_invariants.rs`, persistence/layout helpers in `desktop/persistence_ops.rs`, management dialog panel rendering in `desktop/dialog_panels.rs`, top toolbar/fullscreen origin strip rendering in `desktop/toolbar_ui.rs`, graph search keyboard/shortcut flow in `desktop/graph_search_flow.rs`, and graph search window rendering in `desktop/graph_search_ui.rs`, all with unchanged behavior/tests.
- **Depends on**: F1/F2 stabilized.
- **Gates**:
  - Lifecycle reconciliation, toolbar/nav submit, and compositing are extracted into focused modules.
  - Existing behavior/test outcomes remain unchanged.
- **Implementation shape**:
  - **Refactor**: move cohesive sections out of `gui.rs`.
  - **Add**: module-level tests for extracted logic.
  - **Delete**: duplicated cross-cutting logic in the monolith.
- **Primary files**:
  - `ports/graphshell/desktop/gui.rs`
  - new modules under `ports/graphshell/desktop/`
- **F7 Closeout (Before/After)**:
  - **Before**:
    - `desktop/gui.rs` contained most frame-phase orchestration inline (semantic ingest, keyboard routing, toolbar/dialog handling, lifecycle reconciliation, tile render/composite, and post-render intent application).
    - Core flows depended on monolithic local helpers, making targeted debugging and ownership boundaries harder.
  - **After**:
    - Frame orchestration is decomposed into focused modules with clear boundaries:
      - `desktop/gui_frame.rs`: phase coordination and apply boundaries.
      - `desktop/*` support modules: targeting, toolbar routing/UI, graph search flow/UI, lifecycle reconcile, tile render/composite, persistence ops, semantic event pipeline, thumbnail/favicons, and status sync.
    - `gui.rs` is now a thin orchestrator and state owner rather than the implementation locus for all subflows.
    - Behavior/test outcomes are preserved (`cargo test -p graphshell --lib` remained green throughout extraction steps).

---

## Recommended Execution Order

1. F2 (smaller scope, eliminates global-active fallback before split panes exist)
2. F1 (split panes land on a clean tile-focused routing foundation)
3. F3 + F4 (parallel, can overlap with F1/F2 engineering)
4. F5
5. F6 (tracked as important follow-up, out of focus this cycle)
6. F7

## Notes

- This plan is intentionally feature-gated, not calendar-gated.
- A dependent feature should not start until dependency gates are satisfied.
- **F2-first rationale**: F2 (~80 lines) removes the global-active fallback that F1 would otherwise inherit. Doing F2 first means F1's split-pane code must use tile-focused routing from the start, catching integration mistakes earlier. The F2->F1 dependency noted above is soft; `focused_tile_webview_id()` is already meaningful with the existing tab model and does not require split panes.

## Commit Slicing Guidance

When batching commits by plan phase, prefer this order and file grouping:

1. `F1/F2 desktop routing and pane behavior`
- `ports/graphshell/desktop/*`
- `ports/graphshell/window.rs`
- `ports/graphshell/running_app_state.rs`

2. `F5 edge semantics and persistence`
- `ports/graphshell/app.rs`
- `ports/graphshell/graph/mod.rs`
- `ports/graphshell/persistence/mod.rs`
- `ports/graphshell/persistence/types.rs`

3. `F6 explicit targeting (EGL/WebDriver scoped)`
- `ports/graphshell/egl/app.rs`
- `ports/graphshell/webdriver.rs`
- related F6 plan docs

4. `F7 GUI decomposition`
- `ports/graphshell/desktop/gui.rs`
- extracted modules under `ports/graphshell/desktop/`

5. `Doc alignment and validation artifacts`
- `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/*`
- `ports/graphshell/design_docs/graphshell_docs/INDEX.md`
- `ports/graphshell/design_docs/DOC_POLICY.md`
