# Phase 1 Refinement Plan

**Date:** 2026-02-11
**Status:** In Progress
**Scope:** Stabilize all Phase 1 features before adding Phase 2 complexity

---

## Refinement Plan

### Why
Phase 1 is 4/5 complete (~4,500 LOC, 80 tests). Audit found 6 bugs (2 data-loss, 1 crash, 3 logic), ~30 missing tests, UI flow issues, and structural concerns in gui.rs. Fix everything before adding search/bookmarks/performance.

### Steps

| Step | Items | Scope |
|------|-------|-------|
| 1 | BUG-1 + SIMP-1 | Fix `truncate_label` UTF-8 crash, unify two truncation functions |
| 2 | BUG-2 | Add `RemoveNode`/`ClearGraph` persistence log entries (data loss fix) |
| 3 | SIMP-2 | Extract `webview_controller.rs` from gui.rs (~290 lines), move `nodes_with_webviews` to app state |
| 4 | BUG-3 + BUG-4 | Fix address bar URL not persisted + phantom node creation |
| 5 | BUG-5 + BUG-6 | Fix webview loss on rapid toggle + about:blank node spam |
| 6 | TEST-1/2/3 | Tests for app methods, new persistence variants, snapshot edge cases |
| 7 | A11Y-1 + A11Y-2 | Cold node contrast fix, increase node target size |
| 8 | UI-1/2/3 | Address bar URL sync in graph view, shutdown snapshot, Enter robustness |
| 9 | SIMP-3/4 | Selection clearing dedup, persistence db field annotation |
| 10 | TEST-4/5 | Input and render testability refactors |
| 11 | A11Y-3 | Keyboard shortcut help panel |

### Bug Details

**BUG-1 (crash):** `truncate_label` in `egui_adapter.rs:102` uses byte slicing (`&s[..n]`). Panics on multi-byte UTF-8 (CJK, emoji). Fix: char-aware truncation.

**BUG-2 (data loss):** `remove_selected_nodes()` and `clear_graph()` never call `log_mutation()`. No `RemoveNode`/`ClearGraph` log entry variants exist. Deleted nodes reappear on restart.

**BUG-3 (data loss):** Address bar URL update in graph view mutates `node.url` directly (gui.rs:784) without logging or updating `url_to_node`. Lost on restart.

**BUG-4 (phantom nodes):** After address bar URL update, `webview_previous_url` has stale mapping. Next frame's `sync_webviews_to_graph` sees URL mismatch, creates duplicate node + edge.

**BUG-5 (state loss):** `nodes_with_webviews` (gui.rs local state) saved once on graph-view entry, cleared on detail-view return. Rapid toggling loses the list permanently; only active node gets restored.

**BUG-6 (corruption):** Pressing N repeatedly creates multiple `about:blank` nodes. `url_to_node.insert()` silently overwrites prior entry. Persistence snapshot with duplicate URLs silently merges.

### Webview Controller Extraction (SIMP-2)

gui.rs is 1096 lines. Three hardest bugs live there. Extract ~290 lines into `desktop/webview_controller.rs`:

- `manage_lifecycle(app, window, state)` — save/destroy/recreate webviews on view toggle
- `sync_to_graph(app, previous_urls, window)` — URL change detection, node/edge creation, title updates
- `handle_address_bar_submit(app, url, is_graph_view, previous_urls, window)` — address bar Enter handler
- `close_webviews_for_nodes(app, nodes, window)` — absorbs the TODO stub at app.rs:470

Move `nodes_with_webviews` into `GraphBrowserApp` as `active_webview_nodes`. gui.rs drops to ~800 lines.

### Test Target
From 80 to ~110 tests. Cover all public methods on `GraphBrowserApp`, all persistence log entry variants, snapshot edge cases, and (via testability refactors) input actions and render event processing.

### Deferred to Phase 2
Command palette, trait-based graph/UI decoupling, arrow-key navigation, screen reader labels, full gui.rs controller decomposition, thumbnails/favicons.

---

## Findings

### Audit Results (pre-implementation)
- **80 tests** across 10 modules. 0 tests for: `remove_selected_nodes`, `clear_graph`, `create_new_node_near_center`, `demote/promote_node`, webview mapping methods, input actions, render events
- **gui.rs** is 1096 lines doing webview lifecycle (90 lines), sync (175 lines), toolbar, rendering, and address bar logic (25 lines) — all coupled
- **Persistence** has 4 log entry types (AddNode, AddEdge, UpdateNodeTitle, PinNode). Missing: RemoveNode, ClearGraph, UpdateNodeUrl
- **Two truncation functions** exist with different safety properties (gui.rs is char-safe, egui_adapter.rs is not)
- **Cold node contrast** ~2:1 ratio on dark background (WCAG AA requires 3:1 for non-text)
- **Node radius** 20px diameter for cold nodes (WCAG recommends 24px minimum target)
- **`take_snapshot()`** defined on GraphBrowserApp but never called (no shutdown snapshot)
- **Address bar Enter detection** uses `lost_focus() && key_pressed(Enter)` — fragile timing dependency

### External Repo Patterns (from ARCHITECTURAL_OVERVIEW.md)
- Midori Desktop: controller-style UI decomposition with narrow responsibilities
- BrowseGraph/Obsidian: command palette as primary navigation spine
- egui_node_graph2: trait-based graph model/UI separation
- Applied selectively: webview controller extraction follows Midori pattern; rest deferred

---

## Progress

### Session 1 (2026-02-11)
- Completed: Codebase audit identifying 6 bugs, ~30 test gaps, UI issues
- Completed: Plan creation and approval
- Completed: Step 1 (BUG-1 + SIMP-1) — created `util.rs` with char-safe `truncate_with_ellipsis`, deleted both old functions, 7 new tests. Fixed pre-existing test isolation bug (`new_for_testing()`).
- Completed: Step 2 (BUG-2) — added `RemoveNode`, `ClearGraph`, `UpdateNodeUrl` LogEntry variants + replay logic. Wired `log_mutation` into `remove_selected_nodes()` and `clear_graph()`. Added `Graph::update_node_url()` and `app.update_node_url_and_log()`. 8 new tests.
- Completed: Step 3 (SIMP-2) — extracted `desktop/webview_controller.rs` (299 lines) from gui.rs. Four functions: `manage_lifecycle`, `sync_to_graph`, `handle_address_bar_submit`, `close_webviews_for_nodes`. Moved `nodes_with_webviews` to `GraphBrowserApp` as `active_webview_nodes`. gui.rs: 1090 -> 801 lines.
- Completed: Step 4 (BUG-3 + BUG-4) — `handle_address_bar_submit` now uses `update_node_url_and_log` (persistence + url_to_node consistency) and pre-seeds `previous_urls` to prevent phantom nodes.
- Completed: Step 5 (BUG-5 + BUG-6) — BUG-5 fixed by SIMP-2 state move (active_webview_nodes persists in GraphBrowserApp). BUG-6 fixed with unique placeholder URLs (`about:blank#N` via counter), scan-on-recovery for collision avoidance. 2 new tests.
- Test count: 80 -> 97 (all passing)
- Completed: Step 6 (TEST-1/2/3) — 25 tests for app.rs (remove_selected_nodes, clear_graph, create_new_node, promote/demote, webview mappings, get_single_selected_node, update_node_url_and_log, placeholder uniqueness). 4 tests for graph/mod.rs (snapshot edge cases, update_node_url). Added `base` dev-dependency + `test_webview_id()` helper for PipelineNamespace init. Test count: 97 -> 120.
- Completed: Step 7 (A11Y-1 + A11Y-2) — Cold node color brightened (100,100,120 → 140,140,165) for ~3.5:1 contrast. Node radii increased: cold 10→15 (30px), active 15→18 (36px).
- Completed: Step 8 (UI-1/2/3) — Address bar now shows selected node URL in graph view. `take_snapshot()` wired to `Gui::drop()`. Enter detection uses `location_submitted` flag to decouple `has_focus()+key_pressed` from `lost_focus()`.
- Completed: Step 9 (SIMP-3/4) — `sync_to_graph` now calls `app.select_node()` instead of manual selection loop (~8 lines removed). `db` field renamed to `_db` with doc comment explaining fjall borrow requirement.
- Completed: Step 10 (TEST-4/5) — Extracted `KeyboardActions` struct + `apply_actions()` from `handle_keyboard` (8 input tests). Extracted `GraphAction` enum + `apply_graph_actions()` from `process_events` (8 render tests). Test count: 120 -> 136.
- Completed: Step 11 (A11Y-3) — Added `show_help_panel` toggle, `F1`/`?` binding, `render_help_panel()` with egui Grid listing 12 shortcuts. Updated graph overlay hint text. 1 new test.
- **Phase 1 Refinement COMPLETE.** Final test count: 137 (up from 80). 0 failures.
