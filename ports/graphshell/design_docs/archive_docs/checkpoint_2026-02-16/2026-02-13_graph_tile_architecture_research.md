# Graph-Tile Architecture: Research & Analysis

**Status**: Research / Design Exploration  
**Date**: February 13, 2026  
**Purpose**: Document evolving architectural thinking on graph/tile/webview relationships, parity models, and intent-based mutation.

⚠️ **Disclaimer**: This document captures ongoing research and design debate. It is NOT yet a specification or commitment. Conclusions may change as implementation proceeds. Use this to understand the design space, not as a source of truth for implementation decisions.

---

## Problem Statement

Graphshell must coordinate three interacting systems:
- **Graph**: Persistent model of browsing (nodes = pages, edges = navigation)
- **Tile Tree**: Ephemeral workspace layout (panes, tabs, splits, ordering)
- **Webviews**: Runtime instances (live processes, rendering, input)

**Challenge**: How to keep these systems consistent without creating sync bugs (the "servoshell tab UI problem"), while supporting multiple semantic modes (browser, history, P2P collaborative) and avoiding false constraints.

---

## Design Evolution

### Phase 1: Structural Parity (Rejected)

**Initial hypothesis**: Graph and tile tree should mirror each other (1:1 mapping).

**Rationale**: 
- Simplifies reasoning ("they're the same thing viewed differently")
- Prevents divergence
- Single source of truth

**Problems identified**:

1. **Semantic-structural mismatch**
   - Graph stores positions, velocity, physics state
   - Tiles store layout, split ratios, focus order
   - These are fundamentally different concerns with different update frequencies

2. **Inactive nodes expose the trap**
   - If node exists in graph but has no webview (inactive state), does it appear in tiles?
   - **Yes**: tile tree cluttered with invisible nodes; breaks usability
   - **No**: graph and tiles are NOT in parity—you've already admitted one can have things the other doesn't

3. **Filtered views incompatible with parity**
   - Search the graph: "show only nodes from today"
   - Group by origin: "show github.com tabs together"
   - Sort by recency: "most-recent tabs first"
   - **Problem**: Parity means any reordering/filtering either breaks the invariant or must create graph mutations
   - **Reality**: A real browser can display filtered views without changing underlying state

4. **Collaborative scenarios blocked**
   - Two users with shared graph but personal layouts
   - One user reorders panes → forces structure on other user's graph
   - User B can't have different layout without User A's graph changing

5. **Encoding UI layout as semantic structure**
   - If tiles determine graph, then "I dragged tab A and tab B into the same pane" creates a UserGrouped edge
   - But the edge wasn't user-intent—it was layout incidental
   - Conflates intent with presentation

**Conclusion**: Structural parity is a false constraint that prevents flexibility without solving the actual synchronization problem.

---

### Phase 2: One-Way Data Flow (Incomplete)

**Hypothesis**: Only graph mutates; tiles are read-only derived views.

**Rationale**:
- Eliminates sync problem (no contradictory mutations if one direction is immutable)
- Simplifies reasoning
- Graph is authoritative

**Problems with absolutism**:

1. **Some tile actions ARE semantic**
   - "Close tab" in browser mode is semantic (tab deleted), not just hidden
   - Should not be treated as pure presentation layer
   - Same user action means different things in different modes

2. **Multi-source concurrency still exists**
   - Even if tiles never mutate graph, you have multiple writers:
     - Servo events (`request_create_new`, `notify_url_changed`)
     - Keyboard commands (N = new tab, Del = delete)
     - Graph gestures (drag node, delete node)
     - Persistence restore (load 50 nodes)
   - Single direction doesn't serialize these; you still need intents to handle conflicts

3. **Overly restrictive semantics**
   - "Close tile should not delete node" doesn't fit browser mode
   - Should be policy-dependent (mode-selectable), not absolute
   - Architecture should support both semantics, not hardcode one

**Conclusion**: Directionality alone doesn't solve concurrency. Need explicit operation typing.

---

### Phase 3: Semantic Parity + Intent-Based Mutation (Current Best Model)

**Hypothesis**: 
- Graph and tiles have **semantic parity** (same node identity, lifecycle, intent-driven edges), not structural parity
- **Multiple authorities** for different concerns (graph for identity, tiles for layout, webviews for runtime)
- **Typed intents** distinguish presentation operations (tile-only) from semantic operations (graph + tiles + webview coordinated)
- **Policy-pluggable** handlers map user actions to intents based on mode

**Authority domains**:

| Domain | Authoritative For | Mutable By |
|--------|------------------|----------|
| **Graph** | Node identity (UUID), lifecycle, history/provenance edges (Hyperlink, History) | Semantic intents (NewTab, DeleteNode, GroupNodes) + persistence restore |
| **Tile Tree** | Pane splits, tab ordering, active pane/tab, visibility/focus | Presentation intents (ReorderTabs, ResizePane, FocusPane) + manual layout |
| **Webviews** | Live runtime instances, rendering, input handling | Derived from graph state + tile focus + policy layer |

**Intent taxonomy**:

Intents fall into three categories with different mutation patterns:

```
PRESENTATION OPS (Tile-only, idempotent):
  - ReorderTabs(pane_id, from: index, to: index)
  - ResizePane(pane_id, width, height)
  - FocusPane(pane_id)
  - FocusTab(pane_id, tab_index)
  → Updates: tiles.pane[id].tab_order, tiles.pane[id].focus

SEMANTIC OPS (Graph + Tile + Webview coordinated):
  - NewTab(parent_node: NodeKey, url: Url)
  - DeleteNode(node_key: NodeKey)
  - GroupNodes(nodes: Vec<NodeKey>, edge_type: EdgeType)
  - HideNode(node_key: NodeKey)  // Deactivate, don't delete
  - RestoreNode(node_key: NodeKey)  // Reactivate from history
  - NavigateWithinTab(node_key: NodeKey, url: Url)  // URL change, same node
  → Updates: graph, tile visibility, webview lifecycle

RUNTIME OPS (Webview layer, derived):
  - CreateWebview(node_key: NodeKey)
  - DestroyWebview(webview_id: WebViewId)
  - SuspendWebview(webview_id: WebViewId)
  - ResumeWebview(webview_id: WebViewId)
  → Updates: webview registry only; graph/tiles unchanged
```

**Critical invariants**:

```
1. Tile references must point to existing graph nodes
   ∴ Closing a tile never orphans a graph node (node.lifecycle = Inactive, not deleted)
   ∴ Deleting a node removes its tile(s) automatically

2. Graph nodes may exist without tiles
   ∴ Inactive nodes acceptable (historical, searchable, restorable)
   ∴ No requirement that "all graph nodes appear in current layout"

3. Close intent is mode-dependent
   ∴ Browser mode: CloseTab → DeleteNode intent
   ∴ History mode: CloseTab → HideNode intent
   ∴ Policy layer decides mapping, same UI action, different intents

4. No direct cross-layer mutation
   ∴ All updates go through intent reducer
   ∴ Multi-source events serialized into intent queue
   ∴ Intents applied in defined order with conflict rules
```

**Conflict resolution examples**:

```
Scenario 1: User closes tab while persistence is restoring
  Time 1: Restore intent adds node_X to graph
  Time 2: CloseTab intent arrives for (unknown) node_Y
  Time 3: RestoreComplete intent finishes

  → Both intents serialized, applied sequentially
  → If node_Y doesn't exist yet, CloseTab intent either waits or fails explicitly

Scenario 2: Servo event + keyboard event same frame
  Time 1: Servo notify_url_changed(webview_id, url)
  Time 2: User presses N (new tab)
  
  → Both generate intents (NavigateWithinTab, NewTab)
  → Reducer applies both: existing tab navigates + new tab created
  → No contradiction (different nodes)

Scenario 3: Conflicting mode semantics
  Browser mode: close = delete
  History mode: close = hide
  
  → Gesture handler emits DeleteNode or HideNode depending on app.mode
  → Reducer doesn't know or care about mode (it's in the handler, not the reducer)
  → Same node/tile, different operations based on mode
```

**Reducer pattern**:

```rust
fn apply_intent(app: &mut App, intent: Intent) {
    match intent {
        Intent::NewTab(parent, url) => {
            let new_node = graph.add_node(url, generate_uuid());
            graph.add_edge(parent, new_node, EdgeType::Hyperlink);
            tiles.add_to_parent_pane(parent, new_node);
            webview_lifecycle.mark_create_pending(new_node);
        }
        Intent::DeleteNode(node) => {
            graph.remove_node(node);
            tiles.remove_from_all_panes(node);
            webview_lifecycle.mark_destroy_pending(node);
        }
        Intent::HideNode(node) => {
            node.lifecycle = Inactive;
            tiles.remove_from_all_panes(node);  // Still in graph, just not visible
            webview_lifecycle.mark_destroy_pending(node);
        }
        Intent::ReorderTabs(pane_id, from, to) => {
            tiles.panes[pane_id].reorder_tabs(from, to);
            // Graph unchanged
        }
        // ... other intents
    }
}
```

---

## Remaining Design Tensions

### 1. Intent Atomicity Boundaries

**Question**: When `Intent::NewTab(parent, url)` is applied, what's atomic?

- Just the node creation in graph?
- Graph + tile insertion?
- Graph + tile + webview pipeline initialization?

**Different answer for different failure modes**:
- If webview create fails, is the graph node rolled back? (Probably yes)
- If tile insertion fails, is the node deleted? (Probably no)
- If persistence-log fails, is the whole intent retried? (Probably yes)

**Decision needed**: Define per-intent what counts as "successfully applied" vs "failed, no side effects".

### 2. Policy-Pluggable Handlers

**Question**: Where does mode selection happen?

Option A: Policy object in gesture handlers
```rust
struct TileGesturePolicy { mode: AppMode }

impl TileGesturePolicy {
    fn on_close_tab(&self, node_id: NodeKey) -> Intent {
        match self.mode {
            AppMode::Browser => Intent::DeleteNode(node_id),
            AppMode::History => Intent::HideNode(node_id),
        }
    }
}
```
**Pro**: Intents clean, handlers mode-aware  
**Con**: Handlers become stateful, duplicated logic per mode

Option B: Mode-parameterized intents
```rust
enum Intent {
    CloseTab { node_id: NodeKey, mode: CloseMode::Delete | Hide }
    // ...
}
```
**Pro**: Intent carries full context  
**Con**: Intent enum bloats, reducer needs match on inner mode

Option C: Separate intent sets per mode
```rust
enum BrowserIntent { DeleteTab, ClosePane, ... }
enum HistoryIntent { HideTab, DeactivatePane, ... }
```
**Pro**: Clear separation, no mode checks in reducer  
**Con**: Massive code duplication, transformation layer between modes

**Research status**: Option A (policy object) seems cleanest, but needs careful implementation to avoid handler duplication.

### 3. Cascade Dependencies

**Question**: What happens when one intent's handler needs to emit follow-up intents?

```
User clicks "close all tabs in pane"
  Intent: ClosePane(pane_id)
  Handler iterates tabs: [node_A, node_B, node_C]
  Emits three intents: DeleteNode(A), DeleteNode(B), DeleteNode(C)
  
Reducer processes all three, graph consistency maintained.
```

**But what about async?**
```
User activates a node with no webview
  Intent: ActivateTab(node_id)
  Handler: mark node as focus
  Separate component: "detect focus change, create webview" (async)
  
If webview creation fails, does the focus change roll back?
```

**Research status**: Need to distinguish between:
- Synchronous follow-up intents (safe to emit from handler)
- Async effects (webview creation, network requests, rendering)
- Effects that need rollback guarantees

### 4. Transient State During Load

**Question**: Are graph/tile/webview invariants enforced during restoration, or only at "loaded" state?

```
Restore phase 1: Load 50 graph nodes from snapshot
Restore phase 2: Load tile layout referencing nodes 1-50, plus node 51 (not yet loaded)
Restore phase 3: Load webviews for visible nodes

At end of phase 2, invariant "tiles → nodes exist" is broken.
Is this acceptable? Or must restoration be atomic?
```

**Research status**: Probably need a "restore complete" barrier—invariants relaxed during restoration, enforced after.

---

## Comparison Table: Models Evaluated

| Aspect | Structural Parity | One-Way Flow | Semantic Parity + Intents |
|--------|-------------------|--------------|--------------------------|
| **Supports filtered views** | ✗ | ✓ | ✓ |
| **Supports multi-mode semantics** | ✗ | ✗ | ✓ |
| **Prevents sync bugs** | ✓ (overly) | ✗ (incomplete) | ✓ (via intent serialization) |
| **Supports P2P collaboration** | ✗ | ~ (with work) | ✓ |
| **Supports inactive nodes** | ✗ | ✓ | ✓ |
| **Allows tile reordering free** | ✗ | ✓ | ✓ |
| **Complexity of implementation** | Low | Low-Medium | Medium-High |
| **Flexibility for future features** | Low | Medium | High |

---

## Key Insights

1. **Parity is a trap**
   - False constraint that doesn't solve sync problems
   - Structural parity prevents exactly the features we want (filtering, layout independence, multi-mode)
   - Semantic parity (identity, lifecycle, explicit intents) is what actually matters

2. **Directionality is insufficient**
   - Even one-way data flow doesn't handle multi-source concurrency
   - Intents solve the concurrency problem, not directionality
   - Multiple writers require serialization, not read-only views

3. **Mode selection is a policy layer**
   - Same user action (close tab) means different things in different modes
   - Should not be hardcoded in reducer or architecture
   - Policy object or configuration layer needed to map action → intent

4. **Atomicity is subtle**
   - Need clear boundary between what counts as "successfully applied"
   - Distinguish sync vs async effects, graph mutations vs side effects
   - Restoration phases may relax invariants temporarily

5. **The real constraint is reference integrity**
   - Graph nodes may exist without tiles (fine)
   - Tiles must reference existing nodes (critical)
   - Deletion must cascade from graph to tiles, not vice versa

---

## Recommendations for Implementation

1. **Start with intent taxonomy**
   - Define presentation vs semantic intents explicitly
   - Write down conflict resolution rules for each pair
   - No implementation before intent contracts are clear

2. **Implement reducer logic first**
   - Pure functions mapping (state, intent) → new_state
   - Write exhaustive tests for all intent combinations
   - No effects/side effects in reducer, only state updates

3. **Build policy layer separately**
   - Gesture handlers → intents mapping is mode-pluggable
   - Same gesture produces different intents in different modes
   - Policy can be runtime-selectable (no recompile)

4. **Define atomicity guarantees per intent**
   - Which updates make/break ref integrity?
   - Which updates need to be logged to persistence?
   - What rollback behavior on failure?

5. **Design restoration barrier**
   - Distinguish "loading" vs "loaded" state
   - Relax invariants during loading
   - Enforce them after completion

---

## Open Questions for Future Design Sessions

1. Should intents be synchronous only, or can they spawn async tasks?
2. What's the interaction between intent reducer and persistence logging? (Log before apply? After? Transactional?)
3. For P2P collaboration, how do we merge conflicting intents from different users? (Operation-based? State-based Crdt?)
4. Should webview creation be in the intent reducer, or in a separate effect system?
5. How do we handle undo/redo with intents? (Record to separate stack? Emit reverse intents?)

---

## References

- [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) — Current behavioral specification
- [2026-02-13_graph_tile_unification_plan.md](implementation_strategy/2026-02-13_graph_tile_unification_plan.md) — Implementation plan (partially outdated by this research)
- [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) — Current architecture snapshot
