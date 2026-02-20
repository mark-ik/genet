<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Graph UX Polish Plan (2026-02-19)

**Status**: In Progress (implementation largely complete; closeout is remnant UX items, Phase 5 ownership handoff, and headed/manual validation)

---

## Plan

### Context

Core browsing graph is functional (M1 complete, F1-F7 architectural features complete). The graph UX
research report (`2026-02-18_graph_ux_research_report.md`) identified ~15 polish items not addressed
by the layout strategy, edge operations, or workspace routing plans. This plan collects them into
phases ordered by effort/value ratio from the research report Â§11 priority table.

Physics micro-improvements (auto-pause, reheat, new-node placement â€” research Â§5 and Â§2.6) are
tracked in `2026-02-19_layout_advanced_plan.md Â§Phase 1` with the other layout-system changes.

The layout strategy plan covers physics presets and algorithmic layout (Sugiyama, Radial, BH). The
edge plan covers multi-select wiring and command palette UX. This plan covers the remainder.

### Refactor Note (2026-02-20)

Plan ownership has been split for clarity:

1. Traversal-coupled search/filter work from Phase 5 is now owned by
   `2026-02-20_edge_traversal_impl_plan.md` ("Scope Absorption from Graph UX Polish Phase 5").
2. Settings/configurability architecture is now owned by
   `2026-02-20_settings_architecture_plan.md`, with current toolbar `Settings` treated as bridge
   implementation.
3. This plan now focuses on non-traversal UX remnants and headed validation completion.
4. Any "settings missing" wording in older notes should be read as pre-bridge context only; current
   in-app `Settings` controls (toolbar menu + persisted preferences) are considered live bridge UX.

### User Feedback Intake (2026-02-19)

Manual validation feedback identified radial-menu usability issues:

1. Current radial is visually cluttered (too many actions at once), but also spatially oversized.
2. Desired direction: smaller control footprint, clearer grouping/readability, and more robust
   command discoverability.
3. Suggested follow-up: rework radial placement/spacing using the same intentional layout approach
   used in graph layout planning (algorithmic spacing/packing), or reduce radial scope and keep
   power actions in command palette.
4. Suggested interaction model: directional radial navigation (for example hold `R`, use arrow
   keys to choose domain/command) to avoid pointer precision issues and overlap with draggable
   graph nodes.
5. Context menu replacement was preferred after headed validation, with follow-up asks for:
   menu hierarchy (clear submenus/groups), and a workspace action to add the current tab into a
   target workspace from that menu.

Follow-up item: add a dedicated radial redesign phase or split "quick radial" vs "full command
palette" responsibilities.

---

### Phase 1: Navigation & Interaction Polish (smallâ€“medium effort)

#### 1.1 Keyboard Zoom (`+` / `-` / `0`)

Keyboard zoom for users without scroll wheels or trackpads. Also standard in all graph tools.

- `+`/`=`: zoom in 10%.
- `-`: zoom out 10%.
- `0`: reset to 1.0Ã—.
- Guard: only in graph view when no text field is focused (same pattern as existing keyboard
  shortcuts in `input/mod.rs`).
- Mechanism: write to `MetadataFrame` after `GraphView` renders (same path as zoom clamp).

**Tasks**

- [x] Add `zoom_in: bool`, `zoom_out: bool`, `zoom_reset: bool` flags to `KeyboardActions`.
- [x] Detect `Key::Plus`/`Key::Equals`, `Key::Minus`, `Key::Num0` in `input/mod.rs`.
- [x] Apply zoom delta to `MetadataFrame` in post-render hook; clamp to existing `[0.1, 10.0]`
  bounds.

**Validation Tests**

- `test_keyboard_zoom_in_increases_zoom` â€” `zoom_in` flag â†’ zoom increases by ~10%.
- `test_keyboard_zoom_out_decreases_zoom` â€” `zoom_out` flag â†’ zoom decreases by ~10%.
- `test_keyboard_zoom_reset` â€” `zoom_reset` flag â†’ zoom returns to 1.0.
- Headed: keyboard zoom with text field focused produces no zoom change.

---

#### 1.2 Smart Fit (`Z` key)

`Z` is the single keyboard fit control:
- with 2+ selected nodes, fit the viewport to their bounding box.
- with 0 or 1 selected node, fit the full graph (formerly the `C` key behavior).

- Compute axis-aligned bounding box (AABB) of selected node positions from `app.graph`.
- Add 20% padding.
- Write zoom + pan to `MetadataFrame`.

**Tasks**

- [x] Add `zoom_to_selected: bool` to `KeyboardActions`; detect `Key::Z`.
- [x] In apply/render phase: if `selected_nodes` has 2+ nodes, compute AABB and write to
  `MetadataFrame`.
- [x] If `selected_nodes` has 0 or 1 node, fall through to full-graph fit.

**Validation Tests**

- `test_zoom_to_selected_computes_correct_aabb` â€” two selected nodes at known positions â†’ expected
  AABB with 20% padding.
- `test_zoom_to_selected_falls_back_to_fit_when_selection_empty` â€” no selection â†’ fit-to-screen.
- `test_zoom_to_selected_falls_back_to_fit_when_single_selected` â€” single selection â†’ fit-to-screen.

---

#### 1.3 Pin Node Visual Indicator + Keyboard Toggle

The data model (`node.is_pinned`, `PinNode` log entry, `sync_graph_positions_from_layout` honor
logic) is complete. The visual indicator and `L` keyboard shortcut are implemented (Session 3);
only `KEYBINDINGS.md` update remains.

- **Visual**: small white filled circle (5px radius) at node center-top in `GraphNodeShape::ui()`.
- **Keyboard**: `L` key ("Lock") toggles pin on primary selected node. `P` stays as physics panel.
- Update `KEYBINDINGS.md` and help panel with `L`.

**Tasks**

- [x] In `GraphNodeShape::ui()` (or `graph/egui_adapter.rs`): if `node.is_pinned`, paint indicator.
- [x] Add `toggle_pin` keyboard action to `KeyboardActions`; detect `Key::L` in `input/mod.rs`.
- [x] Emit `GraphIntent` for pin-toggle from keyboard actions handler (`TogglePrimaryNodePin`).
- [x] Update `KEYBINDINGS.md` with `L` entry (help panel and in-graph shortcut overlay updated).

**Validation Tests**

- `test_toggle_pin_primary_action_maps_to_intent` â€” `toggle_pin` flag emits
  `GraphIntent::TogglePrimaryNodePin`.
- Headed: pinned node shows indicator in graph view; unpinned node shows none.

---

#### 1.4 Scroll Wheel / Trackpad Zoom Speed

The current `zoom_speed` of `0.05` in `SettingsNavigation::with_zoom_speed()` (`render/mod.rs:91`)
produces jumpy zoom increments â€” each scroll wheel notch or trackpad swipe overshoots noticeably.
Reduce to `0.01`â€“`0.02` so one scroll notch zooms ~5â€“10% (depending on egui's scroll-delta
normalization per platform).

Trackpad and smooth-scroll devices deliver many small deltas per frame, so reducing the speed
also improves the trackpad glide feel. The `[0.1, 10.0]` zoom clamp already prevents runaway zoom.

**Tasks**

- [x] In `render/mod.rs`, change `.with_zoom_speed(0.05)` to `.with_zoom_speed(0.01)`.
- [ ] Validate headed on both a scroll wheel and a trackpad; adjust if needed (target: 5â€“10% zoom
  change per distinct scroll notch).

**Validation Tests**

- Headed: one scroll wheel notch â†’ zoom changes by ~5â€“10%, not a large jump.
- Headed: smooth trackpad swipe â†’ zoom transitions feel continuous and proportional.

---

### Phase 2: Hover & Labels (medium effort)

#### 2.1 Hover Tooltip

Long URLs are truncated in node labels. No way to see the full URL, title, timestamp, or lifecycle
state without opening the node. Research Â§7.4.

- Attach tooltip to node widget response in the render path.
- Content: full URL, title (if different from URL), last visited (human-readable delta), lifecycle
  state.
- Implementation note: egui_graphs node responses may need to be intercepted via `hovered_node`
  from egui_state and an `egui::Area` overlay rather than direct `response.on_hover_ui()`.

**Tasks**

- [x] Locate correct attachment point for node hover UI in `render/mod.rs` or
  `graph/egui_adapter.rs`.
- [x] Format tooltip: full URL, title (omit if == URL), last-visited delta (e.g. "3 hours ago"),
  lifecycle.
- [x] Ensure tooltip does not block graph interaction.

**Validation Tests**

- Headed: hover a node with a long URL â†’ tooltip shows full URL, title, timestamp, lifecycle.
- Headed: tooltip dismisses promptly when cursor leaves node.

---

#### 2.2 Zoom-Adaptive Labels

At low zoom, many node labels become unreadable clutter. Research Â§7.3.

| Zoom range | Label shown |
| --- | --- |
| > 1.5 | Full title or URL |
| 0.6 â€“ 1.5 | Domain only, or first 20 chars of title |
| < 0.6 | No text label |

Note: labels are currently hover/select/drag-only (see `egui_adapter.rs:118` early return). This
plan changes that: labels become always-visible but zoom-tier-gated. The `< 0.6` tier restores
the label-hidden behavior. Favicon rendering is separate â€” see Â§2.4.

- Read `app.camera.current_zoom` (already synced from `MetadataFrame`) in `GraphNodeShape`.
- Select label string based on zoom tier; render unconditionally (remove the
  `!(selected || dragged || hovered)` gate from the label path, or add a zoom-tier check before it).

**Tasks**

- [x] In `GraphNodeShape::ui()`: read current zoom level (via parameter or app reference).
- [x] Implement 3-tier label string selection.
- [x] Remove or bypass the hover-only early return for the label when zoom > 0.6.

**Validation Tests**

- `test_label_tier_full` â€” zoom 2.0 â†’ full URL returned.
- `test_label_tier_domain` â€” zoom 1.0 â†’ domain-only or truncated title.
- `test_label_tier_none` â€” zoom 0.4 â†’ empty label string.
- Headed: zoom out â†’ labels progressively simplify without layout jank.

---

#### 2.3 Convergence Status Indicator Upgrade

Extends the existing "Physics: Running / Paused" overlay to 4 states. This is the display side of
auto-pause (implemented in `2026-02-19_layout_advanced_plan.md Â§1.1`). The 4-state extension
("Running" / "Settling" / "Settled" / "Paused") is described and tested there; this section
serves as a cross-reference.

---

#### 2.4 Node Visual Hierarchy: Favicon Always, Thumbnail on Hover/Focus

**Design**:

- **Favicon**: always rendered inside the node circle â€” the resting identity of every node.
- **Thumbnail**: rendered only when `selected || dragged || hovered` â€” the active/preview state.
  Overlays the favicon when both are available.
- **Fallback**: if no favicon is loaded, colored dot (domain-hash fill, existing behavior).

This replaces the current rendering priority (`thumbnail > favicon, both unconditional`) with a
state-driven model where the thumbnail acts as a focus indicator rather than a persistent overlay.

**Current code** (`egui_adapter.rs:106â€“116`):

```rust
if let Some(t) = self.ensure_thumbnail_texture(ctx) {
    // render thumbnail always
} else if let Some(f) = self.ensure_favicon_texture(ctx) {
    // render favicon only if no thumbnail
}
```

**New behavior**:

```rust
// Favicon: always (resting state)
if let Some(favicon_id) = self.ensure_favicon_texture(ctx) {
    // render favicon
}
// Thumbnail: overlay only on hover/select/drag
if self.selected || self.dragged || self.hovered {
    if let Some(thumb_id) = self.ensure_thumbnail_texture(ctx) {
        // render thumbnail over favicon
    }
}
```

**Tasks**

- [x] In `egui_adapter.rs GraphNodeShape::shapes()`: separate favicon (unconditional) from
  thumbnail (hover/select/drag-gated) rendering.
- [x] Verify thumbnail alpha-blends over favicon correctly (both at full opacity â†’ thumbnail
  occludes favicon; use `Color32::WHITE` for both as now).

**Validation Tests**

- `test_favicon_renders_without_hover` â€” node with favicon, not hovered/selected â†’ favicon shape
  present in output.
- `test_thumbnail_renders_only_on_hover` â€” node with both favicon and thumbnail, not
  hovered/selected â†’ only favicon shape; no thumbnail. On hover â†’ both shapes present.
- Headed: unfocused nodes show favicon; hover a node â†’ thumbnail appears over the favicon.

---

### Phase 3: Visual Differentiation (mediumâ€“large effort)

Note: `2026-02-19_workspace_routing_and_membership_plan.md Â§Phase 4` adds a workspace-membership
badge to `GraphNodeShape` (a `[N]` count or small graphical badge with hover tooltip listing
workspace names). That badge renders in the same node shape layer as the changes below. Coordinate
implementation to avoid overlapping UI elements.

#### 3.1 Edge Type Visual Differentiation

All three edge types (`Hyperlink`, `History`, `UserGrouped`) render identically. Research Â§7.2
shows type differentiation significantly reduces time-to-interpretation.

| Edge type | Visual | Rationale |
| --- | --- | --- |
| `Hyperlink` | Solid thin line, neutral color | Default/common; lowest visual weight |
| `History` | Dashed line | Traversal semantics; "broken" = traversed path |
| `UserGrouped` | Solid thicker line, amber | User-intentional; highest visual weight |

Requires a custom `EdgeShape` implementation in egui_graphs 0.29.

**Tasks**

- [x] Investigate egui_graphs 0.29 `EdgeShape` trait API (docs.rs/egui_graphs).
- [x] Implement `GraphEdgeShape` in `graph/egui_adapter.rs` branching on `EdgeType`.
- [x] Wire into `EguiGraphState::from_graph()` edge construction.

**Validation Tests**

- `test_edge_shape_selection` â€” `EdgeType::History` â†’ dashed style; `EdgeType::UserGrouped` â†’
  thick amber.
- Headed: all three edge types visible with distinct styles in graph view.

---

#### 3.2 Neighbor Highlight on Hover

When hovering a node, dim all non-adjacent nodes and edges. Reveals local neighborhood without
requiring selection or search. Research Â§7.6.

- Use `hovered_graph_node` (already tracked per-frame via egui_graphs `hovered_node()`).
- In the color projection step: if `hovered_graph_node` is Some, compute adjacency set via
  `out_neighbors` + `in_neighbors`. Dim (reduce alpha/brightness) all non-adjacent nodes.
- Selection takes visual precedence over dimming.
- Restore on hover end.

**Tasks**

- [x] In color projection (adapter or render): branch on `hovered_graph_node`.
- [x] Compute adjacency set; apply dim to non-adjacent nodes and their incident edges.
- [x] Ensure selected-node color takes priority over dimmed state.

**Validation Tests**

- `test_neighbor_set_computation` â€” known graph: hover node A â†’ correct adjacent set computed.
- Headed: hover a node â†’ non-adjacent nodes dim; hover ends â†’ normal colors restore.

---

#### 3.3 Highlight vs. Filter Search Mode Toggle

Current search hides non-matching nodes entirely. Research Â§9.1 recommends "highlight" mode
(dim non-matching, preserve context) as the default; "filter" as the secondary option.

- Add `SearchDisplayMode` enum (`Highlight` / `Filter`) to `GraphBrowserApp`.
- In `apply_search_node_visuals()`: branch on mode for dim-vs-hide.
- Add toggle button in `desktop/graph_search_ui.rs`.
- Default: `Highlight`.

**Tasks**

- [x] Add `SearchDisplayMode` enum and `search_display_mode` field to `app.rs`.
- [x] Update `apply_search_node_visuals()` to branch on mode.
- [x] Add toggle in `desktop/graph_search_ui.rs`.
- [x] Initialize to `Highlight`.

**Validation Tests**

- `test_search_highlight_mode_dims_not_hides` â€” in Highlight mode, non-matching nodes are present
  but dimmed.
- `test_search_filter_mode_hides_nodes` â€” in Filter mode, non-matching nodes are absent from
  render.
- Headed: toggle between modes during active search; correct behavior for both.

---

#### 3.4 Crashed Node Indicator

Servo's crash recovery is visible in the detail view tile (error overlay) but not in the graph
view. A node whose webview has crashed shows no distinct visual state; it looks identical to a
cold node. Research Â§7.1 identifies this as a missing state.

- Apply a red/orange tint (or colored ring) to nodes whose `webview_state == Crashed`.
- Should have lower visual weight than selection amber â€” a tint on the existing circle color is
  sufficient.
- Restore to normal color when the webview recovers or the node is navigated.

**Tasks**

- [x] Confirm `Node` or app-level state carries a `Crashed` lifecycle variant distinguishable from
  `Cold` (check `node.webview_state` or equivalent field).
- [x] In `GraphNodeShape` color projection: if crashed, apply red/orange tint.
- [x] Ensure crashed color does not override `Selected` (amber takes priority).

**Validation Tests**

- `test_crashed_node_color_differs_from_cold` â€” crashed node produces a different color than a
  cold node.
- Headed: crash a tab; corresponding graph node shows red/orange tint; recover tab â†’ tint clears.

---

#### 3.5 Multi-Select Visual: Halo on All Selected Nodes

`Ctrl+Click` multi-select is implemented (edge plan Step 1), but only the primary selected node
shows its full selected color. Secondary selected nodes may not have a distinct visual indicator.
Research Â§7.1: "Distinct border or halo on all selected nodes, not just primary."

- Primary selected node: existing amber fill (unchanged).
- Secondary selected nodes (in `selected_nodes` set but not `primary()`): visible halo or border
  ring in the same amber, reduced opacity or stroke-only, to signal "part of the current
  selection" without displacing primary visual hierarchy.

**Tasks**

- [x] In `GraphNodeShape` color projection: distinguish primary vs. secondary selected nodes.
- [x] Apply a stroke-only ring (amber, stroke width ~2px) to secondary selected nodes.
- [x] Ensure secondary halo does not override hovered or dragged state colors.

**Validation Tests**

- `test_secondary_selected_color_differs_from_primary` â€” two selected nodes: primary â†’ amber fill;
  secondary â†’ different visual (stroke-only or reduced fill).
- Headed: Ctrl+Click two nodes â†’ both visually indicated as selected with clear hierarchy.

---

### Phase 4: Multi-Select Extensions (in progress)

Rationale: `rstar` here is a UX interaction-performance improvement for lasso/hit-testing, not a layout algorithm change.

Workspace routing Phases 1â€“3 are complete. Group drag implemented via `sync_graph_positions_from_layout` â€” no Step 4d gate needed (sync-layer approach is independent of edge operations).

- **`Ctrl+A` select all**: emit `SelectAll` intent â†’ populate `selected_nodes` with all `NodeKey`s.
- **Group drag**: when dragging a node that is in `selected_nodes`, apply same delta to all selected
  nodes. Requires reading drag delta from egui_graphs event and iterating selection set.

**Tasks**

- [x] Implement lasso gesture as `Right+Drag` in graph view to avoid right-click context-menu conflicts.
- [x] Add bulk selection intent path supporting `Replace` / `Add` / `Toggle` semantics.
- [x] Wire modifier behavior: `Right+Drag` = replace, `Right+Ctrl+Drag` = add, `Right+Alt+Drag` = toggle.
- [x] Render lasso rectangle overlay during drag.
- [x] Extend with group drag for selected-node sets.
- [x] Add `Ctrl+A` select-all intent.
- [x] Evaluate optional right-drag lasso mode once context-menu redesign is finalized.
- [x] Add `rstar`-backed spatial index for graph-node hit-testing (lasso/box queries in world space).
- [x] Route right-drag lasso selection through spatial range queries instead of full-node scans.
- [x] Add perf validation at medium/large node counts to verify lasso frame-time improvement.

---

### Phase 5: Search & Context Filtering (medium effort)

Ownership update (2026-02-20):
- Phase 5.1 through 5.4 are traversal-coupled and now tracked under
  `2026-02-20_edge_traversal_impl_plan.md`.
- Phase 5.5 (`Shift+Click` range select) and 5.6 (`R` manual reheat) remain in this plan as UX
  remnants not blocked on traversal-model migration.

Dependency boundary:
- Keep this section as historical/requirements context for 5.1-5.4 only.
- Do not execute 5.1-5.4 implementation work from this plan.
- Execute traversal-coupled delivery, sequencing, and acceptance criteria from
  `2026-02-20_edge_traversal_impl_plan.md`.
- Keep only non-traversal, direct UX/input remnant execution here (currently 5.5 and 5.6).

#### 5.1 Neighborhood Filter (N-hop Focus)

"Show me this node and everything reachable within N hops." Keeps the selected node and its
N-hop graph neighbors visible; dims or hides everything else. Research Â§9.2. Natural companion to
Phase 3.2 neighbor highlight â€” this is the persistent, pinnable version of that hover interaction.

- N=1 (direct neighbors) is the primary case; N=2 a secondary option.
- Expose as a right-click / context menu action: "Focus neighborhood."
- When active, a dismissal affordance (`Esc` or toolbar button) restores full graph view.
- Composable with search: focus applied on top of any active search query.

**Tasks**

- [ ] Add `neighborhood_focus: Option<(NodeKey, u8)>` field to `GraphBrowserApp` (node + hop depth).
- [ ] In `apply_search_node_visuals()`: when set, compute reachable set (BFS to depth N via
  `out_neighbors` + `in_neighbors`) and dim/hide non-reachable nodes; stack with existing search
  dim logic.
- [ ] Add "Focus neighborhood" entry in node context menu.
- [ ] Emit `GraphIntent::SetNeighborhoodFocus(NodeKey, depth: u8)` and `ClearNeighborhoodFocus`.
- [ ] `Esc` or toolbar dismiss clears `neighborhood_focus`.

**Validation Tests**

- `test_neighborhood_focus_bfs_depth_1` â€” known graph: focus node A at depth 1 â†’ only A and
  direct neighbors in reachable set; others excluded.
- `test_neighborhood_focus_bfs_depth_2` â€” same graph at depth 2 â†’ two-hop neighbors included.
- `test_neighborhood_focus_cleared_on_intent` â€” `ClearNeighborhoodFocus` intent â†’ field is None.
- Headed: right-click node â†’ "Focus neighborhood" â†’ non-adjacent nodes dim; Esc â†’ restored.

---

#### 5.2 Edge-Type Filter

Filter the visible graph to nodes connected by a specific edge type. Useful for reviewing
intentional groupings (`UserGrouped` filter) or raw navigation paths (`History` filter). Research Â§9.3.

- Toggle filter chips in the search panel: `[All] [Hyperlink] [History] [UserGrouped]`.
- When active, nodes with no incident edges of the selected type are dimmed or hidden (same
  `SearchDisplayMode` Highlight/Filter as Phase 3.3).
- Composable: edge-type filter stacks with text search and neighborhood focus.

**Tasks**

- [ ] Add `edge_type_filter: Option<EdgeType>` field to `GraphBrowserApp`.
- [ ] In `apply_search_node_visuals()`: when set, exclude nodes that have no incident edges of the
  filtered type (Highlight â†’ dim; Filter â†’ hide).
- [ ] Add edge-type filter chip row in `desktop/graph_search_ui.rs`.
- [ ] Emit `GraphIntent::SetEdgeTypeFilter(EdgeType)` and `ClearEdgeTypeFilter`.

**Validation Tests**

- `test_edge_type_filter_excludes_unmatched_nodes` â€” graph with `UserGrouped` and `History` edges;
  filter to `UserGrouped` â†’ node connected only by `History` is excluded.
- `test_edge_type_filter_none_shows_all` â€” filter cleared â†’ all nodes visible.
- Headed: activate edge-type filter chip; unmatched nodes dim; toggle off â†’ restored.

---

#### 5.3 Faceted Search

Extend the `Ctrl+F` search bar to parse structured filter clauses. Research Â§13.3.

| Syntax | Filter |
| --- | --- |
| `domain:github.com` | URL host matches |
| `date:>2026-01-01` | `last_visited` after date |
| `date:<2026-01-01` | `last_visited` before date |
| `is:pinned` | `node.is_pinned == true` |
| `is:active` | node has a live webview |

Bare text (no `:`) continues to match title/URL substring as now. Multiple clauses AND together.

**Tasks**

- [ ] Add a facet query parser: tokenize on whitespace; split `key:value`; dispatch to per-facet
  filter predicates.
- [ ] Add facet fields to search state: `domain_filter`, `date_after`, `date_before`,
  `pinned_only`, `active_only`.
- [ ] Route parsed facets into `apply_search_node_visuals()` alongside existing text filter.
- [ ] Update search bar placeholder: `"Searchâ€¦ (domain:, date:>, is:pinned)"`.

**Validation Tests**

- `test_facet_parse_domain` â€” `"domain:rust-lang.org"` â†’ domain filter set.
- `test_facet_parse_date_after` â€” `"date:>2026-01-01"` â†’ `date_after` = 2026-01-01 UTC.
- `test_facet_parse_is_pinned` â€” `"is:pinned"` â†’ `pinned_only = true`.
- `test_facet_compose_text_and_domain` â€” `"servo domain:servo.org"` â†’ both text and domain
  filters active simultaneously.
- `test_facet_domain_excludes_non_matching` â€” node URL does not match domain facet â†’ excluded.
- Headed: type `domain:` in search bar; non-matching domain nodes dim.

---

#### 5.4 Degree of Interest (DOI) Filtering

A continuous relevance score drives node visual weight instead of binary show/hide. Research
Â§13.2, Â§14.8.

**Formula:**

```text
DOI(n) = Î±Â·Recency(n) + Î²Â·Frequency(n) + Î³Â·ExplicitInterest(n) âˆ’ Î´Â·DistanceFromFocus(n)
```

- `Recency`: `1 / (1 + seconds_since_last_visit)` â€” high for recently-visited nodes.
- `Frequency`: `log(1 + visit_count)` â€” high for frequently-visited nodes.
- `ExplicitInterest`: pinned = 1.0, otherwise 0.0.
- `DistanceFromFocus`: graph hop distance from selected/hovered node (0 for the node itself).
- Default weights: Î±=0.4, Î²=0.3, Î³=0.2, Î´=0.1. Expose as sliders in a "Relevance" sub-panel.

**Visualization (three DOI tiers):**

| Score | Rendering |
| --- | --- |
| High | Full size, full opacity, full label |
| Medium | Normal size, domain-only label |
| Low | Shrinks to dot, muted color, no label â€” but still visible (preserves context) |

Compute throttled at ~100ms; cache in app state. Default opt-in: `doi_enabled: bool = false`.

**Tasks**

- [ ] Add `doi_enabled: bool` and `DoiWeights { alpha, beta, gamma, delta }` to `GraphBrowserApp`
  (or a `DoiState` sub-struct).
- [ ] Implement `compute_doi(node, graph, focused_node: Option<NodeKey>) -> f32`.
- [ ] Add throttled DOI recompute (100ms tick): runs when `doi_enabled` and selected node changed
  or graph structurally changed.
- [ ] In `GraphNodeShape::ui()`: when `doi_enabled`, scale node radius and label tier by DOI score.
- [ ] Add DOI toggle + weight sliders in a collapsible "Relevance filter" section of the search panel.

**Validation Tests**

- `test_doi_recency_decays_with_time` â€” node visited 1s ago vs. node visited 1 year ago â†’ recency
  component significantly higher for recent node.
- `test_doi_pinned_contributes_explicit_interest` â€” pinned node â†’ `ExplicitInterest` term = Î³.
- `test_doi_focus_node_has_zero_distance_penalty` â€” selected node â†’ distance component = 0.
- `test_doi_disabled_uniform` â€” when `doi_enabled = false`, all nodes compute DOI = 1.0.
- Headed: enable DOI, select a hub node; neighbors prominent; distant nodes shrink to dots.

---

#### 5.5 `Shift+Click` Range Select

Deferred in Section 6.3 of the research report ("needs ordered `SelectionState`"). Research Section 6.1.

Policy decision (2026-02-20):

- `Shift+Click` is the cross-domain range-select gesture for ordered surfaces.
  This applies to workspace tabs, omnibar/list rows, and other orderable UI lists.
- Graph-view spatial selection remains box/lasso based:
  `Right+Drag` (default, already implemented with `rstar` hit-testing),
  plus optional `Shift+Right+Drag` for additive range-box selection semantics.
- Do not force ordered-list semantics onto free-form graph topology.
  Graph selection should stay spatial-first.

**Tasks**

- [x] Decision recorded: cross-domain ordered range for `Shift+Click`; graph uses spatial box/lasso.
- [x] Introduce a shared ordered-range selection primitive for list-like surfaces
  (`anchor_index`, `target_index`, inclusive range application).
- [x] Apply ordered-range primitive to workspace tabs and omnibar/list contexts.
- [x] For graph interactions, add optional `Shift+Right+Drag` additive box-select mode
  (reusing existing lasso rectangle + `rstar` query path).
- [x] Ensure gesture precedence and context-menu suppression remain deterministic when modifiers are held.

**Validation Tests**

- Ordered surfaces: click item A, Shift+click item B -> inclusive contiguous range is selected.
- Graph: `Right+Drag` keeps current lasso behavior; `Shift+Right+Drag` applies additive spatial range.
- Headed: tabs, omnibar rows, and graph each follow the policy without cross-surface gesture conflicts.

---

#### 5.6 `R` Key - Manual Reheat

The layout advanced plan Section 1.2 adds automatic reheat when adding a node or edge. The `R` key
provides manual reheat from any state: set `physics.is_running = true` from current positions
without resetting velocities. Research Section 6.4.

**Tasks**

- [x] Add `reheat: bool` to `KeyboardActions`; detect `Key::R` in `input/mod.rs` (graph view only,
  no text field focused).
- [x] Emit `GraphIntent::ReheatPhysics`.
- [x] In `apply_intent()`: set `physics.is_running = true`.
- [x] Update keyboard overlay help text and `KEYBINDINGS.md`.

**Validation Tests**

- `test_r_key_emits_reheat_intent` - `Key::R`, no modifier, graph view -> `ReheatPhysics` intent.
- `test_reheat_intent_enables_physics` - `ReheatPhysics` intent -> `physics.is_running == true`.
- Headed: pause physics, press `R` -> simulation resumes from current positions.

---

## Findings

Research source: `2026-02-18_graph_ux_research_report.md`

Key section cross-references per phase:

- Phase 1: Â§6.2 (pinning workflow), Â§8.1 (keyboard zoom), Â§8.2 (zoom-to-selected)
- Phase 2: Â§7.3 (zoom-adaptive labels), Â§7.4 (hover tooltip), Â§7.5 (convergence indicator)
- Phase 3: Â§7.2 (edge differentiation), Â§7.6 (neighbor highlight), Â§9.1 (highlight vs. filter)
- Phase 4: Â§6.3 (multi-select extensions)

Research Â§11 priority table items tracked:

| Priority | Item | Location |
|---|---|---|
| #1 | `Ctrl+Click` multi-select | âœ… done (edge plan Step 1) |
| #2 | Pin node UX | complete (shortcut + docs + visual) |
| #3 | Physics presets | Not yet â€” no preset system exists (archived plan [x] marks are wrong) |
| #4 | Auto-pause on convergence | Layout Advanced Plan Â§1.1 |
| #5 | Reheat on structural change | Layout Advanced Plan Â§1.2 |
| #6 | Hover tooltip | Phase 2.1 |
| #7 | Keyboard zoom | Phase 1.1 |
| #8 | New-node placement near neighbors | Layout Advanced Plan Â§1.3 |
| #9 | Zoom to selected | Phase 1.2 |
| #10 | Edge type visual differentiation | Phase 3.1 |
| #11 | Zoom-adaptive labels | Phase 2.2 |
| #12 | Convergence status indicator | Phase 2.3 (see Layout Advanced Plan Â§1.1) |
| #13 | Neighbor highlight on hover | Phase 3.2 |
| #14 | Highlight vs. filter search toggle | Phase 3.3 |
| #15 | Crashed node indicator | Phase 3.4 |
| â€” | Multi-select halo (all selected nodes) | Phase 3.5 |
| #16-18 | Lasso, group drag, edge hit targets | Phase 4 (lasso âœ… done; group drag âœ… done; edge hit targets deferred) |

Research Â§14 advanced recommendations (degree-dependent repulsion, greedy label culling, invisible
layout constraints) are tracked in `2026-02-19_layout_advanced_plan.md Â§Phase 2`.

---

## Progress

### 2026-02-19 â€” Session 1

- Plan created from research report Â§11 priority table and Â§2â€“9 detail sections.
- Phases 1â€“3 have full task lists and unit test stubs.
- Phase 4 deferred pending pair-operation and workspace routing stability.
- Implementation not started.

### 2026-02-19 â€” Session 2

- Physics micro-improvements (original Phase 1: auto-pause, reheat, new-node placement) moved to
  `2026-02-19_layout_advanced_plan.md Â§Phase 1` to consolidate layout-system changes.
- Remaining phases renumbered: old 2â†’1, old 3â†’2, old 4â†’3, old 5â†’4.

### 2026-02-19 â€” Session 3

- Implemented keyboard-grouped node context menu navigation (Left/Right group switch, Up/Down
  action cycle, Enter execute) with persistent focus state.
- Added Persistence Hub `Load Pin...` chooser popup with `Workspace Pin` and `Pane Pin` restore
  actions.
- Implemented pin UX polish items: `L` toggles primary-node pin state, help/overlay shortcut text
  updated, and pinned nodes now render a top-center marker.
- Reduced graph zoom speed to `0.01` for finer wheel/trackpad control.

### 2026-02-19 - Session 4

- Implemented Phase 1.1 keyboard zoom (`+`/`-`/`0`) end-to-end:
  input flags, intent mapping, app request queueing, and post-render `MetadataFrame` updates.
- Implemented Phase 1.2 `Z` zoom-to-selected:
  selected-node AABB fit with 20% padding, plus no-selection fallback to fit-to-screen.
- Updated graph overlay/help text with the new zoom shortcuts.
- Added app-level tests for keyboard zoom request queueing and zoom-to-selected fallback behavior.
- Follow-up adjustment: retired `C` keyboard fit shortcut; `Z` now owns smart-fit
  (2+ selected â†’ fit selection, 0/1 selected â†’ fit graph).

### 2026-02-19 - Session 5

- Implemented Phase 2.1 hover tooltip in `render/mod.rs` using hovered-node context.
- Tooltip now shows title/URL, relative last-visited time, and lifecycle state.
- Tooltip is rendered on a non-interactable tooltip layer and suppresses itself while hovering
  workspace-membership badges to avoid overlap.
- Added render-layer unit tests for relative-time formatting helpers.


### 2026-02-20 - Session 6

- Completed Phase 1.3 doc follow-up by adding `ports/graphshell/KEYBINDINGS.md` with `L` toggle-pin
  and current graph shortcuts.
- Implemented Phase 2.2 zoom-adaptive labels with three tiers (`>1.5` full, `0.6-1.5` simplified,
  `<0.6` hidden) and removed hover-only label gating when zoom supports labels.
- Implemented Phase 2.4 node visual hierarchy update: favicon always renders, thumbnail overlays
  only on hover/select/drag.
- Implemented Phase 3.1 custom `GraphEdgeShape` in `graph/egui_adapter.rs` and wired edge-type
  styling into egui graph construction.
- Implemented Phase 3.2 neighbor highlight dimming (non-adjacent nodes/edges dim while hovered)
  with selected-node precedence.
- Implemented Phase 3.3 search display mode toggle with `SearchDisplayMode` (`Highlight` default /
  `Filter`) in app state and graph-search UI.
- Implemented Phase 3.4 crashed-node graph tint using runtime crash metadata, with primary selected
  amber precedence retained.
- Implemented Phase 3.5 multi-select secondary halo (stroke ring on non-primary selected nodes),
  without overriding hovered/dragged styling.
- Added/updated unit tests for label tiers, edge-shape style selection, secondary-selection visual
  role, crashed-vs-cold color projection, neighbor-set computation, and search highlight/filter
  behavior.

### 2026-02-20 - Session 7

- Implemented first-pass lasso multi-select in graph view with `Right+Drag` rectangle selection.
- Added bulk selection semantics via `SelectionUpdateMode` and `GraphIntent::UpdateSelection`
  (`Replace`, `Add`, `Toggle`) for future transform features.
- Wired modifier behavior: `Right+Drag` replaces selection, `Right+Ctrl+Drag` adds, and
  `Right+Alt+Drag` toggles inside-lasso nodes.
- Added lasso rectangle overlay rendering and updated graph shortcut help text.
- Added unit coverage for bulk selection reducer behavior and lasso action intent mapping.

### 2026-02-20 - Session 8

- Switched lasso activation from `Shift+LeftDrag` to `Right+Drag` with click-vs-drag thresholding.
- Added context-menu suppression on right-drag release so drag gesture does not also open node context UI.
- Added `arboard` clipboard actions for node `Copy URL` and `Copy Title` from node/context command surfaces.
- Added `egui-notify` non-blocking toasts for clipboard success/failure feedback.

### 2026-02-20 - Session 9

- Added UI feedback policy guidance for when to use blocking dialogs vs non-blocking toasts.

### 2026-02-20 - Session 10

- Implemented `Ctrl+A` select-all: `GraphIntent::SelectAll` added to `app.rs`, detected in
  `input/mod.rs` as `Ctrl+A`, mapped via `intents_from_actions`. Handler iterates `graph.nodes()`
  and replaces `selected_nodes` with all keys.
- Added `rstar`-backed `NodeSpatialIndex` in `render/spatial_index.rs`. Index is built in canvas
  (world) space from node positions; queries use `MetadataFrame::screen_to_canvas_pos` to invert
  the lasso screen rect into canvas space before the range query.
- Replaced the O(n) linear scan in `collect_right_drag_lasso_action` with the rstar range query.
- Added 3 unit tests in `spatial_index.rs` (contained, excluded, empty graph) and 2 in
  `input/mod.rs` (select-all applies, select-all maps to intent). All 316 tests pass.

### 2026-02-19 - Session 11

- Implemented group drag via `sync_graph_positions_from_layout` sync-layer approach.
  During `is_interacting && selected_nodes.len() > 1`, detects the dragged node by finding a
  selected node whose egui_graphs canvas position diverges from `app.graph` position by >0.01.
  Applies the same delta to all other selected non-pinned nodes in both `app.graph` and
  `egui_state` directly (same pattern as pinned-node position restoration).
- No changes to `GraphAction`, `GraphIntent`, or `intents_from_actions` needed.
- Added `setup_group_drag_sync` helper and 2 unit tests. All 318 tests pass.

### 2026-02-20 - Session 12

- Finalized right-drag lasso as the default gesture and retained context-menu suppression with
  click-vs-drag thresholding.
- Added ignored perf test `perf_nodes_in_canvas_rect_10k_under_budget` in
  `render/spatial_index.rs` to validate medium/large-node spatial query performance.

### 2026-02-20 - Session 13

- Added persisted input-binding preferences in app state:
  lasso gesture (`RightDrag`/`ShiftLeftDrag`) and configurable command/help/radial shortcuts.
- Wired keyboard action collection through binding lookup (`input::collect_actions(ctx, graph_app)`).
- Added `Settings -> Input` UI controls for lasso and shortcut preferences.
- Updated graph overlay/help panel and `KEYBINDINGS.md` text to reflect configurable defaults.

### 2026-02-20 - Session 14

- Omnibar scope selectors (`@n`, `@N`, `@t`, `@T`, `@g`, `@b`, `@d`) now render as a persistent
  bottom row in the dropdown (search-engine grommet style).
- Scope row remains available even when there are zero result rows, so scope can be switched on
  first focus/empty query.
- Non-`@` still prioritizes local workspace tabs before provider suggestions.

### 2026-02-20 - Session 15

- Omnibar: added scoped provider commands (`@g`, `@b`, `@d`) with explicit provider submit target.
- Omnibar: added non-`@` ordering/cap controls and persisted settings for preferred scope + order.
- Omnibar: added provider suggestion debounce and visible loading/error status in dropdown.
- Zoom feel: added scroll-wheel/trackpad zoom inertia tail (damped per-frame velocity) for smoother
  motion without increasing base zoom sensitivity.

### 2026-02-20 - Session 16

- Exposed zoom inertia tuning as persisted user preferences in `Settings -> Graph Zoom`:
  `Inertia Impulse`, `Inertia Damping`, and `Inertia Stop Threshold`.
- Wired render zoom behavior to use persisted preference values instead of hardcoded constants.
- Added persistence coverage for zoom settings across restart.
- Added validation carry-over/watchlist alignment in `tests/VALIDATION_TESTING.md` to track warning/stub
  cleanup and dormant feature revival risk.

### 2026-02-20 - Session 17

- Implemented manual physics reheat intent path:
  `KeyboardActions.reheat_physics` -> `GraphIntent::ReheatPhysics` -> `apply_intent()` enabling simulation.
- Added a conflict guard so plain `R` reheat does not double-trigger when radial shortcut is configured to `R`.
- Added graph lasso additive shortcut parity:
  `Shift+Right+Drag` now maps to additive lasso mode (same selection mode as `Ctrl` add).
- Updated in-app shortcut help text and `KEYBINDINGS.md` for `R` reheat and additive lasso variants.

### 2026-02-20 - Session 18

- Added shared ordered-range helper (`desktop/selection_range.rs`) with inclusive index-range tests.
- Implemented independent workspace-tab multi-selection state (separate from graph node selection):
  plain click sets single tab selection, Ctrl-click toggles, Shift-click applies contiguous range
  within the current tab group.
- Added visible tab highlight for multi-selected tabs in tab strip rendering.
- Implemented omnibar row multi/range select:
  Ctrl-click toggle, Shift-click contiguous range.
- Added omnibar bulk actions for selected rows:
  `Open Selected` and `Add Selected To Workspace...` (workspace picker path).

---

## Dialog vs Toast Policy

Use dialogs only when user input or explicit confirmation is required before continuing.

- Use a dialog for destructive or irreversible actions that need explicit confirmation.
- Use a dialog for branching decisions that must be resolved immediately (for example unsaved workspace prompt).
- Use a dialog for required multi-field input that cannot be handled safely inline.

Use toasts for non-blocking feedback and status.

- Use a toast for success/failure outcomes (copy, save, switch data directory, settings apply).
- Use a toast for background progress/status messages.
- Use a toast for lightweight warnings and undo affordances.

Prefer inline panels over dialogs for persistent settings surfaces (for example Persistence Hub).
