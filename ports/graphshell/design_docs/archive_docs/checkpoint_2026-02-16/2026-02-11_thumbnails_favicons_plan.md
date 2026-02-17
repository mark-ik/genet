# Thumbnails And Favicons Plan (2026-02-11)

## Thumbnails And Favicons Plan
- Goal: Show recognizable node visuals using thumbnail > favicon > color fallback.
- Scope: Graphshell node rendering and data pipeline only; no external sync.
- Dependencies: Servo capture hook, egui textures, persistence updates.
- Ordering dependency: complete physics migration + selection consolidation first so FT2 lands on the stabilized render/state model.
- Phase 1: Data model
  - Add optional `thumbnail` and `favicon` handles to the node model.
  - Add metadata for source URL and last update timestamp.
- Phase 2: Favicon pipeline
  - Parse `<link rel="icon">` and fallback to `/favicon.ico`.
  - Fetch asynchronously, decode to RGBA, resize to 32x32.
  - Cache per origin and persist to disk.
- Phase 3: Thumbnail pipeline
  - Capture after load idle; downscale to 256x192.
  - Store as texture and persist compressed bytes.
  - Evict or refresh based on node lifecycle.
- Phase 4: Rendering
  - Render thumbnail if present; else favicon; else colored circle.
  - Add subtle border to indicate Active/Warm/Cold state.
- Validation tests
  - Favicon loads in under 500ms for common sites.
  - Thumbnail appears within 2 seconds without UI hitching.
  - Mixed 100-node graph maintains target FPS.
- Outputs
  - Capture and fetch pipeline in app/render modules.
  - Persistence schema update for favicon/thumbnail bytes.
  - Visual regression snapshots for sample URLs.

## Findings
- Existing tab favicon plumbing already existed (`window.take_pending_favicon_loads` + tab texture cache).
- The missing path was graph-node integration and persistence.
- `egui_graphs` supports custom node drawing via `DisplayNode`; this allows favicon rendering in graph view without changing graph interaction handling.
- Snapshot persistence is the right first persistence step for favicon bytes (log entries are not required for this slice).
- `image` crate is already available in graphshell dependencies, so resize/encode work for thumbnail bytes does not require dependency changes.
- FT2 should target the stabilized physics/selection architecture to avoid rebasing thumbnail node rendering onto changing state contracts.

## Progress
- 2026-02-11: Plan created.
- 2026-02-12: Implemented favicon vertical slice:
  - Added favicon fields to node model (`favicon_rgba`, `favicon_width`, `favicon_height`).
  - Added snapshot persistence support for favicon data (`PersistedNode` extended).
  - Added custom graph node shape that renders favicon textures when available (falls back to colored circle).
  - Wired favicon ingestion to persist bytes on mapped graph nodes from pending favicon loads.
  - Added tests for favicon snapshot/schema roundtrip.
- 2026-02-12: Sequencing updated:
  - Physics migration and selection consolidation are the immediate predecessor tasks.
  - Next FT2 slice starts with thumbnail byte pipeline (`capture -> resize -> persist -> render`).
- 2026-02-14: Landed FT2 thumbnail completion:
  - Added load-complete-triggered thumbnail capture requests via window semantic queue.
  - Added asynchronous screenshot -> resize (256x192) -> PNG pipeline.
  - Added stale-result rejection by webview URL match before reducer apply.
  - Persisted thumbnail bytes in snapshot schema and rendered with thumbnail > favicon > color fallback.
  - Added tests for thumbnail intent mapping and stale/empty capture rejection.
