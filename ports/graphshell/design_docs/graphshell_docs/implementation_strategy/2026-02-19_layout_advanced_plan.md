<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Layout: Advanced Physics and Algorithms Plan (2026-02-19)

**Status**: Draft — implementation not started.

---

## Plan

### Context

The layout strategy plan (archived 2026-02-19, `archive_docs/checkpoint_2026-02-19/`) implemented
FR presets, position-injection architecture, Sugiyama hierarchical layout, radial ego layout, and
Barnes-Hut approximation (all Feature Targets 1–6 complete). This plan covers the next layer:

1. **Physics micro-improvements** (auto-pause, reheat, new-node placement) — operational quality
   items from research §5 and §2.6.
2. **Advanced layout algorithms** (degree-dependent repulsion, greedy label culling, invisible domain
   clustering) — from research §14.

Physics micro-improvements were originally listed in `2026-02-19_graph_ux_polish_plan.md §Phase 1`;
they are consolidated here to keep layout-system changes in one plan.

---

### Phase 1: Physics Micro-Improvements

#### 1.1 Auto-Pause on Convergence

Watch `last_avg_displacement < epsilon` while `is_running`. When crossed, set `is_running = false`.
Prevents wasted CPU and makes physics feel responsive (research §5.2, §16.4: users perceive a
perpetually-running simulation as broken within 2–3 seconds).

**Tasks**

- [ ] In `render_graph_in_ui_collect_actions()` post-physics-step, compare `last_avg_displacement`
  to `epsilon`. If below threshold and `is_running`, set `is_running = false`.
- [ ] Add `auto_pause_enabled: bool` toggle (default true) to physics state or `GraphBrowserApp`;
  expose in physics panel so power users can disable.
- [ ] Update physics info overlay: extend from 2-state ("Running" / "Paused") to 4-state
  ("Running" / "Settling" / "Settled" / "Paused"). "Settling" = displacement between `epsilon` and
  `epsilon × 10`; "Settled" = below epsilon while still running (transient before auto-pause fires).

**Validation Tests**

- `test_auto_pause_triggers_below_epsilon` — `is_running` becomes false when displacement < epsilon
  and `auto_pause_enabled`.
- `test_auto_pause_disabled_keeps_running` — when disabled, simulation keeps running past threshold.
- `test_physics_display_state_settling` — displacement = epsilon × 5, running → "Settling".
- `test_physics_display_state_settled` — displacement = epsilon × 0.5, running → "Settled".

---

#### 1.2 Reheat on Structural Change

Adding a node or edge while physics is paused leaves the new element physics-invisible: it occupies
a position but no forces act on it until the user manually re-enables physics. Research §5.3 calls
this confusing and inconsistent.

- In `apply_intent()`, when `AddNode` or `AddEdge` is applied and `is_loading_snapshot` is false,
  set `physics.is_running = true`.
- Reheat from current positions (do not reset forces or velocities).
- Guard: snapshot-restore paths must not trigger reheat.

**Tasks**

- [ ] In `apply_intent()` `AddNode` arm: set `physics.is_running = true` (guard on snapshot load).
- [ ] In `apply_intent()` `AddEdge` arm: same.
- [ ] Ensure `LoadGraphSnapshot` and `RestoreWorkspace` paths do not trigger reheat.

**Validation Tests**

- `test_add_node_reheats_physics_when_paused` — physics was paused; after `AddNode`, `is_running`
  is true.
- `test_add_edge_reheats_physics_when_paused` — same for `AddEdge`.
- `test_snapshot_restore_does_not_reheat` — after `LoadGraphSnapshot`, `is_running` retains
  pre-restore value.

---

#### 1.3 New Node Placement Near Topological Neighbors

New nodes currently spawn at center with jitter, placing them far from their parent. This triggers
large convergence displacements and breaks mental map preservation (research §2.6).

When a node is created via navigation from a parent (hyperlink follow, history back), initialize
its `position` near the parent. Manually-created nodes (keyboard `N`, omnibar) keep existing
center behavior.

**Tasks**

- [ ] Check what data is available in `AddNode` intent at creation time — verify whether
  `from_node: Option<NodeKey>` is carried or derivable from navigation context.
- [ ] If `from_node` is `Some`, compute spawn position as
  `from_node.position + jitter(radius: 60.0)`.
- [ ] Keep existing center-plus-jitter behavior for manually-created nodes.

**Validation Tests**

- `test_navigation_node_spawns_near_parent` — node created via navigation spawns within 100 canvas
  units of parent.
- `test_manual_node_spawns_at_center_region` — node created via `N` key spawns near canvas center.

---

### Phase 2: Advanced Layout Algorithms

#### 2.1 Degree-Dependent Repulsion (ForceAtlas2 Approximation)

Standard FR applies equal repulsion between all node pairs. Weighting repulsion by node degree
causes high-degree hub nodes to push neighbors further away — naturally spreading hub-and-spoke
topologies and separating communities without manual tuning (research §14.1, §14.3).

**Formula**: `Force = k * (deg(A) + 1) * (deg(B) + 1) / dist`

This single factor in the repulsion pass effectively pushes hubs apart. No AGPL dependency: this
is an approximation within the existing FR engine.

**Tasks**

- [ ] Locate the FR repulsion force computation in egui_graphs or the BH physics step in
  `render/mod.rs`.
- [ ] Introduce `degree_repulsion_enabled: bool` flag on physics state (default true; opt-out in
  physics panel).
- [ ] When enabled: multiply FR repulsion force by `(deg(A) + 1) * (deg(B) + 1)`. Requires node
  degree lookup — use `graph.inner.edges(key).count()` or pre-compute a degree map per frame.
- [ ] Scope: applies to all FR-based presets (Peer, Community, Dense, Sparse, Timeline). The BH
  path receives per-node degree weight in `apply_barnes_hut_physics_step()`.
- [ ] Expose toggle in physics panel alongside preset selector.

**Validation Tests**

- `test_degree_repulsion_hub_pushed_further` — star graph (hub + 5 leaves): hub-leaf equilibrium
  distance greater with degree repulsion enabled than disabled.
- `test_degree_repulsion_disabled_matches_standard_fr` — when disabled, force magnitude matches
  unweighted FR formula.
- `test_degree_zero_node_uses_unit_factor` — isolated node (degree 0): repulsion factor = 1;
  no division-by-zero, no zero or negative weighting.

---

#### 2.2 Greedy Label Occlusion Culling

At moderate graph sizes, label overlap makes the graph unreadable. Greedy occlusion culling
(research §14.2) is O(N log N) sort + O(N) placement pass — fast enough for 60FPS.

**Algorithm**:

1. Sort visible nodes by importance (degree centrality; fallback: `last_visited` recency).
2. Iterate sorted nodes. Maintain an occupied screen-space set (grid buckets or small quad structure).
3. If a node's label bounding box overlaps an already-drawn label, skip it.
4. Important nodes always show labels; clutter is strictly capped.

This is a display-layer operation — no graph data changes.

**Tasks**

- [ ] Implement label occlusion pass in `render/mod.rs` or `GraphNodeShape::ui()` caller.
- [ ] Compute label bounding box from node screen position + label string width estimate.
- [ ] Maintain occupied regions (grid cells keyed by screen bucket); skip occluded labels.
- [ ] Add `label_culling_enabled: bool` toggle (default true); expose in physics/display panel.

**Validation Tests**

- `test_label_culling_sort_by_degree` — two nodes with degrees 5 and 1; degree-5 node ranks first
  in the sorted pass.
- `test_label_culling_occlusion_detected` — two overlapping label rects → second entry is culled.
- `test_label_culling_disabled_shows_all` — when disabled, all labels rendered regardless of
  overlap.

---

#### 2.3 Invisible Domain Clustering Constraints

To visually group nodes by domain (e.g., all `wikipedia.org` nodes cluster together) without
introducing semantic graph edges, add invisible layout-only attraction forces (research §14.4).

**Technique**: During `apply_post_frame_layout_injection()` (Hook B), for each pair of same-domain
nodes, compute additional centroid attraction force. These are layout hints only — they MUST NOT
be persisted to the graph log or appear in serialized state.

Rationale for centroid attraction over phantom edges (§14.4 alternative): avoids egui_graphs state
leakage and keeps the approach strictly external to the graph model.

**Tasks**

- [ ] Parse registered domain from node URL (eTLD+1 or host) — add a small utility fn or reuse
  existing URL parsing.
- [ ] In `apply_post_frame_layout_injection()` Hook B: group nodes by domain; compute per-domain
  centroid from current positions.
- [ ] Apply weak attraction force from each node toward its domain centroid
  (`k_cluster ≈ 0.05`, long-range soft force compatible with §14.7 attractor-point model).
- [ ] Ensure: these forces are NEVER written to `LogEntry`, `Graph`, or any persistence path.
- [ ] Add `domain_clustering_enabled: bool` flag (default false — experimental); expose in physics
  panel.

**Validation Tests**

- `test_domain_centroid_computed_correctly` — three nodes with same domain → centroid =
  mean position.
- `test_domain_clustering_does_not_persist` — after applying constraints and serializing graph,
  no additional edges or fields are present.
- `test_domain_clustering_noop_for_single_domain_node` — a node with a unique domain receives
  no clustering force.
- `test_different_domains_not_clustered_together` — nodes from two different domains do not
  attract each other.

---

## Findings

### Architecture Continuity

Physics micro-improvements use the existing `physics.is_running`, `last_avg_displacement`, and
`epsilon` fields. No new structs required for Phase 1.

Degree-dependent repulsion requires node degree during the force pass. For FR, this means passing
a degree map to the repulsion loop. Check whether egui_graphs' internal FR loop exposes a hook;
if not, the BH path (`apply_barnes_hut_physics_step`) is the better initial target for Phase 2.1
since it already reads `app.graph` directly.

Label culling operates in screen space only; no physics integration required.

Domain clustering via centroid attraction is the recommended approach over phantom edges (cleaner
separation, avoids egui_graphs state leakage, matches §14.7 soft-force model).

### Relationship to Archived Layout Plan

All 6 feature targets in `archive_docs/checkpoint_2026-02-19/2026-02-18_layout_strategy_plan.md`
are complete. That plan's position-sync architecture and FR preset parameter tables remain the
reference for existing physics configuration — this plan does not supersede them.

### Research Cross-References

- Phase 1.1: §5.2 (auto-pause perception), §16.4 (convergence UX rule)
- Phase 1.2: §5.3 (reheat on structural change)
- Phase 1.3: §2.6 (mental map preservation, neighbor placement)
- Phase 2.1: §14.1 (FA2 degree repulsion formula), §14.3 (Forest of Fireflies topology)
- Phase 2.2: §14.2 (greedy occlusion culling)
- Phase 2.3: §14.4 (WebCola invisible constraints), §14.7 (attractor-point soft forces)

---

## Progress

### 2026-02-19 — Session 1

- Plan created from research report §2.6, §5, and §14.
- Physics micro-improvements consolidated here from `2026-02-19_graph_ux_polish_plan.md §Phase 1`
  (removed from that plan to eliminate redundancy per DOC_POLICY §2).
- Phases 1 and 2 have full task lists and unit test stubs.
- Implementation not started.
