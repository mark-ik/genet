# Committed Architecture vs. Design Research

**Current Date**: February 13, 2026  
**Commit**: e342a5e6a43 (HEAD -> graphshell, origin/graphshell) — Merge branch 'servo:main' into graphshell  
**Last graphshell-specific commit**: 1a8a793d8ba (egui_tiles Phases 5-8 complete)

---

## What Is Actually Committed

### Core Data Structures

**Graph** (`graph/mod.rs`, 709 lines)
- `Graph`: petgraph `StableGraph<Node, EdgeType, Directed>`
- `Node`: uuid, url, title, position, velocity, is_selected, is_pinned, last_visited, favicon_rgba/width/height, **lifecycle** (Active/Cold)
- `EdgeType`: Hyperlink, History (UserGrouped **not yet implemented**)
- `url_to_node: HashMap<String, NodeKey>` for O(1) URL lookup
- **Problem**: Uses URL as identity, breaks with duplicate URLs (research identifies this as Phase 1 required change)

**Tile Tree** (`desktop/gui.rs` lines ~100-150)
- `egui_tiles::Tree<TileKind>` scaffolded (imported, initialized)
- `tile_kind.rs` — defines TileKind::Graph and TileKind::WebView(NodeKey)
- `tile_behavior.rs` — GraphshellTileBehavior trait impl (scaffold in place)
- **Status**: Structure in place, not fully wired to graph/webview lifecycle

**Webviews**
- Bidirectional mapping: `HashMap<WebViewId, NodeKey>` + inverse
- Lifecycle: created/destroyed based on tile visibility
- `active_webview_nodes` list saves state during graph<->detail view transitions
- Managed by `webview_controller.rs` (manage_lifecycle, sync_to_graph functions)

**Physics**
- `PhysicsEngine` in-process (for local queries)
- `PhysicsWorker` background thread (clones entire graph, runs simulation, reports positions)
- **Known bugs**:
  - Bidirectional spring application (applies force both directions: out_neighbors AND in_neighbors)
  - Workspace clone on every physics tick (expensive)
- **Planned for replacement**: egui_graphs FruchtermanReingold (physics_selection_plan.md)

**Persistence**
- **fjall**: Append-only log (`mutations` keyspace) w/ rkyv serialization
- **redb**: Periodic snapshots w/ rkyv serialization
- **Recovery**: load latest snapshot → replay log entries since snapshot timestamp
- **Issue**: Log entries use URLs as keys (breaks with UUID-based identity)

### Input Handling

**KeyboardActions** (`input/mod.rs`, 87 lines)
- `struct KeyboardActions`: booleans for toggle_physics, toggle_view, fit_to_screen, etc.
- `collect_actions(ctx: &egui::Context)` — input detection only
- `apply_actions(app: &mut GraphBrowserApp, actions: &KeyboardActions)` — pure state mutation
- **Properties**:
  - Decouples input detection from application
  - Actions are simple booleans, not structured intents
  - No semantic vs presentation distinction
  - No mode-dependent behavior

### Synchronization Between Layers

**Current architecture** (no intent system):

```
Servo Events (notify_url_changed, request_create_new, etc.)
    ↓
Webview mappings + sync_to_graph polling
    ↓
Graph mutations (create node, add edge)
    ↓
Persistence logging (fjall)
    
Keyboard Input
    ↓
collect_actions() → KeyboardActions
    ↓
apply_actions() → Graph mutations
    ↓
Persistence logging

Tile Tree Interactions
    ↓
(Not yet implemented — egui_tiles structure in place but handlers missing)
    ↓
???
```

**Problems this creates**:
- No serialization point for multi-source mutations
- `sync_to_graph()` does URL polling every frame (wrong for within-tab nav; creates new nodes on every URL change — see previous context)
- No distinction between presentation ops (reorder tabs) and semantic ops (delete node)
- Tile tree can't drive mutations yet
- No explicit conflict resolution for concurrent intents

### View/Mode System

**Current**: Implicit tile tree structure (detail view vs graph view)
- Graph-only: destroy webviews, save node list
- Detail-only: recreate webviews for saved nodes, show tile tree
- No explicit "mode" concept (browser vs history vs collaborative)

**Missing**: Policy layer for mode-dependent intent mapping

---

## What Is Planned But Not Implemented

### Intent System (2026-02-13_graph_tile_unification_plan.md)

**Phases planned**:
1. **UUID-based node identity** — Add uuid field, migrate from URL-based identity
2. **Servo signal wiring** — Implement notify_url_changed, request_create_new handlers
3. **UserGrouped edge type** — New edge type for user-initiated grouping
4. **Pane membership tracking** — Track which nodes belong to which pane
5. **Intent-based mutation** — Collect intents, apply at single sync point
6. **sync_to_graph replacement** — Replace polling with event-driven signal handlers

**What was designed but not implemented**:
- Intent enum with typing (semantic vs presentation)
- Intent reducer/apply_intents function
- Policy layer (mode-selectable action→intent mapping)
- Explicit conflict resolution rules
- Atomicity guarantees per intent type

### Architecture Research (2026-02-13_graph_tile_architecture_research.md)

**Conclusions** (supersedes earlier unification_plan sections):
- Structural parity is a trap
- Semantic parity + multi-authority model is better:
  - Graph authoritative for: identity, lifecycle, history edges
  - Tiles authoritative for: pane splits, tab order, active pane/tab
  - Webviews authoritative for: runtime instances only
- Intent types should distinguish presentation (tile-only) vs semantic (graph+tile+webview coordinated)
- Policy layer needed for mode-dependent intent mapping

---

## Comparison Table: Committed vs. Fully Designed

| Aspect | Committed | Designed | Gap |
|--------|-----------|----------|-----|
| **Node identity** | URL-based HashMap | UUID-based + url_to_node index | Phase 1 of plan |
| **Input system** | KeyboardActions booleans | Typed Intent enum | Entire intent system |
| **Servo signals** | URL polling (sync_to_graph) | Event-driven handlers | Phase 2 of plan |
| **Tile mutations** | Structure in place, handlers missing | Presentation intents (ReorderTabs, etc.) | Tile behavior implementation |
| **Graph mutations** | Direct (add_node, add_edge) | Intent reducer (apply_intent) | Intent system layer |
| **Multi-source serialization** | No (can race) | Intent queue with ordering | Serialization point missing |
| **Mode selection** | Implicit (detail vs graph view) | Policy-pluggable (browser/history/collab) | Policy layer + intent variants |
| **Conflict resolution** | N/A (no conflicts) | Declared per-intent pair | Rules not specified |
| **Atomicity guarantees** | Implicit | Declared per-intent | Boundaries undefined |
| **sync_to_graph** | Implemented (URL polling) | Removed, split across event handlers | Phase 6 cleanup |
| **Physics** | Custom worker + spatial hash | egui_graphs FruchtermanReingold | Physics migration plan |
| **Selection state** | Vec<NodeKey> in app + is_selected in node | HashSet<NodeKey> only | Selection consolidation |

---

## The Immediate Path Forward

### Option A: Build Intent System on Current Foundation (Higher Risk)

**Pros**:
- Incremental
- Smaller diffs for review
- Can land physics/selection migrations independently

**Cons**:
- Architecture research suggests intent typing might need redesign after Phase 2 (Servo signals)
- May need to refactor after multi-source concurrency emerges
- sync_to_graph is already creating wrong nodes (breaks browser semantics)

### Option B: Refactor for Semantic Parity First (Clean Slate)

**Pros**:
- Architecture research informs design upfront
- No rework after Phase 2
- Cleaner separation of concerns

**Cons**:
- Larger refactor upfront
- More complex initial diff
- Physics/selection still not migrated

---

## Key Architectural Decisions Needed

**1. Do we implement the full intent system, or simpler state mutations?**
- Committed current: Direct mutations (no intents)
- Design plan Phase 5: Full intent reducer with conflict resolution
- Research suggests: Intent types matter for multi-source concurrency

**Decision**: Consensus from research is that intent types are necessary for correctness (conflict resolution, atomicity guarantees). But can start simpler: just intents, defer policy layer.

**2. Do we migrate node identity now or after Servo signals?**
- Committed current: URL-based, breaks with duplicates
- Plan Phase 1: UUID-based
- Risk: sync_to_graph's URL polling can't distinguish duplicate-URL tabs

**Decision**: Should do Phase 1 early (UUID identity). Phase 2 (Servo signals) depends on it.

**3. Do we scaffold tiles first or build intent system first?**
- Committed current: Tiles scaffolded, handlers missing
- Design plan: Intent system Phase 5
- Research: Tile handlers need to emit intents

**Decision**: Tiles depend on intent system. Should build intents first, then tile handlers can emit them.

---

## Current Blockers

1. **sync_to_graph creates new nodes on every URL change** (wrong behavior for within-tab navigation)
   - Blocked by: Phase 2 (Servo signal wiring)
   - Workaround: None; current behavior is semantically wrong

---

## Research Conclusion (2026-02-15)

Recent fix attempts showed that patching around window-global routing does not fix the underlying semantic drift. The root issue is that navigation is still driven by polling (`sync_to_graph`) rather than delegate callbacks, so same-tab navigation is mis-modeled as new-node creation. The conclusion is to make `notify_url_changed` authoritative, remove polling-based node creation, and add an explicit intent boundary to serialize multi-source mutations. See NAVIGATION_NEXT_STEPS_OPTIONS.md for the decision options.

2. **no mode-dependent intent mapping**
   - Blocks: close_tab being either delete or hide depending on mode
   - Blocked by: Policy layer design

3. **Tile handlers can't emit mutations**
   - Blocked by: Intent system implementation
   - Currently: No tile-level handlers wired in tile_behavior.rs

4. **URL-only identity prevents duplicate tabs**
   - Blocked by: Phase 1 (UUID identity + persistence migration)
   - Workaround: Could use (url, created_at) as temporary composite key, but messy

---

## Recommendation: Phased Implementation Order

1. **Phase 1a: Define intent taxonomy** (small, 1-2 days)
   - Write Intent enum with all types
   - Define conflict resolution rules per pair
   - Write down atomicity boundaries

2. **Phase 1b: Implement intent reducer** (medium, 2-3 days)
   - Pure function: (state, intent) → new_state
   - Extensive tests for all combos
   - No side effects

3. **Phase 2a: UUID identity migration** (medium, 2-3 days)
   - Add uuid field to Node
   - Migrate url_to_node to HashMap<String, Vec<NodeKey>>
   - Update persistence types

4. **Phase 2b: Servo signal handlers** (medium, 2-3 days)
   - Implement notify_url_changed on WebViewDelegate
   - Implement request_create_new properly
   - Emit intents instead of direct mutations

5. **Phase 3: Tile behavior handlers** (medium, 2-3 days)
   - Implement tile_behavior.rs handlers
   - Emit tile intents (ReorderTabs, etc.)

6. **Phase 4: Policy layer** (small, 1-2 days)
   - Mode-selectable action→intent mapping
   - Test different modes (browser vs history)

7. **Phase 5+**: Physics migration, selection consolidation (not dependent on intent system)

---

## References

- [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) — Current behavioral spec
- [2026-02-13_graph_tile_unification_plan.md](implementation_strategy/2026-02-13_graph_tile_unification_plan.md) — Phased implementation plan (now partially superseded)
- [2026-02-13_graph_tile_architecture_research.md](2026-02-13_graph_tile_architecture_research.md) — Architecture design research (more recent, higher fidelity)
