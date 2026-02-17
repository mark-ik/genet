# Physics Selection Plan (2026-02-12)

## Physics Selection Plan

- Goal: Replace custom physics/worker stack (~720 LOC + ~110 LOC spatial) with egui_graphs force-directed layout; consolidate duplicated selection state to one source of truth.
- Scope: Graphshell runtime state, render integration, dependency cleanup, and tests. No Servo core changes.
- Sequencing: Physics migration and selection consolidation happen before FT2 thumbnail completion.
- Net effect: Delete ~830 LOC across 3 files + 1 module, add ~80 LOC pin-aware wrapper. Remove `kiddo` dependency.

### Phase 1: Physics Engine Migration

**What we're replacing:**

| File | LOC | Role | Action |
| ---- | --- | ---- | ------ |
| `physics/mod.rs` | 459 | Custom force engine (repulsion, springs, damping, auto-pause) | Delete |
| `physics/worker.rs` | 262 | Background thread with crossbeam channels | Delete |
| `physics/spatial_hash.rs` | 9 | Re-export shim for `graph::spatial` | Delete |
| `graph/spatial.rs` | 110 | KD-tree wrapper (kiddo), used only by physics | Delete |

**Why replace:**

1. **Correctness bug**: Springs applied over both `out_neighbors` AND `in_neighbors`, doubling effective attraction per edge. egui_graphs FR uses `neighbors_undirected()` counting each edge once.
2. **Unnecessary complexity**: Background worker thread clones entire `Graph` over crossbeam channel. The graph is a spatial tab manager (like Tree Style Tabs but force-directed): each node is a tab, edges encode navigation relationships. Power users with 100+ tabs produce ~100-300 nodes. egui_graphs FR with Newton's 3rd law symmetry handles this synchronously on the UI thread: ~45K pair checks at 300 nodes is trivially fast. O(n²) only becomes a concern at 3000-5000+ nodes.
3. **No shutdown**: Worker thread runs `loop {}` with no `Shutdown` command variant.
4. **Duplicated concerns**: Custom engine reimplements repulsion/attraction/damping/convergence that egui_graphs already provides correctly.

**What we're switching to:**

egui_graphs 0.29 provides `FruchtermanReingold`, a correct force-directed layout accessible through the `ForceAlgorithm` trait:

```text
ForceAlgorithm trait:
  fn from_state(state: Self::State) -> Self
  fn step(g: &mut Graph, view: Rect)
  fn state() -> Self::State

FruchtermanReingoldState fields:
  is_running, dt, epsilon, damping, max_step,
  k_scale, c_attract, c_repulse,
  last_avg_displacement, step_count
```

The layout is integrated via `GraphView` type parameters: replace `LayoutStateRandom, LayoutRandom` with `FruchtermanReingoldState, ForceDirected<FruchtermanReingold>` (or custom wrapper).

**Pin support:**

egui_graphs FR's `apply_displacements()` does NOT check for pinned nodes. Our graph has `Node.is_pinned`. Two options:

- **Option A (preferred)**: Implement a custom `ForceAlgorithm` that wraps `FruchtermanReingold` and skips displacement for pinned nodes. The `step()` method delegates to FR's internal functions (`compute_repulsion`, `compute_attraction`) then applies a modified `apply_displacements` that checks `node.payload().is_pinned`. Approximately 60-80 LOC.

- **Option B**: Use egui_graphs' `dragged()` flag as a proxy for pinned. Set `dragged = true` on pinned nodes before each layout step, clear after. Fragile: conflates two concepts.

**Implementation steps:**

1. Create `PinnedFruchtermanReingold` implementing `ForceAlgorithm`:
   - Delegates force computation to FR internals
   - Skips displacement for nodes where `payload().is_pinned == true`
   - Exposes same `FruchtermanReingoldState` (or thin wrapper adding pin config)

2. Update `render/mod.rs`:
   - Change `GraphView` type parameters from `LayoutStateRandom, LayoutRandom` to the new FR layout types
   - Remove physics-related imports (`PhysicsConfig`, etc.)

3. Update `app.rs`:
   - Remove `physics: PhysicsEngine` field
   - Remove `physics_worker: Option<PhysicsWorker>` field
   - Remove `update_physics()` method and all `PhysicsCommand`/`PhysicsResponse` handling
   - Physics toggle (`T` key) maps to `FruchtermanReingoldState.is_running` toggle
   - Config panel adjusts FR parameters instead of `PhysicsConfig`

4. Delete files:
   - `physics/mod.rs`, `physics/worker.rs`, `physics/spatial_hash.rs`
   - `graph/spatial.rs`
   - Remove `pub mod physics;` from crate root
   - Remove `pub mod spatial;` from `graph/mod.rs`

5. Remove `kiddo` from `Cargo.toml` (only consumer was `graph/spatial.rs`)

6. Update `gui.rs` frame order:
   - Remove physics update step (egui_graphs FR runs inside `GraphView::show()`)
   - Physics toggle sends state change to layout, not to worker thread

**Preserved user-visible behavior:**

- `T` key toggles physics on/off (maps to `state.is_running`)
- Config panel adjusts force parameters (maps to FR state fields)
- Pinned nodes stay fixed under layout
- Interaction pause during drag (egui_graphs handles this via `dragged()` flag)
- Auto-convergence (FR tracks `last_avg_displacement`, can auto-set `is_running = false`)

**Test migration:**

| Current test | Location | Action |
| ------------ | -------- | ------ |
| 13 PhysicsEngine unit tests | `physics/mod.rs` | Delete (replaced by FR correctness) |
| 7 worker process_command tests | `physics/worker.rs` | Delete |
| 3 SpatialGrid tests | `graph/spatial.rs` | Delete |
| Physics integration in app tests | `app.rs` | Update to use FR state toggle |
| render_graph tests | `render/mod.rs` | Update type parameters |

New tests to add:

- Pin-aware FR: pinned node doesn't move after N steps
- FR convergence: `last_avg_displacement` decreases over steps
- Toggle: `is_running` flag respected by layout

### Phase 2: Selection State Consolidation

**Current state (duplicated):**

1. `app.selected_nodes: Vec<NodeKey>` (app.rs:59) - authoritative for multi-select logic
2. `node.is_selected: bool` (graph/mod.rs:47) - read by egui_adapter for render color

`select_node()` (app.rs:181-202) iterates ALL nodes to clear `is_selected` on every single-select: O(n) per click.

**Target state:**

1. Replace `Vec<NodeKey>` with `HashSet<NodeKey>` in `app.rs` for O(1) contains/insert/remove
2. Remove `Node.is_selected` field from `graph/mod.rs`
3. `egui_adapter.rs` sync function reads from `HashSet` to set egui_graphs node `selected` property

**Implementation steps:**

1. Change `selected_nodes: Vec<NodeKey>` to `selected_nodes: HashSet<NodeKey>` in `GraphBrowserApp`
2. Remove `is_selected` field from `Node` struct in `graph/mod.rs`
3. Update `select_node()`:
   - Single-select: `self.selected_nodes.clear(); self.selected_nodes.insert(key);`
   - Multi-select: `self.selected_nodes.insert(key);`
   - No more O(n) loop over all nodes
4. Update `egui_adapter.rs` sync function (~line 227):
   - Instead of reading `node.payload().is_selected`, accept `&HashSet<NodeKey>` parameter
   - Look up selection status from set: `selected_nodes.contains(&node_key)`
5. Update all test files that set `node.is_selected = true` to use `app.select_node()` instead

**Files changed:**

| File | Change |
| ---- | ------ |
| `app.rs` | `Vec` -> `HashSet`, simplify `select_node()` |
| `graph/mod.rs` | Remove `is_selected` field, update `Node::new()` |
| `graph/egui_adapter.rs` | Accept selection set, query it instead of node field |
| `render/mod.rs` | Pass selection set to adapter sync |
| Tests in graph/mod.rs | Remove `is_selected` assertions |
| Tests in egui_adapter.rs | Update selection setup |

### Phase 3: Dependency and Config Cleanup

1. Remove `kiddo = "4.2"` from `Cargo.toml`
2. `crossbeam-channel` stays (used by `running_app_state.rs`)
3. Update `PhysicsConfig` panel in gui.rs to expose FR parameters:
   - `c_attract` (attraction coefficient, replaces `spring_strength`)
   - `c_repulse` (repulsion coefficient, replaces `repulsion_strength`)
   - `damping` (maps directly)
   - `k_scale` (ideal spacing scale)
4. Remove or archive `PhysicsConfig` struct

### Validation Tests

**Physics migration:**

- [ ] Existing physics/render interaction tests pass after type parameter changes
- [ ] No edge-force double application (regression against bidirectional spring bug)
- [ ] Pinned nodes remain fixed under layout steps
- [ ] Physics toggle (T key) pauses/resumes FR layout
- [ ] Config panel adjusts force parameters and layout responds
- [ ] Graph with 50 nodes converges to stable layout within 5 seconds

**Selection consolidation:**

- [ ] Single-select and multi-select behavior unchanged from user perspective
- [ ] Selection survives graph interactions (drag, focus, delete)
- [ ] No O(n) iteration on single-select (HashSet operations)
- [ ] egui_graphs renders selected nodes with correct visual treatment

**Cleanup:**

- [ ] `kiddo` removed from Cargo.toml, `cargo build` succeeds
- [ ] No dead code warnings from removed physics modules
- [ ] Test count recovers after removing 23 physics tests and adding ~6 FR tests

## Findings

- Custom physics has a correctness bug: spring attraction applied over both `out_neighbors` and `in_neighbors`, doubling effective spring force per edge. egui_graphs FR uses `neighbors_undirected()` which counts each edge once (correct).
- egui_graphs FR is O(n^2) all-pairs repulsion with Newton's 3rd law symmetry optimization (n*(n-1)/2 pairs). The graph is a spatial tab manager: nodes are tabs, edges are navigation relationships. Power users with 100+ tabs produce ~100-300 nodes; at 300 nodes, ~45K pair checks per frame is trivially fast. A spatial index (Barnes-Hut or KD-tree) becomes necessary around 3000-5000+ nodes; if we reach that scale, contributing spatial acceleration upstream to egui_graphs is preferable to maintaining a local fork.
- `ForceAlgorithm` trait allows pluggable custom implementations. Pin support is best added at the `apply_displacements` step: skip nodes where `payload().is_pinned`. Approximately 60-80 LOC wrapper.
- egui_graphs also has an `ExtraForce` trait for adding custom forces to the displacement buffer, but this doesn't help with pin support (it adds forces, doesn't skip displacement).
- `kiddo` KD-tree is only used by `graph/spatial.rs` which is only consumed by custom physics. Safe to remove.
- `crossbeam-channel` is also used by `running_app_state.rs`, so it stays even after physics worker deletion.
- Selection duplication (`Vec<NodeKey>` + `Node.is_selected`) causes O(n) clear on every single-select and creates coherence risk between two sources of truth.
- FR state is serializable (`Serialize`/`Deserialize`), so physics config can persist through the same redb snapshot path if needed.

## Progress

- 2026-02-12: Plan created (initial skeleton).
- 2026-02-12: Confirmed sequencing: physics migration -> selection consolidation -> FT2 thumbnails -> FT6 search.
- 2026-02-12: Comprehensive plan written with file-level deletion inventory, egui_graphs API analysis, pin support approach, test migration plan, and dependency cleanup.
