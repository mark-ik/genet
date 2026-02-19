# Graph UI/UX Research Report (2026-02-18)

## Purpose

A comprehensive reference for force-directed graph interaction design, physics parameter tuning, layout quality, and UX principles — grounded in research literature and calibrated against graphshell's current codebase. Intended to guide all graph-facing development decisions.

---

## 1. Current State Inventory

### 1.1 Interactions Implemented

| Interaction | Gesture | Status |
| --- | --- | --- |
| Pan graph | Background drag | ✅ egui_graphs built-in |
| Zoom in/out | Scroll wheel | ✅ clamped `[0.1, 10.0]` via `Camera` |
| Select node | Click | ✅ single-select only |
| Open node in detail | Double-click | ✅ |
| Open node in split pane | Shift+Double-click | ✅ |
| Move (drag) node | Drag node | ✅ physics pauses during drag |
| Toggle physics | `T` | ✅ |
| Fit to screen | `C` | ✅ |
| Create node | `N` | ✅ |
| Delete selected | `Delete` | ✅ |
| Clear graph | `Ctrl+Shift+Delete` | ✅ |
| Graph search/filter | `Ctrl+F` | ✅ |
| Toggle graph/detail view | `Home` / `Esc` | ✅ |
| Physics panel | `P` | ✅ |
| Help panel | `F1` / `?` | ✅ |

### 1.2 Physics Parameters (egui_graphs `FruchtermanReingoldState`)

| Field | Graphshell Default | Role |
| --- | --- | --- |
| `c_repulse` | 0.75 | Repulsion coefficient between all node pairs |
| `c_attract` | 0.08 | Attraction coefficient along edges |
| `k_scale` | 0.65 | Multiplier on ideal inter-node distance |
| `max_step` | 10.0 | Max displacement per step (explosion guard) |
| `damping` | 0.92 | Velocity decay per step (friction) |
| `dt` | egui_graphs default | Time step delta |
| `epsilon` | egui_graphs default | Convergence threshold |
| `is_running` | true | Whether simulation is active |
| `last_avg_displacement` | computed | Convergence diagnostic |
| `step_count` | computed | Total steps taken |

### 1.3 Gaps vs. Research Baseline

Multi-select (`Ctrl+Click`), pin node UX (data model exists, no affordance), lasso/rubber-band selection, zoom-to-selected, hover tooltip, edge-type visual differentiation, zoom-adaptive labels, physics presets, auto-pause on convergence, neighbor highlight, keyboard zoom controls, edge hit targets.

---

## 2. Layout Quality: What "Good" Means

Research establishes five canonical aesthetic criteria for force-directed layouts, in descending priority for user task performance:

### 2.1 Minimize Edge Crossings

The single most impactful factor for readability. User studies (Purchase 2002, Yoghourdjian 2018) show even a 10–20% reduction in edge crossings measurably improves path-following and adjacency tasks. Crossings are caused by suboptimal initial positions; FR converges to local minima, so starting positions matter. For graphshell's incremental add-node model, placing new nodes near their graph neighbors (rather than center-plus-jitter) would reduce crossing risk.

### 2.2 Prevent Node Overlap

Overlapping nodes destroy the position-as-meaning encoding that makes spatial graphs useful. `c_repulse` is the primary control: it must be strong enough at close range to separate nodes, but not so strong that the graph explodes. The current default (0.75) is conservative; graphs with many close nodes may need higher values.

### 2.3 Uniform Edge Length

The original Fruchterman-Reingold (1991) optimization goal. Equal-length edges encode structural equivalence: nodes at similar graph distance appear at similar visual distance. Controlled by `k_scale` (sets the equilibrium distance `k`) and the `c_attract`/`c_repulse` ratio.

### 2.4 Maximize Angular Resolution

At each node, edges should spread as evenly as possible in angle. Poor angular resolution (all edges fanning in a narrow arc) makes local adjacency relationships hard to read. Degrades in dense graphs. The t-FDP algorithm (2023) improves angular resolution via a bounded short-range force modeled on Student's t-distribution; this is relevant for future algorithm upgrades.

### 2.5 Expose Symmetry

Graph automorphisms should appear visually symmetric. FR handles this well for planar and near-planar graphs (typical for web browsing history). Degrades for large irregular graphs.

### 2.6 Mental Map Preservation (Critical for Graphshell)

When the graph updates incrementally — new node added, new edge created — existing node positions should change minimally. Large positional jumps on each new node destroy the user's spatial memory of the graph they've been building. The current behavior (adding a node near the center with jitter) is not ideal: it adds new nodes far from their topological neighbors, which triggers large displacement during convergence.

**Recommendation:** When adding a node that is connected to an existing node, initialize its position near that neighbor, not at the graph center.

### 2.7 Neighborhood Preservation

Semantically related nodes (connected via edges, or sharing history) should appear close together after layout. Standard FR handles this for small graphs. For graphs with community structure (multiple browsing sessions, topic clusters), the `c_repulse`/`c_attract` ratio determines how well clusters separate.

---

## 3. Physics Parameters: Explained and Tuned

### 3.1 The Core Trade-off

The ratio `c_repulse / c_attract` is the most important tuning axis. It determines how spread vs. compact the graph appears.

- **High ratio** (e.g., 0.75 / 0.04): Nodes spread apart, clusters become visible, inter-cluster edges stretch long. Good for topology exploration.
- **Low ratio** (e.g., 0.4 / 0.12): Graph compresses, all nodes close together. Good for large graphs where you want everything on screen.

The absolute values matter less than the ratio; the `k_scale` parameter then sets the physical scale of the equilibrium distance.

### 3.2 Parameter Reference

| Parameter | Too Low | Too High | Recommended Range | Graphshell Default |
| --- | --- | --- | --- | --- |
| `c_repulse` | Node overlap, hairball | Nodes scatter to edges of screen | 0.4 – 2.0 | 0.75 |
| `c_attract` | Disconnected components drift apart | Connected nodes collapse into center | 0.03 – 0.20 | 0.08 |
| `k_scale` | Dense, cluttered layout | Sparse, nodes at screen edges | 0.4 – 1.5 | 0.65 |
| `max_step` | (No effect unless it becomes the binding constraint) | Explosive instability at startup | 5.0 – 20.0 | 10.0 |
| `damping` | Perpetual oscillation (nodes never settle) | Dead stop, poor convergence (no movement) | 0.85 – 0.97 | 0.92 |
| `dt` | Slow but stable | Explosive instability | 0.01 – 0.1 | default |
| `epsilon` | Runs forever | Stops too early, poor layout | 1e-4 – 1e-2 | default |

**`damping` is the most important convergence parameter.** Values above ~0.95 cause slow ring oscillations. Values below ~0.80 cause premature freeze. 0.92 is well-positioned.

**`max_step` prevents explosion** on graph load when many nodes start at similar positions. 10.0 is appropriate for a medium-density graph in screen-space units.

### 3.3 The FR Ideal Distance Formula

The "equilibrium distance" `k` is computed as:

```
k = k_scale * sqrt(display_area / N)
```

where `N` is the node count and `display_area` is the rendering canvas area. This means `k_scale` effectively controls the density relative to canvas size. For graphshell at typical window sizes (~1200×800 px):

- `k_scale = 0.65`: `k ≈ 100px` for N=50 — comfortable spacing
- `k_scale = 1.0`: `k ≈ 155px` for N=50 — spread out
- `k_scale = 0.4`: `k ≈ 62px` for N=50 — compact

---

## 4. Physics Presets

Research (Cambridge Intelligence, yWorks, empirical FR studies, Noack 2007) establishes that no single parameter set works across all graph topologies. For graphshell, five presets cover the practical range.

### Preset A: Compact (default — general browsing)

- **Use when:** 5–30 nodes, mixed topology, daily browsing graph
- **Favors:** Tight clusters, visible connections, fast convergence
- `c_repulse: 0.55, c_attract: 0.10, k_scale: 0.65, max_step: 10.0, damping: 0.92`
- *Current default.*
- Tuning note from current prototype feedback: repulsion is perceived as too strong. Lowering default repulsion and slightly raising attraction improves long-range coherence.

### Preset B: Spread (research/exploration)

- **Use when:** 20–80 nodes, clustered structure (topic research, multi-session)
- **Favors:** Community separation, readable cross-cluster edges
- `c_repulse: 1.5, c_attract: 0.05, k_scale: 1.0, max_step: 12.0, damping: 0.93`
- Higher repulsion separates communities. Lower attraction lets clusters drift.

### Preset C: Tight (large graph overview)

- **Use when:** 80+ nodes, want all on screen, accept some overlap
- **Favors:** All-nodes-visible, minimal pan required
- `c_repulse: 0.4, c_attract: 0.12, k_scale: 0.4, max_step: 8.0, damping: 0.90`
- Lower repulsion + higher attraction + smaller k_scale = compact.

### Preset D: Star (hub-and-spoke browsing)

- **Use when:** One dominant hub with many linked neighbors (docs, Wikipedia, news site)
- **Favors:** Hub at center, spokes radiating, minimal crossing
- `c_repulse: 1.0, c_attract: 0.05, k_scale: 0.8, max_step: 10.0, damping: 0.94`
- Low attraction prevents hub from being pulled toward the spoke average.

### Preset E: Frozen (manual layout)

- `is_running: false`
- All nodes remain exactly where placed. For users who have established a meaningful spatial map.
- Switching to Frozen preserves all positions. Switching away from Frozen resumes physics from current positions (no reset).

### Preset Switching Rules

- Switching presets while `is_running` should apply new parameters and continue running.
- Switching to a non-Frozen preset while `is_running == false` should start the simulation.
- Switching to Frozen should pause without resetting positions.
- Switching presets must never reset node positions.

---

## 5. Physics UX Controls

### 5.1 Two-Tier Approach

**Tier 1 — Presets (always visible, no expertise required):** A row of named preset buttons. One-click layout change for common graph types. Zero parameters to understand.

**Tier 2 — Fine-tune (collapsible, power users):** Sliders for individual parameters with human-readable labels:

| Technical Name | User Label | Slider Range |
| --- | --- | --- |
| `c_repulse` | "Node spread" | 0.0 – 3.0 |
| `c_attract` | "Edge pull" | 0.0 – 0.5 |
| `k_scale` | "Spacing multiplier" | 0.2 – 2.0 |
| `damping` | "Settling friction" | 0.5 – 1.0 |
| `max_step` | "Stability limit" | 1.0 – 50.0 |
| `dt` | "Simulation speed" | 0.001 – 0.1 (log) |
| `epsilon` | "Convergence threshold" | 1e-6 – 0.1 (log) |

The current physics panel already exists with raw names and correct slider ranges. Renaming the labels and adding presets above the sliders is the minimal change.

### 5.2 Convergence Indicator

`last_avg_displacement` is already read and displayed as a number. This is correct. Enhance with:

- A visual bar (full = active, empty = settled) in addition to the number
- An auto-pause trigger: when `last_avg_displacement < epsilon`, set `is_running = false` and display a "Layout settled" indicator
- Auto-pause prevents wasted CPU and makes physics feel responsive rather than perpetually running

### 5.3 Reheat on Structural Change

When adding a node or edge (from any source — keyboard `N`, URL navigation, edge creation command), locally reheat the simulation rather than globally resetting temperature. Concretely: after a structural change, increment `is_running = true` and let the simulation run until convergence. This preserves the user's spatial mental model of the unchanged subgraph.

Currently, adding a node does not automatically re-enable physics if it was paused. It should: adding a node that is physics-invisible is confusing.

---

## 6. Interaction Model: Complete Recommended Design

### 6.1 Gesture Disambiguation

| Gesture | Interpretation | Condition |
| --- | --- | --- |
| Click on node | Select (replace previous selection) | no modifier |
| `Ctrl+Click` on node | Toggle add/remove from selection | Ctrl held |
| `Shift+Click` on node | Range select (defer — needs ordered `SelectionState`) | Shift held |
| Click on background | Clear selection + begin pan after drag threshold | — |
| Double-click on node | Open node in detail view | no modifier |
| `Shift+Double-click` on node | Open node in split pane | Shift held |
| Right-click on node | Context menu / radial palette | — |
| Drag node | Move node; physics pauses during drag | initiated on node |
| Drag background | Pan viewport | initiated on background, past drag threshold |
| `Alt+Drag` background | Lasso multi-select (deferred) | Alt held |
| Scroll | Zoom in/out centered on cursor | — |
| Pinch (trackpad) | Zoom (via egui_graphs built-in) | — |

**Drag threshold:** 4–8px before committing to either pan or node-drag is standard practice (D3, Cytoscape.js defaults). This prevents accidental node moves on click.

**Background drag conflict with lasso:** Pan and lasso-select both begin with a background drag. The conventional resolution is a mode switch via a toolbar toggle or a distinct modifier key (`Alt+Drag` for lasso). Without a mode switch, background drag should always pan.

### 6.2 Pinning

The data model (`node.is_pinned`, `PinNode` log entry, `sync_graph_positions_from_layout` honor logic) already exists. Missing: UX affordance.

**Recommended:**

- `P` key with node(s) selected: toggle pin. (Conflicts with current `P` = physics panel; consider `Shift+P` for the panel, or remap to a different key.)
- Right-click > "Pin here" / "Unpin" in context menu
- Visual indicator: a small filled dot, ring, or pin icon on the node (currently no visual distinction between pinned and unpinned nodes)
- "Pin all" / "Unpin all" commands accessible from toolbar or physics panel

**Pinning workflow from research:**

Placing 2–3 pinned anchors before enabling physics significantly improves convergence quality by constraining the solution space. A practical workflow: explore graph manually → pin a few landmarks → re-run physics to settle unanchored nodes around them. This is a high-value interaction that graphshell is one implementation step away from supporting.

### 6.3 Multi-Select

`SelectionState` supports multi-select; no call site passes `multi_select: true` today. The wiring is 5 lines in [render/mod.rs](ports/graphshell/render/mod.rs) (read `ui.input(|i| i.modifiers.ctrl)` and pass through).

**Selection semantics:**

- `click`: replace selection with single node
- `Ctrl+click`: toggle node in/out of selection set
- `Ctrl+A`: select all (useful before "fully connect selection" or "pin all")
- Click on background: clear selection

**Group drag of selected nodes:** Once multi-select is wired, users will expect to drag a selection as a group. The current drag implementation only moves the single dragged node. Group drag means: detect if the dragged node is in the selection set; if so, apply the same delta to all selected nodes.

### 6.4 Keyboard Shortcuts: Full Recommended Set

Current shortcuts plus recommended additions:

| Key | Action | Status |
| --- | --- | --- |
| `T` | Toggle physics | ✅ |
| `C` | Fit graph to screen | ✅ |
| `N` | Create new node | ✅ |
| `Delete` | Remove selected nodes | ✅ |
| `Ctrl+Shift+Delete` | Clear graph | ✅ |
| `P` | Physics panel | ✅ (consider remap) |
| `Ctrl+F` | Graph search | ✅ |
| `Home` / `Esc` | Toggle graph/detail view | ✅ |
| `F1` / `?` | Help panel | ✅ |
| `Ctrl+Click` | Multi-select toggle | ❌ needs wiring |
| `Ctrl+A` | Select all nodes | ❌ |
| `Z` | Zoom to selected nodes | ❌ |
| `+` / `=` | Zoom in | ❌ |
| `-` | Zoom out | ❌ |
| `0` | Reset zoom to 1.0x | ❌ |
| `L` or `Shift+P` | Toggle pin on selected | ❌ |
| `R` | Reheat simulation (restart physics from current positions) | ❌ |
| `G` | "Group with focused" command (connect selected pair) | ❌ (planned in edge plan) |

---

## 7. Visual Feedback

### 7.1 Node State Color Encoding

Current:

| State | Color | RGB |
| --- | --- | --- |
| Cold node | Grey-blue | `(140, 140, 165)` |
| Active node | Cyan | `(100, 200, 255)` |
| Selected | Amber | `(255, 200, 100)` |
| Search match | Green | `(95, 220, 130)` |
| Active search match | Bright green | `(140, 255, 140)` |

Missing states:

| State | Suggested Encoding |
| --- | --- |
| Pinned | Small filled ring or dot overlay; or a slightly different border weight |
| Crashed (in graph view) | Red or orange tint; currently only visible in detail view tile |
| In current selection set (multi-select) | Distinct border or halo on all selected nodes, not just primary |

### 7.2 Edge Type Visual Differentiation

All three edge types (`Hyperlink`, `History`, `UserGrouped`) currently render identically. Research on multi-relational graph comprehension shows edge type differentiation significantly reduces time-to-interpretation. Recommended encoding:

| Edge Type | Visual | Rationale |
| --- | --- | --- |
| `Hyperlink` | Solid thin line, neutral color | Default/common; should be lowest visual weight |
| `History` | Dashed line | "Traversal" semantics; broken = traversed path |
| `UserGrouped` | Solid thicker line, amber (matches selected node color) | "User-intentional"; highest visual weight, user owns these |

This requires a custom `EdgeShape` implementation in egui_graphs. The trait exists; this is a non-trivial but high-value change.

### 7.3 Zoom-Adaptive Labels

At low zoom many nodes are visible but labels become unreadable noise. Three progressive levels based on `app.camera.current_zoom`:

| Zoom Range | Label Display |
| --- | --- |
| > 1.5 | Full title or full URL (current behavior) |
| 0.6 – 1.5 | Short form: domain only or first 20 chars of title |
| < 0.6 | No label; favicon only or colored dot |

The zoom level is available as `app.camera.current_zoom` and is already synced from egui_graphs metadata. Label rendering happens in the custom `GraphNodeShape` — the threshold check is a few lines in that rendering path.

### 7.4 Hover Tooltip

Currently, long URLs are truncated in the node label. There is no mechanism to see the full URL without opening the node.

**Recommended:** On hover, show an egui tooltip with:
- Full URL
- Title (if different from URL)
- Last visited timestamp
- Node lifecycle state

This is a standard `response.on_hover_text(...)` pattern in egui, attached to the node response in `GraphNodeShape`.

### 7.5 Simulation State Feedback

Users cannot tell when physics has "settled" short of watching node movement slow down. Add:

- A status indicator in the graph info overlay: "Physics: Running" / "Physics: Settling" / "Physics: Settled" (using `last_avg_displacement` relative to `epsilon`)
- Optionally, a subtle jitter animation overlay that fades out as displacement decreases
- The current info overlay already shows "Physics: Running" / "Physics: Paused" — extend to include "Settled" state after auto-pause

### 7.6 Neighbor Highlighting on Hover

When hovering a node, dim all non-adjacent nodes and edges. This reveals the local neighborhood without requiring selection or filtering. Implemented by modifying node opacity in `apply_search_node_visuals` (or a parallel hover-visual function) based on graph adjacency from `app.graph.out_neighbors()` / `in_neighbors()`.

---

## 8. Zoom and Navigation

### 8.1 Zoom Levels

| Level | Zoom Value | Typical Use |
| --- | --- | --- |
| Full overview | 0.3 – 0.5 | All nodes visible, read cluster structure |
| Normal | 1.0 | Daily use |
| Detail | 2.0 – 3.0 | Read full URLs, see edge labels |

Current range: `[0.1, 10.0]`. The upper bound (10.0) is excessive for typical use. `5.0` is a practical maximum; labels become pixel-perfect below that. The lower bound (0.1) is about right for very large graphs.

**Keyboard zoom:**
- `+` / `=`: zoom in by 25%
- `-`: zoom out by 25%
- `0`: reset to 1.0x
- `Z` with selection: fit viewport to bounding box of selected nodes (most-used navigation shortcut in Gephi, Cytoscape, yEd)

### 8.2 Zoom-to-Selected

When `Z` is pressed with nodes selected, compute the axis-aligned bounding box of selected node positions and set zoom + pan to fit that box with a 20% padding margin. This requires reading node positions from `app.graph` and setting egui_graphs camera state via `MetadataFrame`.

### 8.3 Canvas Boundary / Gravity

Standard FR runs without boundary constraints; nodes can drift off-screen with low graph density or high repulsion. A weak centering force (gravity) pulls nodes toward the canvas center. The Fruchterman-Reingold implementation in egui_graphs may not include gravity — worth checking.

If gravity is not implemented: after simulation converges, auto-run "fit to screen" to re-center the result. This is a partial substitute.

---

## 9. Search and Filter

The current search implementation filters the graph view to matching nodes only (hides non-matching nodes).

### 9.1 Highlight Mode vs. Filter Mode

Research shows users need both:

- **Highlight mode:** Non-matching nodes remain visible but dimmed; context is preserved. Good for "where does this node fit in the broader graph?"
- **Filter mode:** Non-matching nodes are hidden; only matches shown. Good for "I only care about these."

Recommended: A toggle between "highlight" and "filter" search modes. Default to highlight (less destructive; users can always see the full graph).

### 9.2 Neighbor Filter

A high-value operation for web history graphs: "show me this node and everything connected to it." Implemented as a filter that keeps the selected node plus its N-hop neighborhood. N=1 is the most useful (direct neighbors only); N=2 is the secondary option.

### 9.3 Edge-Type Filter

Filter to show only nodes connected by a specific edge type (e.g., only `UserGrouped` connections). Useful for reviewing explicitly-grouped browsing sessions.

---

## 10. Anti-Patterns (Do Not Do)

**Do not autorun physics indefinitely.** A perpetually-running simulation prevents users from creating stable spatial layouts. Auto-pause on convergence. Providing a "reheat" button for intentional re-layout preserves user control.

**Do not create edges from ambiguous gestures.** The deterministic trigger matrix in the edge operations plan is the right model. Any gesture users encounter naturally (focus change, tab switch, navigation) must be a hard no-trigger path. Edge creation must always require explicit intent.

**Do not hide node identity under zoom.** When zoomed out, show at minimum a colored dot or favicon. Blank rectangles lose the graph's structure entirely.

**Do not let node drag fight physics while dragging.** Physics pause during drag is already implemented — do not remove it. Dragging a node while physics is running causes the node to "fight back" toward equilibrium, which is disorienting and feels broken.

**Do not expose raw physics parameters as the primary UI.** Raw slider values (`c_repulse`, `c_attract`) are meaningless to users. Named presets first, sliders second (collapsible).

**Do not place new nodes at the graph center.** New nodes placed at center are pushed outward by repulsion, which causes all other nodes to shift. Place new nodes near their topological neighbors (if connected) or at a low-density area of the canvas.

**Do not use the same gesture for pan and lasso.** Background drag must consistently mean pan. Lasso requires either a mode switch or a distinct modifier.

**Do not run O(N²) FR for large graphs without a step budget.** Standard FR is quadratic per iteration. For graphshell's expected scale (50–150 nodes), this is fine at ~60fps. At >200 nodes, frame rate will degrade. A step budget (run N iterations per frame, not until convergence) or Barnes-Hut approximation (O(N log N)) would be needed if the graph grows larger.

---

## 11. Implementation Priority Order

Ranked by research-backed value relative to implementation effort:

| Priority | Feature | Effort | Value |
| --- | --- | --- | --- |
| 1 | `Ctrl+Click` multi-select wiring | ~5 lines | High — unblocks all pair commands |
| 2 | Pin node UX (visual indicator + keyboard toggle) | Small | High — data model ready; users need the affordance |
| 3 | Physics presets (preset buttons above panel sliders) | Small–Medium | High — most impactful UX improvement to physics |
| 4 | Auto-pause on convergence (watch `last_avg_displacement`) | Small | High — prevents wasted CPU, improves UX feel |
| 5 | Reheat on structural change (new node/edge enables physics) | Small | Medium — discoverability of layout |
| 6 | Hover tooltip (full URL/title on mouse hover) | Small | Medium — label truncation is a real friction point |
| 7 | Keyboard zoom (`+`/`-`/`0`) | Small | Medium — standard navigation control |
| 8 | New node placement near topological neighbors | Small | Medium — improves mental map preservation |
| 9 | Zoom to selected (`Z` key) | Medium | Medium — standard graph nav shortcut |
| 10 | Edge type visual differentiation (solid/dashed/thick) | Medium | Medium — requires custom `EdgeShape` |
| 11 | Zoom-adaptive labels (hide/shorten labels at low zoom) | Small–Medium | Medium — readability at small graph sizes |
| 12 | Simulation convergence status indicator | Small | Low–Medium — polish |
| 13 | Neighbor highlight on hover | Medium | Medium — exploration aid |
| 14 | Highlight vs. filter search mode toggle | Small | Medium — less destructive search |
| 15 | Lasso multi-select (`Alt+Drag`) | Large | Medium — needs custom input handling on top of egui_graphs |
| 16 | Group drag of multi-selected nodes | Medium | Medium — follows multi-select wiring |
| 17 | Edge hit target widening | Small | Low–Medium — polish |
| 18 | Crashed node indicator in graph view | Small | Low — already visible in detail view |

---

## 12. Research Sources

- [Fruchterman & Reingold (1991): Graph Drawing by Force-Directed Placement](https://www.researchgate.net/publication/328078452_FruchtermanReingold_1991_Graph_Drawing_by_Force-Directed_Placement) — foundational FR algorithm; ideal distance formula, temperature cooling schedule
- [Force-Directed Graph Layouts Revisited: t-FDP (arXiv 2303.03964)](https://arxiv.org/abs/2303.03964) — t-distribution bounded force; better neighborhood preservation; 1–2 orders of magnitude faster on GPU than standard methods
- [User-Guided Force-Directed Graph Layout (arXiv 2506.15860)](https://arxiv.org/html/2506.15860v1) — sketch-based user constraints; freehand shapes → layout directives; fCoSE algorithm with constraint support
- [Force-Directed Drawing Algorithms Survey, Kobourov (Brown University)](https://cs.brown.edu/people/rtamassi/gdhandbook/chapters/force-directed.pdf) — aesthetic criteria hierarchy, algorithm comparison, parameter sensitivity
- [An Improved Force-Directed Layout Based on Aesthetic Criteria (ResearchGate)](https://www.researchgate.net/publication/273311262_An_improved_force-directed_graph_layout_algorithm_based_on_aesthetic_criteria) — edge-edge repulsion for angular resolution
- [Graph Visualization UX: Cambridge Intelligence](https://cambridge-intelligence.com/graph-visualization-ux-how-to-avoid-wrecking-your-graph-visualization/) — progressive disclosure, cognitive load, filter vs. highlight modes, label management
- [Automatic Graph Layouts: Cambridge Intelligence](https://cambridge-intelligence.com/automatic-graph-layouts/) — layout type selection guide; preset recommendations for sparse/dense/clustered graphs
- [Force-Directed Graph Layouts: yWorks](https://www.yworks.com/pages/force-directed-graph-layout) — enterprise layout parameter recommendations
- [Persistent Homology Guided Force-Directed Graph Layouts (ar5iv)](https://ar5iv.labs.arxiv.org/html/1712.05548) — topology-aware layout to reveal community structure
- [egui_graphs FruchtermanReingoldState API](https://docs.rs/egui_graphs/latest/egui_graphs/struct.FruchtermanReingoldState.html) — all fields, types, and current defaults
- [Interactive Force-Directed Graphs with D3 (NinjaConcept/Medium)](https://medium.com/ninjaconcept/interactive-dynamic-force-directed-graphs-with-d3-da720c6d7811) — drag threshold, pin semantics, zoom interaction patterns
- [Aesthetic-Driven Navigation for Node-Link Diagrams in VR (ACM SUI 2023)](https://dl.acm.org/doi/10.1145/3607822.3614537) — navigation quality and spatial memory in graph interfaces
- Noack (2007), "Energy Models for Graph Clustering," JGAA — LinLog model; repulsion exponent sensitivity analysis
- Purchase et al. (2002), "Metrics for Graph Drawing Aesthetics," Journal of Visual Languages — user study basis for aesthetic criteria priority ordering
- Yoghourdjian et al. (2018), "Exploring the Limits of Complexity: A Survey of Studies on Graph Visualisation," Visual Informatics — meta-analysis of user study results
- D3.js force simulation defaults (`forceManyBody`, `forceLink`, drag threshold) — industry-standard interaction baseline
- Cytoscape.js layout documentation — gesture conventions for biological graph visualization tools

---

## 13. Spatial Organization, DOI, and Advanced Search

This section covers rule-based spatial zoning (magnetic zones, Group-in-a-Box), Degree of Interest filtering, semantic fisheye focus+context, and faceted search UI paradigms.

### 13.1 Zoning: Rule-Based Spatial Layout

Users want to organize their mental map: "Work is on the left, news is on the right." In a force-directed graph, nodes drift. We need constraints.

#### 13.1.1 Technique: Magnetic Fields vs. Hard Boxes
*   **Hard Bounding Boxes**: "Nodes must stay within $(x1, y1)$ to $(x2, y2)$."
    *   **Pros**: Absolute guarantee of separation.
    *   **Cons**: Unstable at boundaries. Nodes bounce off "walls", adding jitter.
    *   **Verdict**: Avoid for interactive graphs.
*   **Soft Magnetic Arrays (Recommended)**:
    *   **Concept**: Define a "Zone Center" (e.g., $(-500, 0)$ for "Work").
    *   **Implementation**: A custom force in `fdg-sim` that applies a weak, long-range linear attraction to the Zone Center for all matching nodes.
    *   **Result**: Nodes *can* leave the zone if pulled by strong topology (e.g., a work link to a news article), but they *tend* to cluster in their assigned region. This feels organic and stable.

#### 13.1.2 Group-in-a-Box (Visual Container)
*   **Visuals**: Render a faint, rounded rectangle behind the nodes of a zone.
*   **Interaction**: Dragging the *box* moves the Zone Center (and thus the magnetic attractor).
*   **Creation**:
    *   "Select nodes -> Right Click -> Create Zone"
    *   "Create Zone from Search (e.g., `domain:wikipedia.org`) -> Auto-maintain"
    *   **Auto-Maintenance**: As new nodes appear, if they match the rule, they immediately feel the magnetic force of that zone.

### 13.2 Advanced Filtering: Degree-of-Interest (DOI)

Simple string matching is binary (show/hide). Browsing history is continuous. We need a continuous "relevance" function to drive size and visibility.

#### 13.2.1 The DOI Function
$DOI(n) = \alpha \cdot Recency(n) + \beta \cdot Frequency(n) + \gamma \cdot ExplicitInterest(n) - \delta \cdot DistanceFromFocus(n)$

*   **Recency**: Decay function (`1 / (1 + time_since_visit)`).
*   **Frequency**: Basic visit count (`log(1 + visits)`).
*   **Explicit Interest**: Pinned = 1.0, Bookmarked = 0.8, etc.
*   **Distance**: Graph hop distance from the currently selected/hovered node.

#### 13.2.2 Visualizing DOI (Focus + Context)
Instead of hiding low-DOI nodes completely (which destroys context), use **Semantic Fisheye**:
*   **High DOI**: Large node, full label, bright color.
*   **Medium DOI**: Normal node, short label.
*   **Low DOI**: Dot only, muted color, no label.
*   **Zero DOI (Filtered)**: Hidden or very faint ghost.

**Implementation**: Calculate DOI in a background thread (throttled). Update styling metadata in `egui_graphs`.

### 13.3 Search UI: Faceted & Natural Language

*   **Global Search Bar (`Ctrl+F` / generic input)**:
    *   Accepts:
        *   Simple text ("rust compiler") -> Matches title/url.
        *   **Facets**: `domain:github.com`, `date:>2026-01-01`, `is:pinned`.
        *   **Natural Language**: "Pages from yesterday about servo".
*   **Interaction (Filtering vs. Selection)**:
    *   **"Select"**: Adds matching nodes to selection (allows bulk moves/ops).
    *   **"Filter"**: Sets the DOI threshold (hides non-matches).
    *   **"Zone"**: Converts the search query into a permanent Spatial Zone.

### 13.4 Implementation Strategy

1.  **Magnetic Force**: Implement `MagneticForce` struct in `fdg-sim`. It holds a `Map<Rule, Point>`. In `apply_force`, check if node matches rule -> pull toward point.
2.  **Zone Renderer**: Custom `egui` layer *below* the graph to draw zone backgrounds.
3.  **DOI Calculator**: A system in `app.rs` that runs every 100ms, updates node `color`/`radius` based on the DOI formula.
4.  **Search Parser**: A simple parser for `key:value` syntax to drive the DOI function.


---

## 14. Advanced Layout Algorithms and Rendering

### 14.1 ForceAtlas2-Style Degree-Dependent Repulsion

Standard FR applies equal repulsion between all node pairs. ForceAtlas2 weights repulsion by node degree, causing high-degree hub nodes to push neighbors further away — naturally spreading hub-and-spoke topologies and separating communities without manual tuning.

**Force model:**

- **Attraction**: Linear ($d$), like FR.
- **Repulsion**: Proportional to $1/d$ like FR, but weighted by node degree. Hubs repel neighbors more strongly, pushing "spokes" further out from the cluttered center.
- **Gravity**: A central gravity force prevents disconnected components from drifting off-screen.
- **LinLog Mode**: A variant that uses logarithmic attraction ($\ln(d+1)$). This creates tighter clusters — makes browsing sessions visually distinct from each other.
- **Trade-off**: Marginally more complex to tune (`gravity`, `scaling`, `edge_weight_influence`). Computational cost comparable to FR ($O(N^2)$ or $O(N \log N)$).

**Implementation Hint**: To approximate ForceAtlas2 behavior in `egui_graphs` without rewriting the engine, multiply the repulsive force by `(degree(node_a) + 1) * (degree(node_b) + 1)`. This single tweak effectively pushes hubs apart. Full formula: `Force = k * (deg(A)+1) * (deg(B)+1) / dist`.

### 14.2 Label Placement: Solving Overlap

Overlapping labels make dense graphs unreadable. Two strategies are viable for 60FPS:

1.  **Force-Biased Label Positioning (Simulated)**:
    *   Treat labels as "ghost nodes" connected to their parent node by a stiff, short spring.
    *   Labels repel *only* other labels (not graph nodes).
    *   **Rust Impl**: Run a lightweight secondary physics pass (5 iterations per frame) just for label rectangles. The computational cost is low ($M^2$ where $M$ is visible labels).
    *   *Result*: Labels "slide" around their parent node to find empty space.

2.  **Greedy Occlusion Culling (Cheaper)**:
    *   Sort labels by node importance (e.g., degree centrality or "last visited").
    *   Iterate through the sorted list. Maintain a "mask" (quadtree or grid) of occupied screen space.
    *   If a label overlaps an already-drawn label, discard it (or fade it out).
    *   *Result*: Important nodes always have labels; clutter is strictly capped.

### 14.3 Browsing History Topology: "Forest of Fireflies"

Web history is neither a pure Tree nor a random Small-World network. It effectively models as a **"Forest of Fireflies"**:
*   **Linear Chains**: Depth-first navigation (Wikipedia rabbit holes).
*   **Starbursts**: Hub-and-spoke (Google Search results, Reddit threads).
*   **Cycles**: Rare, usually "Back" button (which we often visualize as a tree branch anyway) or circular navigation menus.
*   **Disconnected Components**: Very common (opening a new tab from a bookmark).

**Tuning Implication**:
*   Because linear chains are common, `c_attract` must be high enough to keep sequences readable.
*   Because "starbursts" (hubs) are density drivers, `c_repulse` must be degree-dependent (see §14.1) to open up the fans.
*   The layout *must* handle disconnected subgraphs gracefully (typically via a weak central gravity to keep them on-screen).

### 14.4 Constraint-Based Layouts (WebCola approach)

To visually group nodes by domain (e.g., "envelop all `wikipedia.org` nodes in a box"), widely used in WebCola:

*   **Technique**: "Layout constraints" are applied *after* the force integration step but *before* the position update.
*   **Algorithm**:
    1.  Compute standard FR forces.
    2.  **Projection**: For each group, calculate the bounding box of its nodes.
    3.  **Constraint force**: If a node is outside its group's "target box" (or if the box is too big/small), add a strong corrective force vector towards the group centroid or boundary.
    4.  Alternatively, introduce "invisible structural edges" with high stiffness between all nodes of the same domain.

**Rust/egui Feasibility**: "Invisible edges" is the easiest implementation path. Just add edges between temporal neighbors of the same domain with `stroke: None` and `strength: 2.0`. This clusters domains without complex constraint solvers.

### 14.5 Edge Bundling in Real-Time

**Verdict**: Full Force-Directed Edge Bundling (FDEB) is **too expensive** for real-time 60FPS interaction on the CPU for >50 edges. It requires subdividing edges into dummy nodes and running physics on them ($E \times segments$).

**Viable Alternatives**:
1.  **Splines (Cubic Bezier)**: Instead of straight lines, use curved lines.
    *   Control point 1: Node A + vector roughly towards graph center.
    *   Control point 2: Node B + vector roughly towards graph center.
    *   *Result*: Simple "fish-eye" curving that reduces visual clutter near the center, mimicking bundling for practically zero cost.
2.  **Step-Bundling**: Run FDEB *once* when the graph settles (physics pauses). Do not calculate during interaction. Fade from straight to bundled lines when idle.

### 14.6 Recommendation Summary

1.  **Immediate Win**: Switch Repulsion to **Degree-Dependent Repulsion** (ForceAtlas2 style). `Force = k * (deg(A)+1) * (deg(B)+1) / dist`. This fixes the "hub crush" problem.
2.  **Labels**: Implement **Greedy Occlusion Culling**. It’s O(N log N) (sorting) + O(N) (placing) and robust.
3.  **Grouping**: Add **Invisible Layout Constraints** between same-domain nodes to encourage clustering (runtime/layout-only; do not persist as semantic graph edges).

---

### 14.7 Zoning: Physics Implementation Detail

The attractor-point approach from the earlier zoning section can be implemented with either hard or soft forces:

*   **Bounded Container Nodes (Hard Constraints)**: A transparent "box" node that strictly contains its children. If a node's position $(x, y)$ is outside the box bounds, apply a strong restorative force $\vec{F} = k \cdot (\vec{p}_{clamped} - \vec{p})$. "Hard" walls cause jitter when repulsion pushes nodes against the boundary — prefer soft forces for interactive graphs.

*   **Attractor Points (Soft Forces, Recommended)**: Define a centroid point per group (e.g., `Wikipedia: (500, -500)`). Apply a weak, long-range attraction force from every group node toward its centroid. Soft forces allow the graph to "breathe" while maintaining regional order.

```rust
// Pseudocode for fdg-sim custom force
fn update(&self, nodes: &mut [Node]) {
    for node in nodes {
        if let Some(target) = self.get_target_for_domain(node.data.domain) {
            node.velocity += self.force_fn(target - node.position); // e.g., F = 0.05 * dist
        }
    }
}
```

### 14.8 DOI Visualization Strategies

The DOI score from the earlier DOI section drives rendering properties continuously:

*   **Size**: Scale node radius proportionally to DOI.
*   **Opacity**: Fade out nodes below a DOI threshold; avoid hiding entirely — keep as a ghost for context.
*   **Level of Detail (LOD)**: Full label + favicon for high DOI; domain abbreviation for medium; dot only for low.

**Implementation**: Calculate DOI in a background thread (throttled to every ~100ms). Cache results in `MetadataFrame` to avoid per-frame recomputation. Update `node.color` and `node.radius` via `egui_graphs` styling metadata.

### 14.9 Semantic Fisheye (Focus + Context)

Instead of geometric distortion (which warps text), use **Semantic Fisheye**: scale node *rendering size* based on distance from cursor/selection without changing $(x, y)$ positions. The graph topology remains undistorted; only visual emphasis shifts.

**Implementation:**

1. Calculate `dist = |mouse_pos - node_pos|` for each visible node.
2. Compute `scale = max(1.0, 3.0 * (1.0 - dist / radius))` where `radius` is the influence radius.
3. Draw node at `base_size * scale`.
4. Draw high-scale nodes on top (z-order by scale) to prevent occlusion of focused nodes.

*Result*: The focused neighborhood is readable; the periphery provides context without the disorientation of hyperbolic geometry.

### 14.10 Faceted Search: UI Panel Layout

*   **Floating Palette (Recommended)**: A small "Filter" chip in a corner expands into a panel. Keeps the graph maximally visible. Components:
    *   **Time Slider**: Range selector for "Visits between [Date A] and [Date B]."
    *   **Domain Facets**: Top domains with checkboxes (e.g., "[x] github.com (15)", "[ ] rust-lang.org (8)").
    *   **Type Toggles**: [x] Pages, [ ] Images, [ ] Downloads.
*   **Sidebar**: Too rigid — consumes ~300px even when not filtering. Avoid.

**egui hints**: Use `egui::Window` with a transparent frame and `collapsible(true)`. Cache DOI values in `MetadataFrame` to avoid recomputing every frame.

---

## 15. Unaddressed Feature Inventory & Historic Concepts

This section catalogs high-value UX concepts found in project design documents (`archive_docs`) that have not yet been implemented or fully specified in current plans.

### 15.1 Temporal Navigation ("Time Travel")
*   **Source**: `verse_docs/GRAPHSHELL_P2P_COLLABORATION.md`
*   **Concept**: Since the architecture uses a deterministic `Command` log for P2P sync, the UI can expose a **History Slider**.
*   **UX**: Dragging the slider scrubs the graph state backward in time, allowing users to recover deleted subgraphs or understand how a complex research session evolved.
*   **Visuals**: Past states rendered with a desaturated "ghost" effect; current state in full color.

### 15.2 Collaboration Presence ("Ghost Cursors")
*   **Source**: `verse_docs/GRAPHSHELL_P2P_COLLABORATION.md`
*   **Concept**: In a P2P session, remote users need representation.
*   **UX**:
    *   **Remote Cursors**: Labeled pointers showing where others are looking/hovering.
    *   **Remote Selection**: A distinct border color (e.g., Purple for User B) around nodes they have selected.
    *   **"Follow Mode"**: Click a user's avatar to lock your camera to their viewport.

### 15.3 Integrated Browser Panels
*   **Source**: `GRAPHSHELL_AS_BROWSER.md`
*   **Downloads**: Not just a list, but a graph-integrated sidebar. Clicking a download in the list pans the graph to the **Source Node** from which it originated.
*   **Bookmarks**:
    *   **No Folder Tree**: Bookmarks are implemented as **Metadata Tags** on nodes (e.g., `#starred`, `#reading-list`).
    *   **Visuals**: Bookmarked nodes get a permanent visual indicator (star icon or heavy border) in the graph view, separate from the "Selected" state.

### 15.4 The Unified Omnibar
*   **Source**: `GRAPHSHELL_AS_BROWSER.md`
*   **Concept**: A single text input that handles both URL navigation and Graph Search.
*   **Heuristics**:
    *   Input starts with `http`/`www` or contains `.` -> **Navigate** current node (or create new).
    *   Input matches existing node titles (fuzzy match via `nucleo`) -> **Pan & Select** existing node.
    *   Input starts with `?` -> **Web Search** (Google/DDG) in new node.

### 15.5 Node Visual Hierarchy (Thumbnails)
*   **Source**: `archive_docs/.../THUMBNAILS_AND_FAVICONS_PLAN.md`
*   **Concept**: Nodes should not always look the same.
*   **Logic**:
    1.  **Thumbnail**: If `thumbnail_texture` exists and zoom > 1.5, render screen capture.
    2.  **Favicon**: If no thumbnail but `favicon_texture` exists, render large icon in center.
    3.  **Color Fallback**: If neither, render solid color circle based on domain hash.
*   **State Borders**:
    *   **Active (Has Webview)**: Bright Cyan border.
    *   **Cold (Tab Closed)**: Dim/Grey border.
    *   **Pinned**: Dashed white border.

---

## 16. Cross-Implementation Reference Data

Calibration data from D3-force and ForceAtlas2 implementations, included as a reference baseline for tuning egui_graphs parameters and for future algorithm decisions. D3-force uses different units from egui_graphs FR, but the behavioral ratios and convergence constants transfer directly.

### 16.1 D3-force Convergence Constants

| Parameter | D3-force Default | Behavioral Meaning |
| --- | --- | --- |
| `alpha` (initial heat) | 1.0 | Starting temperature |
| `alphaDecay` | 0.0228 per tick | Halves in ~30 ticks; simulation "dead" in ~300 ticks |
| `alphaMin` (convergence) | 0.001 | Stop threshold — below this, simulation ends |
| `velocityDecay` (friction) | 0.4 | Below 0.3 → instability; above 0.6 → sluggish drag response |
| `charge` (repulsion) | -30 per node | Scale as `-30 * sqrt(N)` for node-count-adaptive strength |
| `linkDistance` | 30px dense / 80–120px sparse | Equilibrium edge length in screen pixels |
| `collisionRadius` | node_radius + 2px | Minimum gap between node bounding circles |
| Barnes-Hut theta | 0.9 | Quality/speed tradeoff; safe for ≤500 nodes |

The `-30 * sqrt(N)` adaptive repulsion formula is the most actionable cross-reference: as graph size grows, repulsion must scale with `sqrt(N)` to maintain visual density (individual nodes get less screen space but the total repulsive force grows proportionally).

**egui_graphs translation:** `c_repulse` plays the role of `charge` but in a normalized coordinate system. The adaptive scaling principle still applies: consider adjusting `c_repulse` or `k_scale` as a function of `graph.node_count()` for large graphs.

### 16.2 ForceAtlas2 Parameter Defaults

ForceAtlas2 (Gephi) uses different parameter names but these defaults are useful as reference for the degree-dependent repulsion described in §14.1:

| Parameter | Default | Effect |
| --- | --- | --- |
| `Scaling` | 2.0 | Global repulsion multiplier (increase for sparser layouts) |
| `Gravity` | 1.0 | Central attraction; prevents disconnected component drift |
| `Speed` (jitter tolerance) | 1.0 | Lower = more stable, slower convergence |
| `LinLog mode` | off | Switch to logarithmic attraction for community-revealing layouts |
| `Prevent Overlap` | off | Node-radius-aware repulsion; enable *after* initial convergence |
| `Dissuade Hubs` | off | Penalizes high-degree hub centrality; evens topological spread |
| Barnes-Hut theta | 1.2 | More approximate than D3 default; trades quality for speed |

**Recommended workflow (from Gephi):** Run at default until layout stabilizes visually → enable Prevent Overlap for final cleanup → stop manually. The user judges convergence; do not rely solely on energy threshold. This contrasts with the §5.2 auto-pause recommendation — for power users, expose both: auto-pause as the default, with a manual "keep running" toggle.

### 16.3 Layout Quality Metrics (Formulas)

These formulas enable objective measurement of layout quality, useful for automated tests or future preset calibration:

**Normalized Stress:**

```text
stress = sum((d_graph(i,j) - d_layout(i,j))^2 * w_ij) / sum(d_graph(i,j)^2 * w_ij)
```

where `d_graph` = shortest-path graph distance, `d_layout` = Euclidean layout distance, `w_ij = 1/d_graph^2`. Target: < 0.1 is "excellent." Measures how faithfully graph topology maps to spatial position.

**Neighborhood Preservation (k-NP):**

```text
k-NP = mean over all nodes: |k-nearest-graph-neighbors ∩ k-nearest-layout-neighbors| / k
```

Standard: k = 5. Target: > 0.7. Measures whether topologically close nodes appear spatially close. This is the metric §2.7 describes qualitatively.

**Edge Length Uniformity:**

```text
uniformity = 1 - (std_dev(edge_lengths) / mean(edge_lengths))
```

Target: > 0.7 (coefficient of variation < 0.3). FR directly optimizes for this; the `c_attract`/`c_repulse` ratio is the primary control.

**Angular Resolution:**

```text
angular_resolution = min over all nodes: (min angle between any two incident edges at that node)
```

Target: > 30°. Below 15° → users report "unreadable" local adjacency at that node. Degrades at high-degree hubs.

### 16.4 Convergence UX Rule

**Research finding (Lucas 2016):** Users perceive a physics simulation as "broken" if it does not visually stabilize within 2–3 seconds. Layouts that continue oscillating past that threshold cause users to abandon the tool or assume a failure has occurred.

Concrete implication for graphshell:

- With `damping = 0.92` and `epsilon` at egui_graphs default, verify that typical graph sizes (10–50 nodes) converge within ~180 frames (~3s at 60 FPS).
- If convergence is slower, the primary knobs are: increase `damping` (toward 0.95), reduce `max_step`, or lower `dt`.
- Auto-pause (§5.2) stops oscillation at convergence — but the physics parameters must *reach* convergence within the 3-second window for the rule to hold.
- For graphs that don't converge in 3s (large graphs, bad initial positions), show a "still settling…" indicator rather than a frozen simulation that looks stuck.

