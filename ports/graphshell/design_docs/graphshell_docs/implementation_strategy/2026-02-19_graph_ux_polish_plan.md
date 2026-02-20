<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Graph UX Polish Plan (2026-02-19)

**Status**: In Progress

---

## Plan

### Context

Core browsing graph is functional (M1 complete, F1-F7 architectural features complete). The graph UX
research report (`2026-02-18_graph_ux_research_report.md`) identified ~15 polish items not addressed
by the layout strategy, edge operations, or workspace routing plans. This plan collects them into
phases ordered by effort/value ratio from the research report §11 priority table.

Physics micro-improvements (auto-pause, reheat, new-node placement — research §5 and §2.6) are
tracked in `2026-02-19_layout_advanced_plan.md §Phase 1` with the other layout-system changes.

The layout strategy plan covers physics presets and algorithmic layout (Sugiyama, Radial, BH). The
edge plan covers multi-select wiring and command palette UX. This plan covers the remainder.

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

### Phase 1: Navigation & Interaction Polish (small–medium effort)

#### 1.1 Keyboard Zoom (`+` / `-` / `0`)

Keyboard zoom for users without scroll wheels or trackpads. Also standard in all graph tools.

- `+`/`=`: zoom in 10%.
- `-`: zoom out 10%.
- `0`: reset to 1.0×.
- Guard: only in graph view when no text field is focused (same pattern as existing keyboard
  shortcuts in `input/mod.rs`).
- Mechanism: write to `MetadataFrame` after `GraphView` renders (same path as zoom clamp).

**Tasks**

- [x] Add `zoom_in: bool`, `zoom_out: bool`, `zoom_reset: bool` flags to `KeyboardActions`.
- [x] Detect `Key::Plus`/`Key::Equals`, `Key::Minus`, `Key::Num0` in `input/mod.rs`.
- [x] Apply zoom delta to `MetadataFrame` in post-render hook; clamp to existing `[0.1, 10.0]`
  bounds.

**Validation Tests**

- `test_keyboard_zoom_in_increases_zoom` — `zoom_in` flag → zoom increases by ~10%.
- `test_keyboard_zoom_out_decreases_zoom` — `zoom_out` flag → zoom decreases by ~10%.
- `test_keyboard_zoom_reset` — `zoom_reset` flag → zoom returns to 1.0.
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

- `test_zoom_to_selected_computes_correct_aabb` — two selected nodes at known positions → expected
  AABB with 20% padding.
- `test_zoom_to_selected_falls_back_to_fit_when_selection_empty` — no selection → fit-to-screen.
- `test_zoom_to_selected_falls_back_to_fit_when_single_selected` — single selection → fit-to-screen.

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

- `test_toggle_pin_primary_action_maps_to_intent` — `toggle_pin` flag emits
  `GraphIntent::TogglePrimaryNodePin`.
- Headed: pinned node shows indicator in graph view; unpinned node shows none.

---

#### 1.4 Scroll Wheel / Trackpad Zoom Speed

The current `zoom_speed` of `0.05` in `SettingsNavigation::with_zoom_speed()` (`render/mod.rs:91`)
produces jumpy zoom increments — each scroll wheel notch or trackpad swipe overshoots noticeably.
Reduce to `0.01`–`0.02` so one scroll notch zooms ~5–10% (depending on egui's scroll-delta
normalization per platform).

Trackpad and smooth-scroll devices deliver many small deltas per frame, so reducing the speed
also improves the trackpad glide feel. The `[0.1, 10.0]` zoom clamp already prevents runaway zoom.

**Tasks**

- [x] In `render/mod.rs`, change `.with_zoom_speed(0.05)` to `.with_zoom_speed(0.01)`.
- [ ] Validate headed on both a scroll wheel and a trackpad; adjust if needed (target: 5–10% zoom
  change per distinct scroll notch).

**Validation Tests**

- Headed: one scroll wheel notch → zoom changes by ~5–10%, not a large jump.
- Headed: smooth trackpad swipe → zoom transitions feel continuous and proportional.

---

### Phase 2: Hover & Labels (medium effort)

#### 2.1 Hover Tooltip

Long URLs are truncated in node labels. No way to see the full URL, title, timestamp, or lifecycle
state without opening the node. Research §7.4.

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

- Headed: hover a node with a long URL → tooltip shows full URL, title, timestamp, lifecycle.
- Headed: tooltip dismisses promptly when cursor leaves node.

---

#### 2.2 Zoom-Adaptive Labels

At low zoom, many node labels become unreadable clutter. Research §7.3.

| Zoom range | Label shown |
| --- | --- |
| > 1.5 | Full title or URL |
| 0.6 – 1.5 | Domain only, or first 20 chars of title |
| < 0.6 | No text label |

Note: labels are currently hover/select/drag-only (see `egui_adapter.rs:118` early return). This
plan changes that: labels become always-visible but zoom-tier-gated. The `< 0.6` tier restores
the label-hidden behavior. Favicon rendering is separate — see §2.4.

- Read `app.camera.current_zoom` (already synced from `MetadataFrame`) in `GraphNodeShape`.
- Select label string based on zoom tier; render unconditionally (remove the
  `!(selected || dragged || hovered)` gate from the label path, or add a zoom-tier check before it).

**Tasks**

- [x] In `GraphNodeShape::ui()`: read current zoom level (via parameter or app reference).
- [x] Implement 3-tier label string selection.
- [x] Remove or bypass the hover-only early return for the label when zoom > 0.6.

**Validation Tests**

- `test_label_tier_full` — zoom 2.0 → full URL returned.
- `test_label_tier_domain` — zoom 1.0 → domain-only or truncated title.
- `test_label_tier_none` — zoom 0.4 → empty label string.
- Headed: zoom out → labels progressively simplify without layout jank.

---

#### 2.3 Convergence Status Indicator Upgrade

Extends the existing "Physics: Running / Paused" overlay to 4 states. This is the display side of
auto-pause (implemented in `2026-02-19_layout_advanced_plan.md §1.1`). The 4-state extension
("Running" / "Settling" / "Settled" / "Paused") is described and tested there; this section
serves as a cross-reference.

---

#### 2.4 Node Visual Hierarchy: Favicon Always, Thumbnail on Hover/Focus

**Design**:

- **Favicon**: always rendered inside the node circle — the resting identity of every node.
- **Thumbnail**: rendered only when `selected || dragged || hovered` — the active/preview state.
  Overlays the favicon when both are available.
- **Fallback**: if no favicon is loaded, colored dot (domain-hash fill, existing behavior).

This replaces the current rendering priority (`thumbnail > favicon, both unconditional`) with a
state-driven model where the thumbnail acts as a focus indicator rather than a persistent overlay.

**Current code** (`egui_adapter.rs:106–116`):

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
- [x] Verify thumbnail alpha-blends over favicon correctly (both at full opacity → thumbnail
  occludes favicon; use `Color32::WHITE` for both as now).

**Validation Tests**

- `test_favicon_renders_without_hover` — node with favicon, not hovered/selected → favicon shape
  present in output.
- `test_thumbnail_renders_only_on_hover` — node with both favicon and thumbnail, not
  hovered/selected → only favicon shape; no thumbnail. On hover → both shapes present.
- Headed: unfocused nodes show favicon; hover a node → thumbnail appears over the favicon.

---

### Phase 3: Visual Differentiation (medium–large effort)

Note: `2026-02-19_workspace_routing_and_membership_plan.md §Phase 4` adds a workspace-membership
badge to `GraphNodeShape` (a `[N]` count or small graphical badge with hover tooltip listing
workspace names). That badge renders in the same node shape layer as the changes below. Coordinate
implementation to avoid overlapping UI elements.

#### 3.1 Edge Type Visual Differentiation

All three edge types (`Hyperlink`, `History`, `UserGrouped`) render identically. Research §7.2
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

- `test_edge_shape_selection` — `EdgeType::History` → dashed style; `EdgeType::UserGrouped` →
  thick amber.
- Headed: all three edge types visible with distinct styles in graph view.

---

#### 3.2 Neighbor Highlight on Hover

When hovering a node, dim all non-adjacent nodes and edges. Reveals local neighborhood without
requiring selection or search. Research §7.6.

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

- `test_neighbor_set_computation` — known graph: hover node A → correct adjacent set computed.
- Headed: hover a node → non-adjacent nodes dim; hover ends → normal colors restore.

---

#### 3.3 Highlight vs. Filter Search Mode Toggle

Current search hides non-matching nodes entirely. Research §9.1 recommends "highlight" mode
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

- `test_search_highlight_mode_dims_not_hides` — in Highlight mode, non-matching nodes are present
  but dimmed.
- `test_search_filter_mode_hides_nodes` — in Filter mode, non-matching nodes are absent from
  render.
- Headed: toggle between modes during active search; correct behavior for both.

---

#### 3.4 Crashed Node Indicator

Servo's crash recovery is visible in the detail view tile (error overlay) but not in the graph
view. A node whose webview has crashed shows no distinct visual state; it looks identical to a
cold node. Research §7.1 identifies this as a missing state.

- Apply a red/orange tint (or colored ring) to nodes whose `webview_state == Crashed`.
- Should have lower visual weight than selection amber — a tint on the existing circle color is
  sufficient.
- Restore to normal color when the webview recovers or the node is navigated.

**Tasks**

- [x] Confirm `Node` or app-level state carries a `Crashed` lifecycle variant distinguishable from
  `Cold` (check `node.webview_state` or equivalent field).
- [x] In `GraphNodeShape` color projection: if crashed, apply red/orange tint.
- [x] Ensure crashed color does not override `Selected` (amber takes priority).

**Validation Tests**

- `test_crashed_node_color_differs_from_cold` — crashed node produces a different color than a
  cold node.
- Headed: crash a tab; corresponding graph node shows red/orange tint; recover tab → tint clears.

---

#### 3.5 Multi-Select Visual: Halo on All Selected Nodes

`Ctrl+Click` multi-select is implemented (edge plan Step 1), but only the primary selected node
shows its full selected color. Secondary selected nodes may not have a distinct visual indicator.
Research §7.1: "Distinct border or halo on all selected nodes, not just primary."

- Primary selected node: existing amber fill (unchanged).
- Secondary selected nodes (in `selected_nodes` set but not `primary()`): visible halo or border
  ring in the same amber, reduced opacity or stroke-only, to signal "part of the current
  selection" without displacing primary visual hierarchy.

**Tasks**

- [x] In `GraphNodeShape` color projection: distinguish primary vs. secondary selected nodes.
- [x] Apply a stroke-only ring (amber, stroke width ~2px) to secondary selected nodes.
- [x] Ensure secondary halo does not override hovered or dragged state colors.

**Validation Tests**

- `test_secondary_selected_color_differs_from_primary` — two selected nodes: primary → amber fill;
  secondary → different visual (stroke-only or reduced fill).
- Headed: Ctrl+Click two nodes → both visually indicated as selected with clear hierarchy.

---

### Phase 4: Multi-Select Extensions (in progress)

Rationale: `rstar` here is a UX interaction-performance improvement for lasso/hit-testing, not a layout algorithm change.

Workspace routing Phases 1–3 are complete. Group drag implemented via `sync_graph_positions_from_layout` — no Step 4d gate needed (sync-layer approach is independent of edge operations).

- **`Ctrl+A` select all**: emit `SelectAll` intent → populate `selected_nodes` with all `NodeKey`s.
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

## Findings

Research source: `2026-02-18_graph_ux_research_report.md`

Key section cross-references per phase:

- Phase 1: §6.2 (pinning workflow), §8.1 (keyboard zoom), §8.2 (zoom-to-selected)
- Phase 2: §7.3 (zoom-adaptive labels), §7.4 (hover tooltip), §7.5 (convergence indicator)
- Phase 3: §7.2 (edge differentiation), §7.6 (neighbor highlight), §9.1 (highlight vs. filter)
- Phase 4: §6.3 (multi-select extensions)

Research §11 priority table items tracked:

| Priority | Item | Location |
|---|---|---|
| #1 | `Ctrl+Click` multi-select | ✅ done (edge plan Step 1) |
| #2 | Pin node UX | complete (shortcut + docs + visual) |
| #3 | Physics presets | Not yet — no preset system exists (archived plan [x] marks are wrong) |
| #4 | Auto-pause on convergence | Layout Advanced Plan §1.1 |
| #5 | Reheat on structural change | Layout Advanced Plan §1.2 |
| #6 | Hover tooltip | Phase 2.1 |
| #7 | Keyboard zoom | Phase 1.1 |
| #8 | New-node placement near neighbors | Layout Advanced Plan §1.3 |
| #9 | Zoom to selected | Phase 1.2 |
| #10 | Edge type visual differentiation | Phase 3.1 |
| #11 | Zoom-adaptive labels | Phase 2.2 |
| #12 | Convergence status indicator | Phase 2.3 (see Layout Advanced Plan §1.1) |
| #13 | Neighbor highlight on hover | Phase 3.2 |
| #14 | Highlight vs. filter search toggle | Phase 3.3 |
| #15 | Crashed node indicator | Phase 3.4 |
| — | Multi-select halo (all selected nodes) | Phase 3.5 |
| #16-18 | Lasso, group drag, edge hit targets | Phase 4 (lasso ✅ done; group drag ✅ done; edge hit targets deferred) |

Research §14 advanced recommendations (degree-dependent repulsion, greedy label culling, invisible
layout constraints) are tracked in `2026-02-19_layout_advanced_plan.md §Phase 2`.

---

## Progress

### 2026-02-19 — Session 1

- Plan created from research report §11 priority table and §2–9 detail sections.
- Phases 1–3 have full task lists and unit test stubs.
- Phase 4 deferred pending pair-operation and workspace routing stability.
- Implementation not started.

### 2026-02-19 — Session 2

- Physics micro-improvements (original Phase 1: auto-pause, reheat, new-node placement) moved to
  `2026-02-19_layout_advanced_plan.md §Phase 1` to consolidate layout-system changes.
- Remaining phases renumbered: old 2→1, old 3→2, old 4→3, old 5→4.

### 2026-02-19 — Session 3

- Implemented keyboard-grouped node context menu navigation (Left/Right group switch, Up/Down
  action cycle, Enter execute) with persistent focus state.
- Added Persistence Hub `Load Pin...` chooser popup with `Workspace Pin` and `Pane Pin` restore
  actions.
- Implemented pin UX polish items: `L` toggles primary-node pin state, help/overlay shortcut text
  updated, and pinned nodes now render a top-center marker.
- Reduced graph zoom speed to `0.01` for finer wheel/trackpad control.

### 2026-02-19 â€” Session 4

- Implemented Phase 1.1 keyboard zoom (`+`/`-`/`0`) end-to-end:
  input flags, intent mapping, app request queueing, and post-render `MetadataFrame` updates.
- Implemented Phase 1.2 `Z` zoom-to-selected:
  selected-node AABB fit with 20% padding, plus no-selection fallback to fit-to-screen.
- Updated graph overlay/help text with the new zoom shortcuts.
- Added app-level tests for keyboard zoom request queueing and zoom-to-selected fallback behavior.
- Follow-up adjustment: retired `C` keyboard fit shortcut; `Z` now owns smart-fit
  (2+ selected → fit selection, 0/1 selected → fit graph).

### 2026-02-19 â€” Session 5

- Implemented Phase 2.1 hover tooltip in `render/mod.rs` using hovered-node context.
- Tooltip now shows title/URL, relative last-visited time, and lifecycle state.
- Tooltip is rendered on a non-interactable tooltip layer and suppresses itself while hovering
  workspace-membership badges to avoid overlap.
- Added render-layer unit tests for relative-time formatting helpers.


### 2026-02-20 � Session 6

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






### 2026-02-20 � Session 7

- Implemented first-pass lasso multi-select in graph view with `Right+Drag` rectangle selection.
- Added bulk selection semantics via `SelectionUpdateMode` and `GraphIntent::UpdateSelection`
  (`Replace`, `Add`, `Toggle`) for future transform features.
- Wired modifier behavior: `Right+Drag` replaces selection, `Right+Ctrl+Drag` adds, and`r`n  `Right+Alt+Drag` toggles inside-lasso nodes.
- Added lasso rectangle overlay rendering and updated graph shortcut help text.
- Added unit coverage for bulk selection reducer behavior and lasso action intent mapping.



### 2026-02-20 � Session 8

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
