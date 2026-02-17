# Persistence UX Plan (2026-02-12)

## Persistence UX Plan
- Scope: Add explicit user control to clear saved graph data and improve testability for recovery paths.
- Feature target 1: Add app/store APIs to wipe persistence state safely.
- Feature target 2: Expose a confirmed UI action for clearing graph + persisted data.
- Feature target 3: Validate behavior with unit tests at persistence and app layers.

## Findings
- Current persistence behavior is automatic (startup recovery + periodic snapshots + shutdown snapshot), but there was no explicit in-app reset for users.
- `GraphBrowserApp::new_from_dir(...)` now enables fixture-based startup testing from controlled storage paths.
- Reset behavior should clear both redb snapshot state and fjall mutation log to avoid stale recovery.

## Progress
- Added `GraphStore::clear_all()` to wipe snapshot + log.
- Added `GraphBrowserApp::clear_graph_and_persistence()` to reset runtime and persisted state.
- Added toolbar action (`Clr`) with confirmation dialog in `desktop/gui.rs`.
- Added/updated tests for store wipe and app-level persistence reset.
- Wired keyboard delete/clear handling in `gui.rs` through `input::collect_actions()` + `input::apply_actions()` so webviews are closed before node/graph deletion.
- `close_webviews_for_nodes(...)` is now actively used by the delete-selected path.
