<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Layout Strategy Plan

**Date**: 2026-02-18
**Status**: In Progress — implementation started

---

## Plan

### Context

GraphShell currently has one physics mode: Fruchterman-Reingold with fixed parameters. The goal
is a layout system where the algorithm adapts to data topology and user context. Different preset
configurations — and for non-flat topologies, position injection — drive layout without requiring
AGPL dependencies (ForceAtlas2 is out).

Five presets are introduced: **Peer** (current defaults), **Community** (adaptive repulsion for
clusters), **Dense**, **Sparse**, **Timeline** (y-axis temporal constraint). Three non-FR modes use
position injection: **Hierarchical** (Sugiyama via rust-sugiyama), **Radial** (ego network circle),
and **Barnes-Hut** (replaces FR for Community when N > 500).

### Architecture: Position Injection Pattern

egui_graphs owns positions internally between structural rebuilds. Positions can be overridden by
writing to `egui_state.graph.node_mut(key).set_location(pos)` after `GraphView` renders each frame.
This lets external algorithms drive positions while egui_graphs handles rendering and interaction.

Frame hooks in `render_graph_in_ui_collect_actions()` (`render/mod.rs`):

- **Hook A** (within `egui_state_dirty` rebuild, before `EguiGraphState::from_graph()`): run Sugiyama,
  write positions to `app.graph.node.position` so they are seeded into egui_graphs on rebuild.
- **Hook B** (after `get_layout_state`): Timeline y-injection, Radial ego injection, BH physics step.

`sync_graph_positions_from_layout()` in `tile_behavior.rs:198` then reads egui_state positions back
to `app.graph.node.position` each frame (existing behavior; compatible with all presets).

---

### Feature Target 1: LayoutPreset Enum + FR-based Presets

**Tasks**

- [x] New file `ports/graphshell/desktop/layout_preset.rs`:
  - `pub enum LayoutPreset { Peer, Community, Dense, Sparse, Timeline, Hierarchical, Radial }`
  - `impl LayoutPreset { fn label(), fn uses_fr(), fn is_position_injected() }`
  - `pub fn params_for(preset, node_count) -> FruchtermanReingoldState`
  - FR configs per preset (see Findings §FR Preset Parameters)
  - Community preset: adaptive `c_repulse = (0.55 + 0.03 * N.sqrt()).min(2.5)`
- [x] `desktop/mod.rs`: add `pub(crate) mod layout_preset;`
- [x] `app.rs`:
  - Add `pub layout_preset: LayoutPreset` and `pub timeline_newer_at_top: bool` fields
  - Add `GraphIntent::SetLayoutPreset(LayoutPreset)` variant
  - Add `set_layout_preset(&mut self, preset)` method (applies params, handles is_running, sets `egui_state_dirty` for Hierarchical)
  - Wire arm in `apply_intent()`
  - Initialize in `new_from_dir()` and `new_for_testing()`

**Validation Tests**

- `test_set_layout_preset_updates_physics_config` — each preset produces distinct c_repulse
- `test_community_preset_adaptive_repulsion` — c_repulse(N=100) > c_repulse(N=10)
- `test_preset_preserves_fr_running_state` — `is_running` preserved across FR preset switch
- `test_non_fr_preset_disables_fr` — Hierarchical/Radial set `is_running = false`
- `test_hierarchical_sets_egui_state_dirty` — switching to Hierarchical triggers rebuild flag

---

### Feature Target 2: Physics Panel Preset Selector + Timeline Direction

**Tasks**

- [x] `render/mod.rs` — `render_physics_panel()`:
  - Add preset selector (horizontal wrapped selectable labels) above existing sliders
  - Emit `GraphIntent::SetLayoutPreset(preset)` on click
  - Add Timeline direction toggle (visible only when `layout_preset == Timeline`):
    - "Newer at bottom" / "Newer at top" via `app.timeline_newer_at_top: bool`
  - Update Reset button to use `layout_preset::params_for(app.layout_preset, node_count)`
    instead of hard-coded `default_physics_state()`
- [x] `render/mod.rs` — Hook B: add `apply_post_frame_layout_injection(app)` call after `get_layout_state`
- [x] New fn `apply_post_frame_layout_injection(app)` dispatches on preset to injection fns
- [x] New fn `apply_timeline_y_positions(app)`:
  - Reads `node.last_visited` timestamps across all nodes
  - Lerps each node's y 5% per frame toward `target_y = (t - t_min) / range * y_span - y_span/2`
  - Direction inverted if `app.timeline_newer_at_top`

**Validation Tests**

- `test_timeline_y_direction_flag` — verify newer-at-top produces lower y target for recent node

---

### Feature Target 3: Hierarchical Layout (rust-sugiyama)

**Tasks**

- [x] `Cargo.toml`: add `rust-sugiyama = "0.4"` under non-Android deps section
  (petgraph 0.8.3 already present; compatible)
- [x] `render/mod.rs` — Hook A: in `egui_state_dirty` branch, if `layout_preset == Hierarchical`,
  call `apply_hierarchical_sugiyama(app, graph_for_render)` before `EguiGraphState::from_graph()`
- [x] New fn `apply_hierarchical_sugiyama(app, graph)`:
  - Call `rust_sugiyama::from_graph(&graph.inner).call()` → `HashMap<NodeIndex, (f32, f32)>`
  - Scale coordinates by `SCALE = 80.0` canvas units
  - Write to `app.graph.inner.node_weight_mut(idx).unwrap().position`
  - On error (cycles, disconnected): fall back silently (keep existing positions)

Note: `rust_sugiyama::from_graph` exact API must be verified against docs.rs/0.4.0 at impl time.
Our `graph.inner` is `StableGraph<Node, EdgeType, Directed>` which IS `StableDiGraph<Node, EdgeType>`.

**Validation Tests**

- `test_hierarchical_does_not_panic_on_cyclic_graph` — fallback works for cyclic input
- `test_hierarchical_positions_differ_from_defaults` — positions change after applying preset

---

### Feature Target 4: Radial Ego Layout

**Tasks**

- [x] `render/mod.rs` — Hook B dispatcher: if `layout_preset == Radial`, call
  `apply_ego_radial_positions(app, ego)` where `ego = selected_nodes.primary().or(hovered_graph_node)`
- [x] New fn `apply_ego_radial_positions(app, ego)`:
  - Read ego's current `egui_state` location (don't move the ego itself)
  - Collect unique neighbors: `out_neighbors(ego) ∪ in_neighbors(ego)` via HashSet dedup
  - Distribute on circle of radius 150.0 canvas units: `angle = TAU * i / n`
  - Soft-spring each neighbor toward its target: lerp 12% per frame
  - `egui_node.set_location(...)` for each neighbor

**Validation Tests**

- `test_ego_radial_neighbors_at_correct_angles` — mock egui_state, verify N neighbors at 360/N° intervals
- `test_ego_radial_noop_when_no_selection` — no-op when `primary()` and `hovered_graph_node` are None

---

### Feature Target 5: Barnes-Hut Physics Step (Scale > 500 nodes)

**Tasks**

- [x] `render/mod.rs` — Hook B dispatcher: if `layout_preset == Community && N > BH_NODE_THRESHOLD (500)`,
  call `apply_barnes_hut_physics_step(app)`; this path also sets `physics.is_running = false`
- [x] New struct `QuadTree` (~100 lines):
  - Fields: `bounds: Rect`, `center_of_mass: Pos2`, `mass: f32`, `children: Option<Box<[QuadTree; 4]>>`
  - `build(positions: &[(NodeKey, Pos2)]) -> Self`
  - `approximate_repulsion(pos: Pos2, theta: f32, strength: f32) -> Vec2` — BH traversal, theta=0.9
- [x] New fn `apply_barnes_hut_physics_step(app)`:
  - Read positions from egui_state (authoritative)
  - Build quadtree from all node positions
  - For each non-pinned node: compute BH repulsion + edge attraction force
  - Update `app.graph.node.velocity` (with damping) and `app.graph.node.position`
  - Write new position to `egui_state.graph.node_mut(key).set_location()`
  - Uses `app.physics.{c_repulse, c_attract, dt, damping}` for force parameters

**Validation Tests**

- `test_bh_threshold_controls_activation` — BH not called for N=499, called for N=501
- `test_bh_repulsion_is_nonzero` — two nearby nodes produce nonzero repulsion force
- `test_bh_pinned_nodes_not_moved` — pinned nodes keep their positions after BH step

---

### Feature Target 6: Design Doc + INDEX Update

**Tasks**

- [x] Write this file to: `implementation_strategy/2026-02-18_layout_strategy_plan.md`
- [x] Update `INDEX.md` Active Implementation Plans table with this doc

---

## Findings

### Rust Graph Layout Ecosystem Survey

| Crate | Algorithm | Status | License | Notes |
| --- | --- | --- | --- | --- |
| `forceatlas2` 0.8.0 | ForceAtlas2 (Barnes-Hut) | Active Oct 2025 | **AGPL v3** | Own graph type; AGPL incompatible with MPL |
| `fdg-sim` 0.9.1 | Custom spring + FR variants | Dormant Dec 2022 | Apache/MIT | petgraph-native; egui_graphs advanced demo uses it |
| `rust-sugiyama` 0.4.0 | Sugiyama layered DAG layout | Active Sep 2025 | MIT | petgraph StableDiGraph input; returns (x,y) coords |
| `dagre-rs` 0.1.0 | Dagre hierarchical | New Oct 2025 | ? | 437 LOC, single release; petgraph 0.8 |
| Graphviz bindings | dot, neato, sfdp, circo, twopi | Various | Various | C library required; batch layout only |

**Missing in Rust**: Kamada-Kawai/stress majorization, pure circular/radial layout, timeline layout,
UMAP-style, mixed-subgraph layouts.

### egui_graphs 0.29 Layout Extension Points

- `ForceAlgorithm` trait: `from_state(S) -> Self`, `step(&mut self, graph, viewport)`, `state(&self) -> S`
- `ExtraForce` trait: compose extra forces via `Extra<T, const ENABLED: bool>` tuple — changes `GraphView` type signature
- `Layout` trait: fully custom positioning (not force-based); `LayoutHierarchical` built-in but minimal
- `LayoutForceDirected<T: ForceAlgorithm>` — current usage with `FruchtermanReingold`
- **Position injection** (chosen approach): write `egui_node.set_location()` post-frame; avoids generics cascade; compatible with existing `GraphView` type

### FR Preset Parameters

Calibrated from research report §16 (D3-force constants) and §4 (Preset A):

| Preset | c_repulse | c_attract | k_scale | damping | FR running |
| --- | --- | --- | --- | --- | --- |
| Peer | 0.55 | 0.10 | 0.65 | 0.92 | preserved |
| Community | 0.55 + 0.03√N (≤2.5) | 0.06 | 0.80 | 0.88 | preserved |
| Dense | 0.25 | 0.15 | 0.45 | 0.95 | preserved |
| Sparse | 0.70 | 0.18 | 0.80 | 0.90 | preserved |
| Timeline | 0.40 | 0.12 | 0.55 | 0.93 | preserved |
| Hierarchical | 0.20 | 0.05 | 0.40 | 0.98 | **false** |
| Radial | 0.20 | 0.05 | 0.40 | 0.98 | **false** |

### Position Sync Architecture

- `app.graph.node.position` — seed for egui_graphs on structural rebuild; updated by `sync_graph_positions_from_layout()` each frame
- `egui_state.graph` — authoritative live positions (FR updates these); read via `node(key).location()`
- `node.velocity: Vector2D<f32>` — available on Node struct; unused by FR (FR manages own velocities); used by BH physics step
- `sync_graph_positions_from_layout()` called from `tile_behavior.rs:198`; reads egui_state → app.graph; pinned nodes restored

### rust-sugiyama API (to verify at impl time)

```rust
// Expected API based on docs.rs/rust-sugiyama/0.4.0:
rust_sugiyama::from_graph(&stable_di_graph)
    .call()
    // -> Result<HashMap<NodeIndex, (f32, f32)>, _>
```

Our `graph.inner` is `StableGraph<Node, EdgeType, Directed>` = `StableDiGraph<Node, EdgeType>`. Compatible.
Sugiyama produces integer-scale coordinates; multiply by SCALE=80.0 for canvas units.

---

## Progress

### 2026-02-18 — Session 1

- Surveyed Rust graph layout ecosystem
- Analyzed egui_graphs 0.29 `ForceAlgorithm` / `Layout` trait extension points
- Identified position injection as the right architecture (avoids generics cascade in `GraphBrowserApp`)
- Explored full codebase: `app.rs`, `render/mod.rs`, `graph/egui_adapter.rs`, `desktop/tile_behavior.rs`
- Located `sync_graph_positions_from_layout()` call site (`tile_behavior.rs:198`)
- Designed all 5 feature targets + Barnes-Hut threshold (N=500)
- Plan approved by user.

### 2026-02-18 — Session 2

- Implementation started.
- Feature Targets 1–6: all tasks completed.
