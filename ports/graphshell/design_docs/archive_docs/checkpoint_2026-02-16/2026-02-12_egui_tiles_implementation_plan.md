# egui_tiles Implementation Plan (2026-02-12)

## egui_tiles Plan

### Phase Dependency Graph

- Phase 5 -> Phase 6 -> Phase 7 -> Phase 8
- Phase 5 is a hard gate for Phase 6. Do not start per-pane rendering-context work until state ownership is finalized.
- Phase 6 is a hard gate for Phase 7. Do not harden lifecycle/favicon tabs until the rendering model is finalized.
- Phase 7 is a hard gate for Phase 8 integration tests and persistence finalization.

### Non-Goals (Current Cycle)

- No redesign of graph physics, force parameters, or selection semantics.
- No UI theme/visual overhaul outside tile/tab behavior needed for egui_tiles migration.
- No broad Servo rendering architecture rewrite beyond the minimal `sibling_context()` addition.
- No cross-platform performance tuning beyond correctness and basic sanity checks.

### Completed Phases (1-4)

- Phase 1: Add dependency and compile-safe scaffolding (`TileKind`, initial `Behavior`, tree initialization). **Done.**
- Phase 2: Wire graph pane rendering through `Behavior::pane_ui` while preserving existing detail-view behavior. **Done.**
- Phase 3: Introduce WebView pane flow (`TileKind::WebView(NodeKey)`) and tab title plumbing from graph state. **Done.**
- Phase 4: Tile creation/focus, per-pane lifecycle, single-active composite. **Done (partial - single-active only).**

### Remaining Phases (5-8)

#### Phase 5: Tile Tree as Single Source of Truth

- Retire `View::Detail(NodeKey)`. The tile tree becomes the authority for what's visible.
- "Graph view" = only the Graph pane visible (no WebView tiles). "Detail" = WebView tile(s) present.
- Remove `app.view` enum; derive view state from tile tree contents.
- The toolbar view-toggle button manipulates tile visibility instead of setting a View enum.
- Add formal invariant assertions: every `TileKind::WebView(nk)` must have a corresponding webview mapping and rendering context; violations logged in debug mode.

Acceptance criteria:

- `app.view` no longer drives lifecycle, input gating, or render branching for headed desktop mode.
- A single derived-state helper fully determines:
  - graph-only mode (no WebView panes),
  - mixed/tiled mode (one or more WebView panes).
- Invariant checks run every frame in debug builds and emit actionable logs on desync.
- Existing targeted tests continue passing.

#### Phase 6: Per-Pane Rendering Contexts

- Add `sibling_context()` to `OffscreenRenderingContext` (3-line Servo core change in `components/shared/paint/rendering_context.rs`).
- Add `create_toplevel_webview_with_context()` to `ServoShellWindow` (accepts custom `Rc<dyn RenderingContext>`).
- Store `tile_rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>>` in `Gui`.
- Each WebView tile gets its own offscreen framebuffer, Painter, and WebRender instance.
- Per-tile paint loop: iterate visible WebView tiles, resize context to tile rect, `webview.paint()`, blit via `render_to_parent_callback()`.
- Per-tile viewport sizing: resize context + webview when tile rect changes.
- Input routing: track tile rects per frame; route pointer events only to the tile under cursor; keyboard to focused tile's webview.
- Fallback: if multi-context proves problematic, degrade to single-active with placeholder tiles.

Acceptance criteria:

- Two visible WebView tiles can be painted in the same frame with distinct contexts.
- Resizing either tile updates only that tile's webview/context viewport.
- Pointer/keyboard routing targets the intended tile/webview without cross-pane bleed.
- No regression in detail-only mode behavior.

Rollback criteria for `sibling_context()` path:

- If context creation fails or causes instability on any supported desktop target:
  - keep `sibling_context()` behind a guarded call site,
  - fall back to current single-active compositing path,
  - leave tile lifecycle/state changes intact so migration progress is preserved.

#### Phase 7: Lifecycle Hardening, Navigation, and Favicon Tabs

- Lifecycle:
  - Last WebView tile close: clean up all tile rendering contexts, return to graph-only state.
  - Stale NodeKey prune: if a graph node is deleted while its tile is open, auto-close the tile.
  - Navigation state: after `sync_to_graph()`, update `TileKind::WebView(old_key)` to `WebView(new_key)` when webview navigates; move rendering context to new key.
- Favicon tabs:
  - Override `tab_ui()` in `GraphshellTileBehavior` to render favicon (16x16) + truncated title + close button.
  - Source favicons from graph node `favicon_rgba` data (already persisted).
  - Cache favicon textures per NodeKey.

Acceptance criteria:

- Closing last WebView tile returns to graph-only state with no stale mappings or contexts.
- Deleting a node with open tile closes tile and cleans mappings/contexts in the same frame.
- URL navigation that remaps node identity updates tile pane key without orphaning context.
- Tabs show title + favicon for nodes with favicon data.

#### Phase 8: Tile Layout Persistence and Integration Tests

- Persistence schema (design this early, implement here):
  - Enable `egui_tiles/serde` feature; add `serde` derives to `TileKind`.
  - Serialize `Tree<TileKind>` to JSON; store in a separate redb table `"tile_layout"`.
  - On restore: deserialize tree, prune tiles whose NodeKey no longer exists in graph.
  - Migration behavior: if no tile layout exists, initialize with single Graph pane (current default).
- Integration tests (tile tree tests, no Servo dependency):
  - `test_open_webview_tile_creates_tabs_container`
  - `test_close_last_webview_tile_leaves_graph_only`
  - `test_open_duplicate_tile_focuses_existing`
  - `test_stale_node_cleanup_removes_tile`
  - `test_navigation_updates_tile_pane`
  - `test_tile_layout_serde_roundtrip`
  - `test_all_webview_tile_nodes_tracks_correctly`
  - `test_invariant_check_detects_desync`

Acceptance criteria:

- Tile layout persists and restores across restart when data exists.
- Restore path prunes stale NodeKeys deterministically.
- All listed integration tests pass in CI for graphshell crate.
- Default bootstrap remains single Graph pane when no persisted layout exists.

### Key Files Modified

| File | Changes |
| ---- | ------- |
| `components/shared/paint/rendering_context.rs` | Add `sibling_context()` |
| `ports/graphshell/desktop/gui.rs` | Retire View enum usage, per-tile context HashMap, paint loop, input routing, invariant checks |
| `ports/graphshell/desktop/tile_behavior.rs` | `tab_ui()` override with favicons |
| `ports/graphshell/desktop/tile_kind.rs` | Add serde derives |
| `ports/graphshell/window.rs` | Add `create_toplevel_webview_with_context()` |
| `ports/graphshell/desktop/webview_controller.rs` | Remove `app.view` dependency, derive state from tiles |
| `ports/graphshell/app.rs` | Remove `View` enum or reduce to derived state |
| `ports/graphshell/persistence/types.rs` | Tile layout persistence type |
| `ports/graphshell/Cargo.toml` | egui_tiles serde feature |

## Findings

- Design docs consistently converge on egui_tiles as the chosen architecture:
  - `2026-02-12_architecture_reconciliation.md`
  - `2026-02-12_servoshell_inheritance_analysis.md`
  - `2026-02-12_egui_tiles_implementation_guide.md`
- Current favicon/thumbnail vertical slice remains compatible; most migration impact is in `desktop/gui.rs` layout/event orchestration.
- `render::render_graph` was `CentralPanel`-bound, so pane integration required extracting a `Ui`-scoped graph renderer for `egui_tiles::Behavior::pane_ui`.
- `GraphAction::FocusNode` can be intercepted in tile behavior, allowing graph double-click to open/focus a `WebView(NodeKey)` tile while other graph actions continue through shared action application.
- WebView lifecycle and input gating were previously keyed only to `app.view`; that conflicts with graph-mode WebView tiles and must be made tile-aware before pane rendering can work.
- Multi-pane rendering investigation (Feb 12):
  - Servo supports multiple Painters with independent RenderingContexts natively (`Paint.painters: Vec<Rc<RefCell<Painter>>>`).
  - `OffscreenRenderingContext` stores `parent_context: Rc<WindowRenderingContext>` and its own GL framebuffer. Creating siblings is trivial.
  - `WebViewBuilder::new(servo, rendering_context)` accepts any `Rc<dyn RenderingContext>` - each webview can target a different offscreen context.
  - `webview.paint()` then `Paint::render(webview_id)` then finds Painter by PainterId then renders into that Painter's context.
  - `render_to_parent_callback()` blits from any offscreen framebuffer to an arbitrary rect in the parent window.
  - Conclusion: Per-pane rendering is architecturally sound. One 3-line addition (`sibling_context()`) to Servo core; everything else is in graphshell.
  - Performance note: Each visible pane gets its own WebRender instance. For 2-5 panes this is acceptable.
- View enum conflict (Feb 12):
  - `app.view` (Graph vs Detail) and tile tree are parallel representations that can desync.
  - Three maps must stay synchronized: tile tree panes, webview-node mappings, rendering contexts.
  - Resolution: retire `View::Detail`, derive view state from tile tree presence of WebView tiles.

## Progress

### Phase 1-2 (Complete)

- Added `egui_tiles` dependency, `tile_kind.rs`, `tile_behavior.rs`, `Tree<TileKind>` init.
- Extracted `render::render_graph_in_ui(ui, app)` for pane-safe rendering.
- Routed graph-mode rendering through `tiles_tree.ui(&mut behavior, ui)`.
- Validation: `cargo check` and `cargo test` pass.

### Phase 3 (Complete)

- `render_graph_in_ui_collect_actions()` + `FocusNode` interception in behavior.
- Tile open/focus: `open_or_focus_webview_tile()`, root auto-promotes to Tabs container.
- Tab behavior: Graph non-closable, WebView closable.
- Deviation: tile-driven lifecycle/input gating (`preserve_webviews_in_graph` flag).
- Validation: `test_focus_node_action`, `test_toggle_view_from_graph_to_detail` pass.

### Phase 4 (Complete - single-active)

- Behavior reports active WebView nodes, pending closes each frame.
- Graph-mode: ensures webviews for active tiles, activates in WebViewCollection, composites via `render_to_parent_callback` into tile rect.
- Lifecycle: closes webviews on tile close, prunes stale mappings, ensures webviews for all active tiles.
- Helpers: `active_webview_tile_rect()`, `webview_tile_rect_for_node()`, `all_webview_tile_nodes()`, `ensure_webview_for_node()`, `close_webview_for_node()`.
- Validation: all tests pass.

### Phases 5-8 (In progress)

- Plan written Feb 12. Ready to implement.
- Phase 5 started (tile tree as source for headed runtime mode):
  - `Gui::is_graph_view()` now derives from tile-tree presence of `WebView` panes.
  - Keyboard toggle (`Home`/`Esc` path via `KeyboardActions.toggle_view`) now uses tile toggle flow in `Gui` instead of directly mutating `app.view`.
  - Toolbar toggle button now uses tile-driven mode state and tile toggle action.
  - Added helpers in `Gui`:
    - `has_any_webview_tiles()` / `has_any_webview_tiles_in(...)`
    - `toggle_tile_view(...)`
    - `preferred_detail_node(...)`
    - `sync_legacy_view_from_tiles(...)` (compatibility shim while `app.view` still exists).
  - Updated graph input/address behavior checks to use tile-derived mode where appropriate.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_from_graph_to_detail` passes.
- Phase 5 continued (controller decoupling from `app.view`):
  - `webview_controller::manage_lifecycle(...)` now takes explicit tile-derived inputs:
    - `has_webview_tiles: bool`
    - `active_webview_node: Option<NodeKey>`
  - Removed internal `app.view` branching from lifecycle management.
  - GUI now passes tile-derived active webview node via `active_webview_tile_node(...)`.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_from_graph_to_detail` passes.
- Phase 5 continued (GUI branch cleanup):
  - Removed remaining direct `app.view` branch in headed tab-strip gating.
  - Tab-strip visibility now derives solely from tile state (`has_webview_tiles`).
  - GUI runtime checks no longer depend on `View::Graph`/`View::Detail`; only compatibility shim assignments remain in `sync_legacy_view_from_tiles(...)`.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_from_graph_to_detail` passes.
- Phase 5 continued (legacy shim narrowed):
  - `sync_legacy_view_from_tiles(...)` now:
    - derives detail node strictly from tile tree (`active_webview_tile_node` then `first_webview_tile_node`),
    - only writes `graph_app.view` when desired value differs.
  - Removed selection-based fallback from shim path.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_from_graph_to_detail` passes.
- Phase 5 continued (single legacy writer policy):
  - Removed direct `graph_app.view` writes from `toggle_tile_view(...)`.
  - `sync_legacy_view_from_tiles(...)` is now the only compatibility writer for `app.view` in `gui.rs`.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_from_graph_to_detail` passes.
- Phase 5 continued (input/render decoupling from `app.view`):
  - `render::draw_graph_info` no longer reads `app.view`; view label removed from overlay text.
  - `input::apply_actions` no longer mutates `app.view` via `app.toggle_view()`; GUI tile logic owns view toggling.
  - Updated input test:
    - `test_toggle_view_action` -> `test_toggle_view_action_is_gui_owned`.
  - Added temporary `#[allow(dead_code)]` on `GraphBrowserApp::toggle_view` while compatibility tests remain.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_toggle_view_action_is_gui_owned` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
- Phase 5 continued (address bar tile-driven behavior):
  - In graph mode, address-bar submit no longer calls `app.focus_node(...)` (legacy view mutation path).
  - Submit now keeps node selection in app state and relies on GUI tile flow to open/focus the corresponding `WebView` tile.
  - GUI now opens/focuses selected node tile after successful graph-mode submit.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_action_is_gui_owned` passes.
- Phase 5 continued (graph action decoupling from `View` mutation):
  - `render::apply_graph_actions` no longer calls `app.focus_node(...)` for `GraphAction::FocusNode`; it now selects node state only.
  - Updated graph overlay hint text: `Double-click: Focus` -> `Double-click: Select/Open`.
  - Updated `render` test `test_focus_node_action` to assert selection semantics instead of `View::Detail`.
  - Added temporary `#[allow(dead_code)]` on `GraphBrowserApp::focus_node` for compatibility tests.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell test_toggle_view_action_is_gui_owned` passes.
- Phase 5 finalized (legacy view path removed):
  - Removed `View` enum and `GraphBrowserApp.view` field from `app.rs`.
  - Removed legacy methods `GraphBrowserApp::toggle_view` and `GraphBrowserApp::focus_node`.
  - Removed GUI compatibility shim `sync_legacy_view_from_tiles(...)` and all remaining writes to `app.view`.
  - Updated impacted tests to assert tile/selection behavior (not legacy view state).
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Phase 6 started (multi-pane paint loop in graph mode):
  - Replaced single-active tile compositing in `desktop/gui.rs` with per-visible-tile compositing:
    - derive visible tile set from `active_webview_tile_rects(...)`,
    - ensure webview/context for each visible tile,
    - resize per-tile `OffscreenRenderingContext` and `WebView` to tile rect,
    - paint each webview and blit each tile via its own `render_to_parent_callback()`.
  - Removed obsolete single-active plumbing:
    - deleted `active_webview_tile_rect(...)`,
    - deleted `webview_tile_rect_for_node(...)`,
    - removed `active_webview_nodes` tracking from `tile_behavior.rs`.
  - Derivation applied:
    - switched to rect-driven visible tile enumeration as the single source of truth (instead of behavior-side active-node bookkeeping), which keeps lifecycle/paint alignment tighter for split layouts.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell test_focus_node_action` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Phase 6 continued (tile-aware input routing):
  - Added tile hit-testing helpers in `desktop/gui.rs`:
    - `webview_at_point(...) -> Option<(WebViewId, local_point)>`
    - `focused_webview_id()`
  - Updated `desktop/headed_window.rs` event routing:
    - pointer events route to the WebView tile under cursor (if any),
    - keyboard/IME route to focused WebView tile (by activating its webview id),
    - egui forwarding now uses tile hit-test (`webview_at_point().is_none()`) instead of toolbar-only gating.
  - Removed obsolete helper paths after routing change.
  - Derivation applied:
    - use GUI-owned tile hit-testing as the single routing oracle so paint/layout and input targeting cannot diverge.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Phase 7 started (lifecycle hardening: stale-key prune + navigation remap):
  - `webview_controller::sync_to_graph(...)` now returns `SyncToGraphResult` with `remapped_nodes`.
  - GUI now applies navigation remaps to tile keys and rendering contexts:
    - updates `TileKind::WebView(old)` -> `WebView(new)` when navigation remaps node identity,
    - if destination tile already exists, source tile is removed (no duplicate panes),
    - per-node rendering context is moved from old key to new key when needed.
  - Added stale tile pruning in GUI:
    - each frame prunes `WebView` tiles whose `NodeKey` no longer exists in graph,
    - closes any mapped webview/context for pruned keys.
  - Added explicit WebView tile cleanup on graph clear / clear-data flows.
  - Derivation applied:
    - keep remap reconciliation in GUI (tile-owner) rather than persistence/controller layer, so tile-tree invariants and context-map invariants are enforced in one place.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Derivation update (merge-conflict minimization):
  - Reverted the temporary Servo-core `sibling_context()` approach.
  - GraphShell now creates per-tile offscreen contexts by plumbing `Rc<WindowRenderingContext>` into GUI and calling existing `WindowRenderingContext::offscreen_context(...)`.
  - Net effect: no GraphShell-specific edits remain in `components/shared/paint/rendering_context.rs`.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Phase 7 continued (favicon tabs in egui_tiles):
  - Implemented `Behavior::tab_ui(...)` override in `desktop/tile_behavior.rs`:
    - renders favicon (16x16) + truncated title + close button for `WebView` panes,
    - preserves tab drag/click behavior from egui_tiles default tab interaction model.
  - Added persistent favicon texture cache per `NodeKey` in `desktop/gui.rs`:
    - cache lives across frames (`tile_favicon_textures`),
    - cache is pruned when nodes are removed,
    - cache entries are moved on node remap (navigation old->new key).
  - Derivation applied:
    - cache by `NodeKey` (not `WebViewId`) to stay consistent with tile identity and remap semantics.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`139 passed, 0 failed`).
- Phase 8 started (tile layout persistence):
  - Enabled `egui_tiles` serde support in `Cargo.toml` (`features = ["serde"]`).
  - Added serde derives to `desktop/tile_kind.rs` (`TileKind` now serializable/deserializable).
  - Added tile-layout storage APIs to `persistence::GraphStore`:
    - `save_tile_layout_json(...)`
    - `load_tile_layout_json(...)`
    - `clear_all()` now removes saved tile layout as well.
  - Added app passthrough APIs:
    - `GraphBrowserApp::save_tile_layout_json(...)`
    - `GraphBrowserApp::load_tile_layout_json(...)`
  - Wired GUI restore/save:
    - On startup, `Gui::new` attempts to restore `Tree<TileKind>` from persisted JSON.
    - Restore path prunes stale `WebView(NodeKey)` panes whose nodes no longer exist.
    - On shutdown (`Drop`), GUI serializes and saves current tile tree before graph snapshot.
  - Added tests in `persistence/mod.rs`:
    - `test_tile_layout_roundtrip`
    - `test_clear_all_removes_tile_layout`
  - Derivation applied:
    - Persisted tile layout as JSON string in persistence layer, while keeping type-aware parsing/pruning in GUI. This avoids pulling egui tile types into core app/persistence APIs.
  - Validation:
    - `cargo check -p graphshell` passes.
    - `cargo test -p graphshell --lib` passes (`141 passed, 0 failed`).
- Earmarked follow-up (persistence configurability):
  - Fixed persistence components should become user/runtime configurable:
    - snapshot interval/cadence,
    - backend/table policy knobs where practical.
  - Data directory should be configurable at runtime via explicit user-facing controls (not only constructor/CLI plumbing).
- Phase 8 continued (tile-tree integration tests):
  - Added GUI tile-tree tests (no Servo runtime dependency) in `desktop/gui.rs`:
    - `test_open_webview_tile_creates_tabs_container`
    - `test_close_last_webview_tile_leaves_graph_only`
    - `test_open_duplicate_tile_focuses_existing`
    - `test_stale_node_cleanup_removes_tile`
    - `test_navigation_updates_tile_pane`
    - `test_navigation_remap_deduplicates_when_target_exists`
    - `test_tile_layout_serde_roundtrip`
    - `test_all_webview_tile_nodes_tracks_correctly`
  - Validation:
    - `cargo test -p graphshell --lib` passes (`149 passed, 0 failed`).
- Runtime persistence directory switching (implemented):
  - Added toolbar `Dir` action in `desktop/gui.rs` with a path-entry dialog.
  - Switching data directory now:
    - closes active webviews,
    - clears tile/webview/favicon transient caches,
    - reopens persistence via `GraphBrowserApp::switch_persistence_dir(...)`,
    - restores persisted tile layout from the selected directory with stale-NodeKey pruning.
  - Added test:
    - `app::tests::test_switch_persistence_dir_reloads_graph_state`
  - Validation:
    - `cargo test -p graphshell --lib` passes (`150 passed, 0 failed`).
- Earmark retained:
  - Snapshot cadence and other fixed persistence knobs should be surfaced as user/runtime-configurable settings.
- Invariant checks implemented (Phase 5 acceptance closure):
  - Added debug-build invariant validation in `desktop/gui.rs`:
    - each `TileKind::WebView(NodeKey)` must resolve to an existing graph node,
    - each such node must have a webview mapping,
    - each such node must have a rendering context entry.
  - Violations are emitted as actionable warnings each frame in debug builds.
  - Added integration-style unit test:
    - `desktop::gui::tests::test_invariant_check_detects_desync`
- Persistence knobs made user-configurable (no prerequisites required):
  - Added runtime persistence settings UI (`Cfg` toolbar action):
    - configurable snapshot interval in seconds (applies immediately),
    - validation + status feedback in dialog.
  - Added CLI/config plumbing:
    - new flag `--graph-snapshot-interval-secs=<u64>`
    - parsed into `ServoShellPreferences.graph_snapshot_interval_secs`.
  - Added persistence/app APIs:
    - `GraphStore::set_snapshot_interval_secs(...)`
    - `GraphStore::snapshot_interval_secs()`
    - `GraphBrowserApp::set_snapshot_interval_secs(...)`
    - `GraphBrowserApp::snapshot_interval_secs()`
  - Added tests:
    - `persistence::tests::test_set_snapshot_interval_secs`
    - `persistence::tests::test_set_snapshot_interval_secs_rejects_zero`
    - `app::tests::test_set_snapshot_interval_secs_updates_store`
    - `app::tests::test_set_snapshot_interval_secs_without_persistence_fails`
    - CLI parse coverage extended in `prefs::test_servoshell_cmd`.
  - Validation:
    - `cargo test -p graphshell --lib` passes (`155 passed, 0 failed`).

