# Graphshell Implementation Roadmap

**Document Type**: Feature-driven implementation plan
**Organization**: By feature targets with validation tests (not calendar time)
**Last Updated**: February 19, 2026
**Priority Focus**: M1 complete (FT1-6); F1-F7 architectural features complete; M2 active: workspace routing/membership, graph UX polish, edge/radial follow-on

**Policy Note (2026-02-14)**: Graphshell has no production users and no legacy dataset obligations. Do not add backward-compat contingency branches unless explicitly requested.

---

## Current State

**Core Browsing Graph Functional** (headed egui_tiles runtime complete):

- [x] Graph data structures (petgraph StableGraph, NodeIndex/EdgeIndex keys)
- [x] Multi-pane tile runtime (egui_tiles): tile tree, per-pane rendering contexts, tile layout persistence
- [x] Runtime persistence controls: data directory switching and snapshot interval configurability
- [x] egui_graphs rendering (GraphView widget, zoom/pan, selection, events)
- [x] Keyboard input and view controls (guarded when text fields are focused)
- [x] Tile-derived view state (legacy `View` enum retired)
- [x] Servo webview integration (create/destroy, navigation tracking, edge creation)
- [x] Graph persistence (fjall log + redb snapshots + rkyv serialization)
- [x] Camera fit-to-screen (C key, egui_graphs `fit_to_screen`)

**Current Gaps**:

- [ ] Selection-state hardening follow-up: keep reducer-driven behavior and explicit selection metadata stable

**Status**: Core browsing, tiled webviews, persistence, thumbnail rendering, and graph search/filter are production-functional.

---

## Phase 1: Core Browsing Graph

### Priority: Top 5 Features for Browsable Graph

These five features enable the core MVP: **users can browse real websites in a spatial graph that persists and feels responsive**.

| # | Feature | Status | Implementation |
| --- | ------- | ------ | -------------- |
| 1 | Servo Webview Integration | ✅ Complete | gui.rs: webview lifecycle, navigation tracking, edge creation |
| 2 | Thumbnail & Favicon Rendering | ✅ Complete | Async thumbnail capture + favicon fallback + snapshot persistence + graph rendering tiering |
| 3 | Graph Persistence | ✅ Complete | fjall log + redb snapshots + rkyv serialization |
| 4 | Camera Zoom Integration | ✅ Complete | egui_graphs built-in zoom/pan + post-frame clamp |
| 5 | Center Camera | ✅ Complete | egui_graphs fit_to_screen via C key |

**Execution order now:**

1. Navigation control-plane stabilization (see [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md))
2. Selection consolidation (see [2026-02-14_selection_semantics_plan.md](implementation_strategy/2026-02-14_selection_semantics_plan.md))
3. FT2 thumbnail completion ✅
4. FT6 search/filtering (`nucleo`) ✅

---

### Feature Target 1: Servo Webview Integration ✅ COMPLETE

**Goal**: Users can browse real websites, and each page becomes a node in the graph.

**Implementation** (in `desktop/gui.rs`, 1096 lines):

- Full webview lifecycle: create/destroy webviews based on view state
- Graph view: destroy all webviews (prevent framebuffer bleed-through), save node list for restoration
- Detail view: recreate webviews for saved nodes, create for newly focused nodes
- Navigation tracking: `sync_webviews_to_graph()` detects URL changes, creates nodes + edges
- Edge creation: Hyperlink for new navigation, History for back/forward (detected by existing reverse edge)
- URL bar: Enter in graph view updates node URL and switches to detail view
- Bidirectional mapping: `HashMap<WebViewId, NodeKey>` and inverse in `app.rs`

**Frame execution order** (critical — misordering caused bugs):

1. Handle keyboard (may change view or clear graph)
2. Webview lifecycle (destroy/create based on current view)
3. Sync webviews to graph (only in detail view — detects URL changes, creates edges)
4. Toolbar + tab bar rendering
5. Physics update
6. View rendering (graph OR detail, exclusive)

**Validation**:

- [x] Load `https://example.com` → creates first node
- [x] Click link in webview → creates second node + edge (Hyperlink type)
- [x] Switch to Graph view → webview hidden, graph visible
- [x] Double-click node → Detail view, correct webview shown
- [x] Close node (delete) → webview destroyed, no crashes

---

### Feature Target 2: Thumbnail & Favicon Rendering ✅ COMPLETE

**Goal**: Nodes show recognizable visuals: page thumbnail (best), site favicon (fallback), or lifecycle color (final fallback).

**Context**: Currently rendering circles with lifecycle colors only. Spatial memory benefits from site/page recognition.

**Rendering Priority** (tier-based approach):

1. **Page thumbnail** (256x192) — Full page preview, best spatial memory aid, Active/Warm nodes
2. **Site favicon** (32x32) — Standard browser favicon, lightweight, works for all lifecycle states
3. **Colored circle** — Lifecycle color only (current state, final fallback)

**Tasks**:

1. Favicon fetch & caching:
   - Fetch `/favicon.ico` or parse `<link rel="icon">` from page metadata
   - Cache favicon in Node struct (Option<egui::TextureHandle>)
   - Fetch asynchronously, non-blocking (parallel with page load)

2. Thumbnail capture:
   - Capture webview frame after page load (use `webrender::screen_capture` or `glow::Context::read_pixels()`)
   - Downscale to 256x192 using `image::imageops::resize()`
   - Store in Node struct (Option<egui::TextureHandle>)

3. Node rendering logic in `render/mod.rs`:

   ```rust
   fn render_node(node: &Node) {
       if let Some(thumbnail) = &node.thumbnail {
           // Render 256x192 thumbnail as rounded rect
       } else if let Some(favicon) = &node.favicon {
           // Render 32x32 favicon centered
       } else {
           // Render colored circle (lifecycle color)
       }
   }
   ```

4. Cache management to avoid re-captures

**Validation Tests**:

- [x] Load page -> favicon appears and is cached on mapped node metadata
- [x] Load page -> thumbnail capture requested on load-complete and applied asynchronously
- [x] Thumbnail matches latest URL only (stale capture rejection by requested URL)
- [x] Rendering fallback works: thumbnail -> favicon -> lifecycle color
- [x] Snapshot persistence preserves favicon and thumbnail bytes
- [x] Favicon/thumbnail failures keep node render path functional (color fallback)

**Outputs**:

- Favicon fetch code in `app.rs` (async loader)
- Thumbnail capture code in `app.rs` or `render/mod.rs`
- Updated Node struct with `thumbnail: Option<egui::TextureHandle>` and `favicon: Option<egui::TextureHandle>`
- Rendering logic in `render/mod.rs` with three-tier fallback
- Visual regression tests comparing renders to expected output

**Success Criteria**:

- Favicons available immediately on most sites (80%+ within 500ms)
- Thumbnails recognizable within 2 seconds (90%+ sites load that fast)
- No performance degradation (<10% fps drop even with 100 mixed nodes)
- Graceful fallback when favicon/thumbnail unavailable (circle always renders)

---

### Feature Target 3: Graph Persistence (Snapshots + Log) ✅ COMPLETE

**Goal**: Save graph on shutdown, restore on startup, survive crashes with minimal data loss.

**Implementation** (`persistence/mod.rs` + `types.rs`, 636 lines):

- **fjall v3**: Append-only operation log — every mutation logged (AddNode, AddEdge, UpdateTitle, PinNode)
- **redb v3**: Periodic snapshots — full graph serialization every 5 minutes
- **rkyv 0.8**: Zero-copy serialization for both log entries and snapshots
- Startup recovery: load latest redb snapshot → replay fjall log entries since snapshot timestamp
- Aligned data handling: redb bytes aren't aligned for rkyv; copy to `AlignedVec` before deserializing
- URL-based identity for persistence (petgraph NodeIndex values change across sessions)
- 19 unit tests covering serialization, log replay, snapshot roundtrip

**Validation**:

- [x] Browse pages → shutdown → restart → all nodes restored
- [x] No snapshot file → creates new graph (empty start)
- [x] Snapshot + log roundtrip preserves graph structure

---

### Feature Target 4: Camera Zoom Integration ✅ COMPLETE

**Goal**: Mouse wheel zooms in/out, graph scales accordingly.

**Implementation**:

- Replaced custom camera with egui_graphs built-in `SettingsNavigation` (zoom/pan)
- Post-frame zoom clamp: read `MetadataFrame` from `ctx.data_mut()` after GraphView renders, enforce 0.1x–2.0x bounds
- `MetadataFrame` stored at `Id::new("egui_graphs_metadata_")` (empty custom_id)
- Custom Camera module removed (dead code after egui_graphs integration)

**Validation**:

- [x] Scroll up/down → graph zooms in/out
- [x] Zoom range clamps at 0.1x–2.0x
- [x] Zoom + pan → coordinates consistent
- [x] Dragging nodes works at any zoom level

---

### Feature Target 5: Center Camera on Graph ✅ COMPLETE

**Goal**: Press `C` key → camera moves to show all nodes (auto-fit).

**Implementation**:

- `C` key sets `fit_to_screen_requested = true` one-shot flag in `app.rs`
- `render/mod.rs` passes flag to egui_graphs `SettingsNavigation::fit_to_screen()`
- egui_graphs calculates bounding box and adjusts zoom/pan automatically
- Flag cleared after use (one-shot behavior)

**Validation**:

- [x] Press C → camera fits all nodes in viewport
- [x] Works at any zoom level
- [x] Press C multiple times → deterministic result

---

## Phase 2: Usability & Polish

### Feature Target 6: Search & Filtering ✅ COMPLETE

**Goal**: Press Ctrl+F → search bar appears → type URL/title → matching nodes highlighted.

**Tasks**:

1. Add search bar UI (egui TextEdit)
2. Implement fuzzy string matching (use **nucleo** crate, fzf-like scoring)
3. Highlight matching nodes (different color or pulsing border)
4. Support filtering: show only matching nodes, hide others
5. Navigate search results: Up/Down arrows cycle through matches

**Validation Tests**:

- [x] Ctrl+F opens graph search UI
- [x] Search "example" highlights nodes with "example" in URL or title
- [x] Fuzzy matching works: "gthub" matches "github.com"
- [x] Empty search shows all nodes (no filter)
- [x] Search + filter mode shows only matching nodes
- [x] Up/Down cycles matches and Enter selects active match
- [x] Esc clears search query (and closes when already empty)

**Outputs**:

- Search UI in `render/mod.rs` or new `search.rs` module
- Integration with nucleo crate
- Keybind for Ctrl+F

**Success Criteria**:

- Find nodes quickly in large graphs (200+ nodes)
- Fuzzy matching feels intuitive (fzf-like behavior)
- No performance degradation during search

---

### Feature Target 7: Bookmarks & History Import

**Goal**: Import browser bookmarks/history → seed initial graph structure.

**Tasks**:

1. Parse Firefox/Chrome bookmark format (JSON/HTML)
2. Create node for each bookmark
3. Create edges based on folder structure (parent-child)
4. Parse browser history database (SQLite)
5. Create nodes for recent history entries
6. Create edges based on referrer chains

**Validation Tests**:

- [ ] Import 100 bookmarks → creates 100 nodes
- [ ] Bookmark folders become node clusters (edges connecting folder contents)
- [ ] Import history → creates nodes for frequently visited sites
- [ ] Duplicate URLs merged (same node for bookmark + history entry)

**Outputs**:

- Import UI (file picker or auto-detect browser data directories)
- Parser for bookmark/history formats
- Migration guide for users switching from traditional browsers

**Success Criteria**:

- Successfully imports bookmarks from major browsers
- Graph structure reflects bookmark organization
- No data loss during import

---

### Feature Target 8: Performance Optimization

**Goal**: 500 nodes @ 45fps, 1000 nodes @ 30+fps (usable), smooth interaction. Realistic baseline: power users with 100+ tabs produce ~100-300 nodes; 500-1000 is a stress target.

**Tasks**:

1. Profile rendering: identify bottlenecks (egui Painter calls)
2. Batch node rendering (single draw call for all circles)
3. Cull off-screen nodes (simple rect test, skip if out of viewport)
4. LOD (level-of-detail): cluster distant nodes when zoomed out
5. Measure FR layout time per frame (ensure <16ms at target node counts)
6. Profile egui_graphs GraphView overhead at scale

**Validation Tests**:

- [ ] 500 nodes → 45fps maintained during pan/zoom
- [ ] 1000 nodes → 30+fps, usable (slight lag acceptable)
- [ ] Profile confirms no single hotspot >50% frame time
- [ ] Memory usage scales linearly (no leaks)

**Outputs**:

- Performance benchmarks (flamegraphs, timing data)
- Optimized rendering code
- LOD system (if needed)

**Success Criteria**:

- Meets performance targets without fallback UI
- Smooth interaction even with large graphs
- If targets not met: implement fallback (cluster strip view)

---

## Phase 3: Advanced Features

### Feature Target 9: Clipping (DOM Element Extraction)

**Goal**: Right-click element in webpage → "Clip to Graph" → element becomes independent node.

**Tasks**:

1. Expose Servo's DOM inspector API to GraphShell
2. Implement right-click context menu in webview
3. Capture clicked element's rendered output (screenshot or HTML snapshot)
4. Create new node with clipped content
5. Store DOM path or HTML in node metadata (for updating)

**Success Criteria**:

- Can clip images, text blocks, or entire sections
- Clipped nodes remain linked to source page
- Updates if source page modified (optional)

---

### Feature Target 10: Diagnostic/Engine Inspector Mode

**Goal**: Toggle mode to visualize Servo's internal architecture (Constellation, threads, IPC channels).

**Tasks**:

1. Instrument Servo with `tracing::span!()` at thread/channel boundaries
2. Collect trace events in GraphShell layer
3. Build dynamic graph: ThreadId → Node, Channel → Edge
4. Visualize message counts, latencies, backpressure as edge weights/colors
5. Implement mode toggle (Ctrl+Shift+D)
6. Support export as SVG for performance reports

**Success Criteria**:

- Developers can see real-time thread activity
- Users understand what browser is doing (transparency/education)
- Identifies performance bottlenecks visually

---

### Feature Target 11: P2P Collaboration (Verse)

**Goal**: Share graph with peers, real-time co-browsing, permissions-based access.

**Tasks**:

- (Deferred to design_docs/verse_docs/)
- Requires: IPFS integration, tokenization, CRDTs for sync

---

## Implementation Notes

**Integrated Crates**:

| Feature | Crate | Status |
| ------- | ----- | ------ |
| Graph data structure | **petgraph** 0.8 | ✅ StableGraph as primary store |
| Graph visualization | **egui_graphs** 0.29 | ✅ GraphView widget, events, navigation |
| Spatial queries | n/a | `kiddo` removed with physics migration |
| Persistence snapshots | **redb** 2 | ✅ Periodic full graph snapshots |
| Persistence log | **fjall** 3 | ✅ Append-only mutation log |
| Serialization | **rkyv** 0.8 | ✅ Zero-copy, used by both fjall and redb |

| Search (fuzzy) | **nucleo** | ✅ Integrated for graph search/filter (FT6) |

**Planned (Not Yet Integrated)**:

| Feature | Recommended Crate | Why |
| ------- | ----------------- | --- |
| Search (full-text) | **tantivy** | Lucene equivalent, BM25, for future content indexing |

**Avoid**:

- `sled` — Stuck in beta since 2021, known corruption bugs
- `keyframe` — Abandoned (no activity since 2022)
- `RocksDB` — Massive C++ dependency, overkill

**Build Time Tracking**:

- Clean build: ~15-30 min (depends on machine)
- Incremental: ~30s-2min (typical code change)
- Release build: +20% time vs debug

---

## Success Milestones

**M1: Browsable Graph** (Feature Targets 1-5) — ✅ Complete

- ✅ Can browse real websites (Servo webviews integrated)
- ✅ Graph persists across sessions (fjall + redb + rkyv)
- ✅ Basic navigation (zoom, pan, center via egui_graphs)
- ✅ Thumbnails and favicons for spatial recognition

**M2: Usable Browser** (Feature Targets 6-8)

- ✅ Search/filter works (nucleo fuzzy search, FT6 complete)
- Performance acceptable (500 nodes)
- Bookmarks import seeded graph

**M3: Advanced** (Feature Targets 9-11)

- Clipping extracts DOM elements
- Diagnostic mode visualizes engine
- P2P collaboration (if Verse phase reached)

**Validation Gates**:

- After M1: deterministic integration tests for graph/webview semantics and persistence recovery
- After M2: performance benchmarks for node count, memory, and frame-time budgets
- After M3: release-readiness audit against architecture and test criteria

---

## Risk Mitigation

**Risk**: Servo/Graphshell semantic divergence  
**Mitigation**: keep event-driven semantics (`notify_*`, `request_create_new`) as single source, with reducer-boundary tests

**Risk**: Performance targets not met (500 nodes too slow)  
**Mitigation**: profile + optimize current architecture first (layout, draw, capture cadence) before introducing alternate UI modes

**Risk**: Persistence bugs cause data loss  
**Mitigation**: comprehensive crash-recovery tests and strict UUID-keyed replay invariants

---

## References

- **Checkpoint Analyses**: `archive_docs/checkpoint_2026-02-09/`
- **Project Vision**: `PROJECT_DESCRIPTION.md`
- **Architecture**: `ARCHITECTURAL_OVERVIEW.md`
- **Code**: `ports/graphshell/` (~4,500 LOC in core modules)

