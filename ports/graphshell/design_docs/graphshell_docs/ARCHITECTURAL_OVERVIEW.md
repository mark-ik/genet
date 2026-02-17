# Graphshell Architectural Overview

**Last Updated**: February 16, 2026
**Status**: Core browsing graph functional — Servo integration, egui_tiles multi-pane, persistence, thumbnails/favicons, search/filter all working

---

## Project Vision

Graphshell is a **spatial tab manager** where webpages are nodes in a force-directed graph instead of tabs in a bar. The graph, tile tree, and per-pane tab bars are all projections of the same source of truth: the set of webviews (active or inactive). Mutations from any context propagate to all others.

**Core Idea**: Replace linear history with spatial memory. Instead of "Back/Forward," you see where you came from and where pages link to.

**See**: [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) for the full behavioral specification of the unified graph-tile-webview model.

---

## Current Implementation Status

### Foundation (~7,000 LOC total; core graph + physics ~1,300 LOC minimum)

**Graph Core** (`graph/mod.rs`, 461 lines)
- `Graph`: petgraph `StableGraph<Node, EdgeType, Directed>` as primary store
- `Node`: URL, title, position, velocity, pinned, lifecycle (Active/Cold), favicon data, thumbnail data, history entries
- `EdgeType`: Hyperlink (new tab from parent), History (back/forward). `UserGrouped` planned but not yet implemented.
- `NodeKey = NodeIndex`, `EdgeKey = EdgeIndex` — stable handles surviving deletions
- `url_to_node: HashMap<String, NodeKey>` for O(1) URL lookup
- `out_neighbors()`, `in_neighbors()`, `has_edge_between()` for traversal
- Snapshot serialization: `to_snapshot()` / `from_snapshot()` for persistence

**egui_graphs Adapter** (`graph/egui_adapter.rs`, 163 lines)
- `EguiGraphState`: converts `Graph` → egui_graphs `Graph` via `to_graph_custom()`
- Sets position, label, color, radius, selection from node data
- Lifecycle-based styling: Active (blue, r=15), Cold (gray, r=10), Selected (gold)
- Rebuilt only when `egui_state_dirty` flag is set (structural changes)

**Tile Tree** (`desktop/gui.rs` tile integration)
- egui_tiles multi-pane runtime: tile tree, per-pane rendering contexts, tile layout persistence
- Each tile pane has a tab bar showing its cluster's nodes (connected by edges)
- Tile-derived view state (legacy `View` enum retired)
- Tab bars are projections of graph clusters; closing a tab removes the node everywhere

**Physics Runtime** (`app.rs` + `render/mod.rs`) — **Implemented via egui_graphs**
- Force-directed layout uses egui_graphs `FruchtermanReingoldState`
- Runtime state is app-owned (`GraphBrowserApp.physics`) and bridged through `set_layout_state`/`get_layout_state` each frame
- Physics panel controls operate directly on FR state (damping, attraction, repulsion, scale, run/pause)
- Previous custom physics module/worker path has been removed from active runtime

**Rendering** (`render/mod.rs`, 339 lines)
- Delegates graph visualization to `egui_graphs::GraphView` widget
- Built-in zoom/pan navigation (`SettingsNavigation`), dragging + selection (`SettingsInteraction`)
- Event-driven: NodeDoubleClick → focus, NodeDragStart/End → physics pause, NodeMove → position sync
- Info overlay: node/edge count, physics status, zoom level, controls hint
- Physics config panel: live sliders for all force parameters
- Post-frame zoom clamp: enforces min/max bounds on egui_graphs zoom

**Input** (`input/mod.rs`, 87 lines)
- Mouse interaction delegated to egui_graphs (drag, pan, zoom, selection, double-click)
- Keyboard shortcuts (guarded — disabled when text field has focus):
  - `T` toggle physics, `C` fit to screen, `P` physics panel, `N` new node
  - `Home`/`Esc` toggle Graph/Detail view
  - `Del` remove selected, `Ctrl+Shift+Del` clear graph

**Application State** (`app.rs`, 590 lines)
- Tile-derived view state (graph pane vs detail panes determined by tile tree)
- Bidirectional webview↔node mapping: `HashMap<WebViewId, NodeKey>` and inverse
- Selection management (single/multi), focus switching
- FR layout state lifecycle (sync app-owned state with egui_graphs per frame)
- Persistence integration: log mutations, periodic snapshots
- `egui_state_dirty` flag controls when egui_graphs state is rebuilt
- Camera: zoom bounds (0.1x–10.0x), post-frame clamping via `MetadataFrame`

**Persistence** (`persistence/` module, 560 lines total)
- **fjall**: Append-only operation log (every mutation: AddNode, AddEdge, UpdateTitle, PinNode)
- **redb**: Periodic snapshots (full graph serialization, every 5 minutes)
- **rkyv**: Zero-copy serialization for both log entries and snapshots
- Startup recovery: load latest snapshot → replay log entries since snapshot
- Aligned data handling: `AlignedVec` for rkyv deserialization from redb bytes

**Servo Integration** (`desktop/gui.rs` + `desktop/webview_controller.rs`, ~1,100 lines total)
- Full webview lifecycle: create/destroy webviews based on tile tree state
- Graph view: destroy all webviews (prevent framebuffer bleed), save node list for restoration
- Detail view: recreate webviews for saved nodes, create for newly focused nodes
- Navigation tracking: `sync_webviews_to_graph()` detects URL changes, creates edges
- Servo signals: `request_create_new` (new tab), `notify_url_changed` (within-tab nav), `notify_title_changed`
- Edge creation: Hyperlink for new tab from parent, History for back/forward (existing reverse edge)

### Not Yet Implemented

**In active development:**

1. **Navigation control-plane cleanup** — Wire Servo delegate callbacks, remove polling-based node creation ([plan](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md))
2. **Selection consolidation** — Remove duplicated selection state ([plan](implementation_strategy/2026-02-14_selection_semantics_plan.md))
3. **Physics follow-up** — Keep FR tuning/profile work visible after migration ([plan](implementation_strategy/2026-02-14_physics_migration_plan.md))

**Phase 2+ features (not started):**

- Bookmarks/history import
- Performance optimization (500+ nodes)
- Clipping (DOM element extraction)
- Diagnostic/Engine Inspector mode
- P2P collaboration (Verse)

---

## Architecture Decisions

### Data Structures

**Why petgraph StableGraph?**
- Stable indices survive node/edge deletions (unlike `Graph` which reuses indices)
- Rich algorithm ecosystem (pathfinding, centrality, clustering) available via trait imports
- `pub(crate) inner` gives egui_adapter direct access for `to_graph_custom()`
- Eliminates the SlotMap + manual adjacency list approach (simpler, fewer data structures)

**Why URL-to-NodeKey HashMap?**
- Fast duplicate detection: "Does this URL already have a node?"
- O(1) lookup for persistence recovery (log replay uses URLs as stable identity)
- **Note**: The unified model allows duplicate URLs (same URL in multiple tabs). Node identity will migrate from URL-based to UUID-based. See [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md).

**Why NodeIndex keys not stable across sessions?**
- petgraph NodeIndex values change when graph is rebuilt from persistence
- URL-based identity used for persistence (snapshot + log use URLs, not indices)
- Planned: UUID-based stable identity that survives across sessions and allows duplicate URLs

### Rendering & UI

**Why egui_graphs?**
- Purpose-built for interactive graph visualization in egui
- Provides zoom/pan, dragging, selection, labels out of the box
- Event-driven interaction model (events collected in `Rc<RefCell<Vec<Event>>>`)
- Reduced custom rendering code by ~80% (input went from 313 → 100 LOC)

**Why `FruchtermanReingold` layout in egui_graphs?**
- Removes custom physics/worker complexity while keeping force-directed behavior
- Keeps layout integration inside GraphView (single framework path)
- Supports the existing physics panel and interaction model with fewer bespoke subsystems
- See [2026-02-14_physics_migration_plan.md](implementation_strategy/2026-02-14_physics_migration_plan.md)

**Why post-frame zoom clamp?**
- egui_graphs has no built-in zoom bounds
- Read `MetadataFrame` from egui's persisted data after `GraphView` renders
- Clamp zoom value, write back if changed

### Graph-Tile-Webview Unified Model

**Why one source of truth (the webview set)?**
- Graph, tile tree, and tab bars are all projections of the same state: the set of webviews (active or inactive)
- Mutations from any context (graph drag, tab close, keyboard shortcut, Servo callback) produce intents processed at a single sync point per frame
- Prevents contradictory-update bugs that arise from bidirectional mutation between graph and tile tree
- See [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) for the full behavioral specification

**Why intent-based mutation?**
- Extends existing `GraphAction`/`KeyboardActions` pattern to cover tile operations
- All interactions produce intents collected during the frame, applied together at one sync point
- No system directly mutates another mid-frame — eliminates the class of bugs from the servoshell tab UI adaptation

**Why nodes ARE tabs (not representations of tabs)?**
- Node identity is the tab itself (stable UUID), not its URL
- Within-tab navigation updates the node's URL; no new node created
- Duplicate URLs allowed (same URL open in multiple tabs = multiple independent nodes)
- Per-node history provided by Servo's `notify_history_changed`, not custom stacks

### Webview Lifecycle

**Why destroy webviews in graph view?**
- Servo renders webviews into the window framebuffer
- In graph view, webview content bleeds through the graph overlay
- Solution: save which nodes had webviews, destroy all, recreate on return to detail view

**Why the frame execution order matters (gui.rs):**
1. Handle keyboard (may change view or clear graph)
2. Webview lifecycle (destroy/create based on current view)
3. Sync webviews to graph (only in detail view — detects URL changes, creates edges)
4. Toolbar + tab bar rendering
5. Physics update
6. View rendering (graph OR detail, exclusive)

If sync runs before lifecycle or in graph view, it sees stale webviews and creates phantom nodes. This ordering was the root cause of two bugs (clear_graph not working, edges not being created properly).

### Persistence

**Why fjall + redb + rkyv?**
- **fjall**: LSM-tree append log, write-optimized, ACID, pure Rust — every mutation logged
- **redb**: B-tree KV store, faster than sled, ACID — periodic full snapshots
- **rkyv**: Zero-copy serialization, fastest in Rust — used for both log and snapshot format
- Recovery: load latest redb snapshot, replay fjall log entries since snapshot timestamp
- Aligned data: redb bytes aren't aligned for rkyv; copy to `AlignedVec` before deserializing

---

## Key Crates

| Crate | Purpose | Notes |
|-------|---------|-------|
| `petgraph` 0.8 | Graph data structure | StableGraph, algorithms via trait imports |
| `egui_graphs` 0.29 | Graph visualization | GraphView widget, events, navigation |
| `egui` 0.33.3 | UI framework | Immediate mode, integrated with Servo |
| `egui_tiles` | Multi-pane tile layout | Tile tree, per-pane tab bars |
| `kiddo` 4.2 | KD-tree spatial queries | Removed from active graphshell runtime |
| `fjall` 3 | Append-only log | Persistence mutations |
| `redb` 3 | KV store | Persistence snapshots |
| `rkyv` 0.8 | Serialization | Zero-copy, used by both fjall and redb |
| `crossbeam` | Worker channels | Physics thread + running_app_state |
| `euclid` | 2D geometry | Point2D, Vector2D throughout |

---

## External Repo Lessons (Feb 11 2026)

Scope: Readme/docs and selected files from GraphRAG, Midori Desktop, egui_node_graph2, BrowseGraph, and Obsidian releases. Obsidian releases does not include app source; Midori Desktop is a large Firefox-derived tree so findings emphasize structure and UI modularization.

### Cross-Repo Patterns Worth Adopting

**Factory/Provider registration (GraphRAG)**
- Pattern: pluggable providers with registration and lazy loading for storage, cache, vector stores, logging, metrics, and pipelines.
- Lesson: define stable interfaces and a registry for Graphshell subsystems (storage, graph store, LLM, indexing, thumbnailing) so implementations can swap without UI churn.

**Local-first data + privacy tiers (BrowseGraph)**
- Pattern: local vector DB (pglite + pgvector), local LLM classification/summarization, minimal cloud calls for graph transforms.
- Lesson: favor local extraction and indexing; allow cloud augmentation only with explicit opt-in and minimal payloads.

**Command palette as primary navigation (BrowseGraph + Obsidian ecosystem)**
- Pattern: cmdk-driven search/command UI; Obsidian relies on a strong command and plugin ecosystem.
- Lesson: make search, node creation, and graph actions accessible via a fast command palette to reduce UI clutter.

**Trait-based UI customization (egui_node_graph2)**
- Pattern: graph model is generic; node UI is driven by traits for data types, values, node templates, and user responses.
- Lesson: keep Graphshell graph model separate from UI widgets; use explicit trait-like interfaces for node rendering, interactions, and extensibility.

**UI modular controllers (Midori Desktop)**
- Pattern: many small controllers with explicit responsibilities and explicit wiring (sidebar_main, settings controller, resizer, mover, patcher).
- Lesson: Graphshell UI should be decomposed into controller-style modules with narrow responsibilities and explicit orchestration.

**Ecosystem distribution and schema discipline (Obsidian releases)**
- Pattern: plugin/theme registries are data-first (JSON registries), with strict conventions and distributed release fetching.
- Lesson: if Graphshell adopts a plugin system, start with a strict registry schema and a clear update and compatibility story.

### Architecture Implications for Graphshell

**Pluggable graph services**
- Add a provider registry for: storage, persistence strategy, vector store, embedding provider, graph extractor, and thumbnail renderer.
- Align this with config-driven selection (GraphRAG-style config sections) so headless automation and UI can share the same defaults.

**Local-first knowledge graph pipeline**
- Implement a minimal local pipeline for: text extraction, entity/relationship extraction, and index storage.
- Reserve cloud LLMs for optional enrichment steps and allow strict data minimization (summary or entity list only).

**Command palette as a spine**
- A primary command palette can unify: node search, open URL, toggle physics, pin, snapshot, export, and graph queries.
- This reduces dependence on multiple panels and keeps the graph view clean.

**UI layering and controller model**
- Follow Midori's controller separation: distinct modules for graph view, detail view, sidebar, command palette, and persistence UI.
- Keep graph interactions as a single module that owns selection, pan/zoom, and node editing.

**Theme and UX extension surface**
- Obsidian-style theme registry implies: theme tokens, CSS-like theming, and preview surfaces.
- Graphshell should define a stable theme token palette early, even if only a few built-in themes exist.

### Concrete Crate/Library Considerations

- **Vector storage**: evaluate a local vector store approach similar to pglite + pgvector (Rust equivalents could be sqlite + sqlite-vss or an embedded HNSW implementation).
- **Command palette**: use a dedicated command palette widget in egui (custom or port a cmdk-like interaction model).
- **Node graph UI**: consider egui_node_graph2 patterns for type-safe customization and a clear data/UI separation, even if we stay on egui_graphs for rendering.

### Servo Leverage Opportunities

**Structured content extraction**
- Use Servo's DOM and layout pipeline to extract text, headings, links, and metadata for graph enrichment.
- Add an internal extraction interface so graph nodes can refresh their summaries as pages mutate.

**Thumbnail and favicon pipeline**
- Implement offscreen rendering in Servo to generate node thumbnails without visible webviews.
- Store thumbnails in persistence and update on navigation or page idle.

**Navigation event fidelity**
- Capture navigation events at the engine layer to distinguish new navigations vs history traversal; avoid heuristic edge typing.
- Use this to generate cleaner History edges and to label link provenance.

**Side-panel and split-view groundwork**
- Servo already supports multiple webviews; use this to build a dedicated sidebar or split view without rehydrating webviews from scratch.
- Wire this into the controller model so views can be swapped without tearing down engine state.

---

## References

**Codebase**:
- `ports/graphshell/` — Main implementation (~4,500 LOC in core modules)
- `ports/servoshell/` — Base shell (windowing, event loop, WebRender) — graphshell extends this
- `components/servo/` — libservo (browser engine)

**Design Specs**:
- [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) — Unified graph-tile-webview behavioral specification
- [IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md) — Feature targets and execution order

**Implementation Plans**:
- [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md) — Architecture and navigation consolidation
- [2026-02-14_physics_migration_plan.md](implementation_strategy/2026-02-14_physics_migration_plan.md) — Physics migration
- [2026-02-14_selection_semantics_plan.md](implementation_strategy/2026-02-14_selection_semantics_plan.md) — Selection consolidation

**Checkpoint Analyses**:
- `archive_docs/checkpoint_2026-02-10/2026-02-10_crate_refactor_plan.md` — egui_graphs + petgraph + kiddo integration history
- `archive_docs/checkpoint_2026-02-09/Claude ANALYSIS 2.9.26.md` — Codebase audit & recommendations
