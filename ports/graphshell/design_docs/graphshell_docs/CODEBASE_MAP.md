# Graphshell Module Map

Quick reference for navigating the graphshell codebase.

**Base:** `ports/graphshell/`  
**Test Count:** 137 (as of Feb 11, 2026)  
**Total LOC:** ~7,000

---

## Source Modules (by responsibility)

### Core Data Structures

**`graph/mod.rs`** (544 lines)
- `Graph` — StableGraph wrapper with URL→NodeKey index
- `Node` — URL, title, position, velocity, selection, lifecycle
- `EdgeType` — Hyperlink vs History
- Key methods: `add_node()`, `add_edge()`, `remove_node()`, `get_node_by_url()`

**`graph/egui_adapter.rs`** (155 lines)
- `EguiGraphState` — Converts Graph → egui_graphs::Graph
- Sets position, label, color, radius based on node data
- Lifecycle-based styling (Active=blue, Cold=gray, Selected=gold)

**`graph/spatial.rs`**
- Removed from active runtime as part of physics migration

---

### Physics & Simulation

**`app.rs` + `render/mod.rs`** (active runtime path)
- App-owned `FruchtermanReingoldState` (`GraphBrowserApp.physics`)
- egui_graphs force-directed layout integration (`LayoutForceDirected<FruchtermanReingold>`)
- Physics controls (`T` toggle, panel sliders) update FR state directly
- Layout state bridged each frame via `set_layout_state` / `get_layout_state`

**Removed from active runtime**
- Legacy `physics/mod.rs`, `physics/worker.rs`, and spatial-hash worker path (see physics migration plan)

---

### Rendering & UI

**`render/mod.rs`** (467 lines)
- Delegates to egui_graphs::GraphView widget
- Event handling: NodeDoubleClick, NodeDrag, NodeMove
- Info overlay: node/edge count, physics status, zoom level
- Physics config panel (live sliders)
- Post-frame zoom clamping

**`input/mod.rs`** (216 lines)
- Keyboard shortcuts (guarded when text focused):
  - `T` toggle physics, `C` fit-to-screen, `P` physics panel, `N` new node
  - `Home`/`Esc` toggle Graph/Detail view
  - `Del` remove selected, `Ctrl+Shift+Del` clear graph
- Mouse handled by egui_graphs

**`desktop/webview_controller.rs`** (278 lines)
- Webview lifecycle management (create/destroy)
- Navigation sync and URL change detection
- Address bar submission handling
- Webview close helpers

**`desktop/gui.rs`** (794 lines, NO DEDICATED TESTS — tested via integration)
- **Servo webview integration**
- Webview lifecycle: create/destroy based on view state
- Navigation tracking: `sync_webviews_to_graph()` detects URL changes
- Edge creation: Hyperlink for new nav, History for back/forward
- Toolbar: address bar, navigation buttons, view toggle
- **3 major bugs fixed in refinement:** (BUG-3, BUG-4, BUG-5 in plan)
- **Extracted:** `desktop/webview_controller.rs` (webview lifecycle/sync)

---

### Application State

**`app.rs`** (1010 lines)
- `GraphBrowserApp` — Main application state
- `View` enum: Graph vs Detail(NodeKey)
- Camera: zoom bounds (0.1x–10.0x)
- Selection management (single/multi)
- Webview↔Node bidirectional mapping
- Physics worker handle
- Persistence integration
- Key methods: `toggle_view()`, `focus_node()`, `select_node()`, `remove_selected_nodes()`, `clear_graph()`

---

### Persistence

**`persistence/mod.rs`** (518 lines)
- `GraphStore` — fjall log + redb snapshots + rkyv serialization
- Every mutation journaled as LogEntry
- Periodic snapshots (every 5 min)
- Recovery: load snapshot → replay log
- Methods: `log_mutation()`, `take_snapshot()`, `recover()`

**`persistence/types.rs`** (232 lines)
- `LogEntry` variants: AddNode, AddEdge, UpdateNodeTitle, PinNode, RemoveNode, ClearGraph, UpdateNodeUrl
- `GraphSnapshot` — Full graph serialization
- All types use rkyv for zero-copy serialization
- URL-based identity (no SlotMap keys)

---

### Utilities

**`util.rs`** (66 lines) **[NEW in Step 1]**
- `truncate_with_ellipsis()` — Char-safe string truncation
- Handles multi-byte UTF-8 (CJK, emoji)
- Replaces two unsafe implementations

**`lib.rs`** (boilerplate + module declarations)
- Entry point for graphshell crate
- Module tree declaration

---

## Test Distribution

- **Total tests:** 137 (as of Feb 11, 2026)
- **Unit-heavy modules:** app.rs, graph/mod.rs, render/mod.rs, input/mod.rs, persistence/
- **Integration-only:** gui.rs and webview_controller.rs (no dedicated unit tests yet)
- **To see exact counts:** `cargo test --lib -- --list`

---

## Data Flow

### Navigation Flow (Detail View)
```
User clicks link in webview
    ↓
Servo navigation event
    ↓
 desktop/webview_controller.rs: sync_to_graph() detects URL change
    ↓
Check if URL exists in graph.url_to_node
    ↓
  No: Create new node + Hyperlink edge
  Yes: Focus existing node + maybe History edge
    ↓
app.log_mutation(LogEntry::AddNode/AddEdge)
    ↓
persistence.log_mutation() → fjall append
    ↓
physics_worker.send(UpdateGraph) → layout update
    ↓
render.rs draws updated graph
```

### Persistence Flow (Startup)
```
GraphStore::recover()
    ↓
Load latest redb snapshot
    ↓
Replay fjall log entries after snapshot timestamp
    ↓
Return recovered Graph
    ↓
app.graph = recovered_graph
    ↓
physics_worker.send(UpdateGraph)
```

### View Toggle Flow
```
User presses Home/Esc
    ↓
app.toggle_view()
    ↓
  From Graph → Detail:
    - Clear egui_graph selection
    - Save active_webview_nodes list
    - Destroy all webviews
    - Set view = View::Detail(focused_node)
    ↓
  From Detail → Graph:
    - Restore webviews from active_webview_nodes list
    - Set view = View::Graph
```

---

## Key Invariants

1. **URL uniqueness:** `graph.url_to_node` has at most one NodeKey per URL
2. **Persistence logging:** Every graph mutation is logged before applying
3. **Webview mapping:** `webview_to_node` and `node_to_webview` are inverses
4. **NodeKey stability:** petgraph StableGraph ensures NodeKey survives deletions
5. **Physics sync:** Worker graph must match app graph for positions to be valid

---

## Hot Paths (Performance Critical)

1. **Physics step** (60 FPS target):
   - `physics/mod.rs::step()` — Force calculation + integration
   - `spatial.rs` — KD-tree neighbor queries

2. **Rendering** (45 FPS target with 500 nodes):
   - `render/mod.rs::render_graph()` → egui_graphs::GraphView
   - `egui_adapter.rs::from_graph()` — Rebuilds on `egui_state_dirty`

3. **Navigation sync** (every frame in Detail view):
   - `gui.rs::sync_webviews_to_graph()` — URL change detection

---

## Common Tasks

### Find where a feature is implemented
- **Node creation:** app.rs `add_node_to_graph()`, gui.rs address bar handler
- **Edge creation:** gui.rs `sync_webviews_to_graph()`
- **Physics:** physics/mod.rs `PhysicsEngine::step()`
- **Rendering:** render/mod.rs `render_graph()`
- **Persistence:** persistence/mod.rs `log_mutation()`, `take_snapshot()`

### Add a new graph mutation
1. Add LogEntry variant in persistence/types.rs
2. Add replay logic in persistence/mod.rs
3. Call log_mutation() in app.rs before mutation
4. Add tests for recovery

### Debug a rendering issue
1. Check `egui_state_dirty` flag in app.rs
2. Verify node positions are finite (not NaN)
3. Check egui_adapter.rs color/radius logic
4. Enable physics panel (P key) to see live state

### Debug a persistence issue
1. Check `default_data_dir()` for store location
2. Verify LogEntry serialization in types.rs tests
3. Add logging in `recover()` and `replay_log()`
4. Test with simplified mutations first

---

## File Size Reference (for context window planning)

**Large files** (read in chunks or sections):
- gui.rs: 978 lines — webview integration (needs Step 3 refactor)
- app.rs: 590 lines — application state
- physics/mod.rs: 385 lines — force-directed engine
- graph/mod.rs: 461 lines — graph data structures

**Medium files** (can read whole):
- render/mod.rs: 339 lines
- persistence/mod.rs: 373 lines

**Small files** (skim quickly):
- All others under 200 lines
