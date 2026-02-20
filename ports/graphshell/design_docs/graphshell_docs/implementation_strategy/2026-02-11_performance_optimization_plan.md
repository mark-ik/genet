# Performance Optimization Plan (2026-02-11)

**Architecture update (2026-02-20):**
This plan predates the tag system, diagnostic inspector, badge/orbit render cost, and
persistence hub Phase 5 (node dissolution). The following table maps old concepts and adds
new performance axes not present in the original:

| Old concept | Current equivalent / addition |
| --- | --- |
| "Profile" infrastructure | Diagnostic inspector plan (`DiagnosticGraph`, ring buffer, `cfg(feature = "diagnostics")`) |
| Render batch / culling | Also: `#archive` exclusion from graph view (badge plan) — free render reduction |
| Badge / orbit animation cost | **New axis** — per-node badge render (orbit animation, chip stack) adds cost per visible node |
| Physics spatial queries | Physics module (`physics/`) — `#pin` nodes skip force displacement; `#focus` adds center-attraction force (tag system coupling) |
| LOD zoom level | `MetadataFrame` at `Id::new("egui_graphs_metadata_")` post-frame — the zoom oracle |
| Graph size management | **New axis** — persistence hub Phase 5 node dissolution (cold `NodeActivityRecord` + DOI) reduces live graph size over sessions; complements visual LOD |
| Tunable thresholds | Settings architecture plan — FPS targets, cell sizes, and badge animation fps cap are settings, not hardcoded |

---

## Performance Optimization Plan

- Goal: 500 nodes at 45 FPS, 1000 nodes at 30+ FPS on reference hardware. No
  interaction stalls during pan/zoom or physics convergence.
- Scope: Rendering, physics, badge animation, and graph size management.
- Dependencies: Diagnostic inspector plan (profiling), physics module, badge plan
  (animation cost), persistence hub Phase 5 (dissolution), settings architecture plan
  (configurable thresholds).
- Phase 1: Profile and baseline
  - Use diagnostic inspector (`cfg(feature = "diagnostics")`) to capture frame timing
    and hot paths (compositor, layout, physics tick, egui_graphs render, badge render).
  - Establish baseline metrics at 100 / 500 / 1000 live nodes with and without
    badge animation active, with and without archived nodes present.
  - Identify which of the 8 gui.rs frame execution steps dominates frame time at scale
    (physics tick vs. egui_graphs render vs. badge orbit animation vs. webview sync).
- Phase 2: Render improvements
  - **`#archive` exclusion first** (badge plan): archived nodes already excluded from
    graph view — verify this is working before adding other culling. This is the highest
    ROI lever for large graphs where many nodes are archived.
  - Viewport culling: nodes outside the egui_graphs camera rect should be skipped.
    Read current zoom/pan from `MetadataFrame` (`Id::new("egui_graphs_metadata_")`)
    post-frame to determine viewport bounds; pre-filter the node set passed to
    egui_graphs. (Note: `MetadataFrame` is available only after `GraphView` renders —
    culling must gate the *next* frame's node set, not the current one.)
  - Badge animation budget: orbit animation (120 ms, badge plan) runs per visible node.
    Cap animation updates at 30 Hz when frame time is high; skip orbit expansion for
    off-screen nodes entirely.
  - Label simplification at distance: hide node labels when egui_graphs zoom < threshold
    (read from `MetadataFrame`); show only node color/shape.
- Phase 3: Physics tuning
  - Benchmark spatial queries in `physics/` at 500 and 1000 nodes. Adjust Barnes-Hut
    cell size and theta threshold for the target node counts.
  - `#pin` nodes: verify they are already excluded from force displacement updates
    (pinned = no velocity update). These are free wins at high pin counts.
  - `#focus` nodes: center-attraction force adds to the per-node force accumulation.
    Verify it does not create instability at large node counts (strong attraction on
    many `#focus` nodes can cause oscillation — may need dampening).
  - Early-exit for near-stable graphs: if max node velocity < epsilon across N
    consecutive ticks, pause physics until the graph is disturbed. This eliminates the
    physics tick cost entirely for settled graphs.
- Phase 4: Graph size management and LOD
  - **Persistence hub Phase 5 (node dissolution)**: `NodeActivityRecord` + DOI scoring
    dissolves cold nodes over sessions. This is the primary long-term graph size control
    — a 5000-node graph that dissolves to 300 live nodes is inherently faster than LOD
    applied to 5000 nodes. Cross-reference the persistence hub plan for implementation.
  - **Visual clustering** (optional, high complexity): when zoomed out below a threshold
    (from `MetadataFrame`), collapse nearby nodes into cluster nodes. Implement only if
    dissolution alone is insufficient for the target FPS at scale.
  - Settings integration: FPS targets, physics cell size, and badge animation fps cap
    should be configurable via the settings architecture plan (Advanced / Performance
    section). Expose as sliders or dropdowns; store in the settings store.

## Validation Tests

- 500 nodes: steady-state FPS ≥ 45 on reference hardware (measure via
  `egui::Context::frame_nr()` delta over 5 s).
- 1000 nodes: steady-state FPS ≥ 30.
- No interaction stall > 16 ms during pan/zoom at 500 nodes.
- Settled graph (max velocity < epsilon): physics tick time ≈ 0 (early-exit active).
- Graph with 200 archived nodes: render cost equivalent to a 0-archived-node graph of
  comparable live-node count (archived nodes not in render pass).
- Badge animation budget: orbit animation does not run for off-screen nodes; FPS
  stable when 100+ nodes have active badges.
- Settings change (FPS target / physics cell size): takes effect within one frame.

## Outputs

- Performance report: baseline metrics at 100/500/1000 nodes with profiling.
- Tuned physics cell size and theta defaults.
- Viewport culling and badge budget implementation.
- Settings entries for performance thresholds.

## Findings

- (See architecture update note at top of file.)
- The `MetadataFrame` post-frame constraint means viewport culling applies to the
  *following* frame, introducing one frame of lag on camera moves. This is acceptable
  — culling a node that just moved off-screen one frame late is invisible to the user.
- Node dissolution (Phase 5) and visual clustering (Phase 4) are alternatives, not
  complements. Implement dissolution first; only add clustering if the graph still
  degrades at scale despite dissolution.

## Progress

- 2026-02-11: Plan created.
- 2026-02-20: Aligned with diagnostic inspector (profiling infrastructure), tag system
  (`#archive` exclusion, `#pin` / `#focus` physics coupling), `MetadataFrame` zoom oracle,
  persistence hub Phase 5 (node dissolution as primary graph-size lever), badge render
  cost as new optimization axis, and settings architecture plan for configurable
  thresholds.
