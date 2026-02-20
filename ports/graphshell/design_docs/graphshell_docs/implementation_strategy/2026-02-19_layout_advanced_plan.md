<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Layout: Advanced Physics and Algorithms Plan (2026-02-19)

**Status**: Draft — implementation not started.

---

## Plan

### Context

The layout strategy plan (archived 2026-02-19, `archive_docs/checkpoint_2026-02-19/`) listed FR
presets, position-injection architecture (Hook B), Sugiyama hierarchical layout, radial ego layout,
and Barnes-Hut approximation as Feature Targets 1–6. Those targets have [x] marks in the archived
plan, but **none exist in the current codebase**: there are no physics presets, no Hook B, no
Sugiyama, no radial ego layout, and no Barnes-Hut implementation. The actual foundation is basic FR
with center gravity (`FruchtermanReingoldWithCenterGravityState`) and a parameter-slider physics
panel with `physics.base.{is_running, last_avg_displacement, epsilon}`.

This plan covers what is buildable on that actual foundation:

1. **Physics micro-improvements** (auto-pause, reheat, new-node placement) — operational quality
   items from research §5 and §2.6.
2. **Advanced layout algorithms** (greedy label culling, degree-dependent repulsion, invisible domain
   clustering) — from research §14, ordered by implementation confidence.

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

- [ ] **Research first**: trace `AddNode` through the Servo navigation pipeline to determine whether
  the originating `NodeKey` (the node whose link was followed) is available at intent-creation time,
  or must be threaded explicitly through the navigation event chain. This is the blocking question —
  do not write spawn logic until the data availability is confirmed.
- [ ] If `from_node` data is available or cheaply plumbable: compute spawn position as
  `from_node.position + jitter(radius: 60.0)`.
- [ ] If not available without significant pipeline changes: document the gap and defer; the spawn
  logic is straightforward once the data exists.
- [ ] Keep existing center-plus-jitter behavior for manually-created nodes (`N` key, omnibar)
  regardless of outcome above.

**Validation Tests**

- `test_navigation_node_spawns_near_parent` — node created via navigation with `from_node: Some(k)`
  spawns within 100 canvas units of parent. (Only write once from_node availability is confirmed.)
- `test_manual_node_spawns_at_center_region` — node created via `N` key spawns near canvas center.

---

#### 1.4 Canvas Gravity

Standard FR has no centering force; nodes can drift off-screen with low graph density or after
a high-repulsion preset is applied. A weak gravity force pulls all nodes toward the canvas center
each frame, preventing the graph from disintegrating. Research §8.3.

`FruchtermanReingoldWithCenterGravityState` (the state already in use) includes center gravity —
verify whether its current strength is sufficient before adding a supplemental force. If the
built-in gravity is too weak (nodes still escape to screen edges in practice), add a configurable
strength parameter.

**Tasks**

- [ ] **Verify first**: at default settings, can nodes drift off-screen? Use a headed test with a
  high-repulsion preset and ~10 isolated nodes; observe whether they stay on canvas.
- [ ] If drift is observed: expose `gravity_strength: f32` (default 0.1) in the physics panel.
- [ ] If the built-in gravity is sufficient: document the finding here; no code change needed.

**Validation Tests**

- Headed: apply Preset B (spread, high repulsion), let simulate 10 seconds → all nodes remain
  visible within the canvas boundary.

---

### Phase 2: Advanced Layout Algorithms

#### 2.1 Greedy Label Occlusion Culling

At moderate graph sizes, label overlap makes the graph unreadable. Greedy occlusion culling
(research §14.2) is O(N log N) sort + O(N) placement pass — fast enough for 60FPS. No physics
dependency; this is a pure display-layer operation and the lowest-risk item in Phase 2.

**Algorithm**:

1. Sort visible nodes by importance (degree centrality; fallback: `last_visited` recency).
2. Iterate sorted nodes. Maintain an occupied screen-space set (grid buckets or small rect list).
3. If a node's label bounding box overlaps an already-drawn label, skip it.
4. Important nodes always show labels; clutter is strictly capped.

**Text width estimation**: use a character-count heuristic (`label.len() as f32 * avg_char_width`)
where `avg_char_width` is a fixed constant (~7px at standard label font size). Full egui font
measurement is overkill here; the heuristic is accurate enough for overlap detection.

**Tasks**

- [ ] Implement label occlusion pass in `render/mod.rs` or `GraphNodeShape::ui()` caller.
- [ ] Compute label bounding box from node screen position + character-count heuristic (see above).
- [ ] Maintain occupied regions (grid cells or rect list); skip occluded labels.
- [ ] Add `label_culling_enabled: bool` toggle (default true); expose in physics/display panel.

**Validation Tests**

- `test_label_culling_sort_by_degree` — two nodes with degrees 5 and 1; degree-5 node ranks first
  in the sorted pass.
- `test_label_culling_occlusion_detected` — two overlapping label rects → second entry is culled.
- `test_label_culling_disabled_shows_all` — when disabled, all labels rendered regardless of
  overlap.

---

#### 2.2 Degree-Dependent Repulsion (ForceAtlas2 Approximation)

Standard FR applies equal repulsion between all node pairs. Weighting repulsion by node degree
causes high-degree hub nodes to push neighbors further away — naturally spreading hub-and-spoke
topologies and separating communities without manual tuning (research §14.1, §14.3).

**Formula**: `Force = k * (deg(A) + 1) * (deg(B) + 1) / dist`

> **Validate before implementing**: the pairwise product `(deg(A) + 1) * (deg(B) + 1)` can produce
> extreme multipliers for high-degree hubs (e.g., deg=50 interacting with deg=5 → 306× amplification
> on every repulsion step). ForceAtlas2 uses per-node degree as a per-node mass coefficient, not a
> pairwise product. If the formula causes hub explosion in practice, fall back to a per-node weight
> `(deg(A) + 1)` applied only to node A's own repulsion term.

No AGPL dependency: this is an approximation within the existing FR engine.

**Architecture note**: egui_graphs' FR repulsion loop is internal — there is no exposed per-pair
force hook, and there is no `apply_barnes_hut_physics_step()` to target. Two viable approaches:

1. **Post-frame position correction**: after `get_layout_state`, compute a per-node degree push as
   a position delta and apply it to `egui_state` node locations.
2. **Supplemental repulsion pass**: implement a standalone loop that runs after `get_layout_state`
   each frame, applying degree-weighted displacement independently of FR.

Confirm the approach before writing the implementation.

**Tasks**

- [ ] Validate the pairwise formula against ForceAtlas2 reference; choose per-node vs pairwise
  weighting based on that review.
- [ ] Confirm implementation approach (post-frame correction or supplemental pass); document choice.
- [ ] Implement `degree_repulsion_pass(app)`: compute per-node degree, apply weighted position
  deltas to `egui_state` node locations after `get_layout_state`.
- [ ] Introduce `degree_repulsion_enabled: bool` flag on physics state (default true; opt-out in
  physics panel).
- [ ] Expose toggle in physics panel alongside other controls.

**Validation Tests**

- `test_degree_repulsion_hub_pushed_further` — star graph (hub + 5 leaves): hub-leaf equilibrium
  distance greater with degree repulsion enabled than disabled.
- `test_degree_repulsion_disabled_is_noop` — when disabled, node positions are unaffected by the
  pass.
- `test_degree_zero_node_uses_unit_factor` — isolated node (degree 0): repulsion factor = 1;
  no division-by-zero, no zero or negative weighting.

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

- [ ] **Prerequisite**: implement `apply_post_frame_layout_injection(app)` in `render/mod.rs`
  called after `get_layout_state` — Hook B does not currently exist.
- [ ] Parse registered domain from node URL (eTLD+1 or host) — add a small utility fn or reuse
  existing URL parsing.
- [ ] In `apply_post_frame_layout_injection()`: group nodes by domain; compute per-domain centroid
  from current positions.
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

### Phase 3: Magnetic Zones / Spatial Zoning

**Goal:** Users can define named spatial regions with soft attractor forces. Nodes tagged to a zone
are weakly pulled toward that zone's center each frame, producing regional organization without
hard walls. Research §13.1, §14.7.

#### 3.1 Soft Attractor Force

Each zone has a `centroid: Vec2` and a `force_strength: f32 ≈ 0.05`. Per-frame, for every node
whose `zone_id` matches the zone, apply a position delta toward the centroid. This is the
attractor-point model from §14.7 — nodes can leave their zone when strongly pulled by topology,
but they tend to cluster in the assigned region. Hard bounding boxes are explicitly rejected (jitter
at boundaries, §13.1.1).

The attractor force runs in the post-frame position-injection hook (Hook B from §2.3 prerequisite).

**Tasks**

- [ ] **Prerequisite**: Hook B (`apply_post_frame_layout_injection()`) must exist — see §2.3.
- [ ] Add `Zone { id: ZoneId, name: String, centroid: Vec2, force_strength: f32 }` to app state
  (not persisted to graph log — layout-only, like domain clustering forces).
- [ ] Add `zone_id: Option<ZoneId>` to `Node` as a layout annotation (persisted to snapshot only,
  not to the WAL log entries).
- [ ] In `apply_post_frame_layout_injection()`: for each zone, apply weak attraction from each
  member node toward the zone centroid.

**Validation Tests**

- `test_zone_attractor_pulls_toward_centroid` — node at (200, 200), centroid at (0, 0), strength
  0.05: after one application, node position is closer to centroid.
- `test_zone_force_noop_for_unassigned_nodes` — node with `zone_id = None` receives no zone force.
- `test_zone_force_does_not_persist_to_log` — after assigning a node to a zone and serializing
  graph, no `ZoneId` appears in `LogEntry` variants.

---

#### 3.2 Zone Creation UI

**Tasks**

- [ ] "Create zone from selection" context action: takes current `selected_nodes`, opens a name
  prompt, creates a `Zone` with centroid = mean position of selected nodes, assigns all selected
  nodes to it.
- [ ] Zone manager panel (collapsible, in physics/layout panel): lists zones with name, node count,
  force strength slider, and delete action.
- [ ] Visual: render a faint, rounded rectangle behind zone member nodes. The rect tracks the
  bounding box of member positions each frame. Use a non-interactable `egui::Area` layer below the
  graph canvas (same approach as overlay positioning, research §13.1.2).
- [ ] Dragging the zone rect background moves the centroid (and thus the attractor point).

**Validation Tests**

- `test_zone_centroid_is_mean_of_member_positions` — three nodes at known positions → zone centroid
  = arithmetic mean.
- `test_zone_delete_removes_member_assignments` — delete a zone → all previously-assigned nodes
  have `zone_id = None`.
- Headed: select nodes, create zone → zone rect appears; physics running → nodes drift toward zone.

---

#### 3.3 Rule-Based Auto-Maintenance

Zones can optionally carry a filter rule (domain, search facet) so new nodes that match are
auto-assigned. This makes the zone self-maintaining as the graph grows.

**Tasks**

- [ ] Add `Zone.rule: Option<ZoneRule>` where `ZoneRule` mirrors the facet syntax from UX polish
  §5.3 (e.g., `ZoneRule::Domain("github.com")`).
- [ ] In `apply_intent(AddNode)`: if `doi_enabled` or zone rules are active, check new node against
  each zone rule; assign matching zone.
- [ ] Expose rule input in the zone manager panel.

**Validation Tests**

- `test_zone_rule_auto_assigns_matching_node` — zone rule `domain:github.com`; add node with
  GitHub URL → `zone_id` is set to that zone.
- `test_zone_rule_does_not_assign_non_matching` — node with different domain → `zone_id` unchanged.

---

## Findings

### Architecture Continuity

Phase 1 uses the existing `physics.base.{is_running, last_avg_displacement, epsilon}` fields.
No new structs required.

Phase 2 operates on the far side of the `set_layout_state`/`get_layout_state` boundary:

- **Label culling (§2.1)**: pure display-layer; no physics state access needed. Can be implemented
  as a pre-render sort+cull pass in `render/mod.rs` or inside the node shape caller.
- **Degree repulsion (§2.2)**: egui_graphs' FR repulsion loop is internal with no exposed per-pair
  force hook. There is no BH physics path to target — it does not exist. The implementation must
  work as a post-frame position-correction pass applied to `egui_state` node locations after
  `get_layout_state`. This is viable but structurally different from "hooking into FR."
- **Domain clustering (§2.3)**: requires Hook B (`apply_post_frame_layout_injection()`), which also
  does not currently exist and must be created as a prerequisite.

Domain clustering via centroid attraction is preferred over phantom edges: cleaner separation,
avoids egui_graphs state leakage, matches §14.7 soft-force model.

### Relationship to Archived Layout Plan

The archived plan (`archive_docs/checkpoint_2026-02-19/2026-02-18_layout_strategy_plan.md`) marks
all 6 feature targets as [x] complete, but the corresponding implementations do not exist in the
codebase (no presets, no Hook B, no Sugiyama, no radial ego, no BH). Those [x] marks are incorrect
or refer to work that was later removed. The plan's FR preset parameter tables (c_repulse,
c_attract, k_scale values per preset) remain useful as a reference if presets are ever added, but
should not be treated as representing the current state.

### Research Cross-References

- Phase 1.1: §5.2 (auto-pause perception), §16.4 (convergence UX rule)
- Phase 1.2: §5.3 (reheat on structural change)
- Phase 1.3: §2.6 (mental map preservation, neighbor placement)
- Phase 2.1: §14.2 (greedy occlusion culling)
- Phase 2.2: §14.1 (FA2 degree repulsion formula), §14.3 (Forest of Fireflies topology)
- Phase 2.3: §14.4 (WebCola invisible constraints), §14.7 (attractor-point soft forces)

---

## Progress

### 2026-02-19 — Session 1

- Plan created from research report §2.6, §5, and §14.
- Physics micro-improvements consolidated here from `2026-02-19_graph_ux_polish_plan.md §Phase 1`
  (removed from that plan to eliminate redundancy per DOC_POLICY §2).
- Phases 1 and 2 have full task lists and unit test stubs.
- Implementation not started.

### 2026-02-19 — Session 2

- Corrected Context: archived layout plan [x] marks for presets, Hook B, Sugiyama, radial ego, and
  BH do not reflect actual code — none of those features exist. Context now describes the real
  foundation (plain FR with center gravity, parameter sliders).
- §1.3: added research-first framing; `from_node` availability through the navigation pipeline is
  the blocking question before any spawn-position code is written.
- Phase 2 reordered: label culling (§2.1) before degree repulsion (§2.2) — no dependencies,
  highest confidence, best immediate value.
- §2.2 degree repulsion: added formula validation caveat (pairwise product can produce 300×+
  amplification for high-degree hubs); clarified implementation must use a post-frame
  position-correction pass since egui_graphs' FR loop has no hook.
- §2.3 domain clustering: added Hook B as an explicit prerequisite task.
- Findings: Architecture Continuity rewritten to remove references to non-existent BH path;
  Archived Layout Plan note corrected.
