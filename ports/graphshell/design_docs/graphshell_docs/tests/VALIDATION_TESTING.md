# Graphshell: Incomplete Validation Tests

**Purpose**: Collect headed-manual and extended automated validation items that have not yet been executed or verified. Extracted from archived plan docs and ongoing plans so completed plans can be closed cleanly.

**Note**: Automated unit tests belong in their respective plan or source files. This file collects validation requiring headed execution or an extended integration harness.

---

## Workspace Routing and Membership (Headed Manual)

**Source**: `implementation_strategy/2026-02-19_workspace_routing_and_membership_plan.md`

**Context**: Items 7, 10, and 11 now have automated coverage. The remaining routing/membership
checks require headed-manual validation.

**Start command (PowerShell):**
```powershell
$env:RUST_LOG="graphshell=debug"; cargo run -p graphshell --bin graphshell -- -M https://example.com
```

**Baseline setup (once):**
1. Create at least 4 nodes (`A`, `B`, `C`, `D`) by opening distinct URLs.
2. Save workspace `workspace-alpha` containing `A` and `B`.
3. Save workspace `workspace-beta` containing `A` and `C`.
4. Save workspace `workspace-single` containing only `D`.
5. Return to a different layout so restore behavior is visible.

1. [x] **Single-membership routed open**
   - Action: double-click node `D`.
   - Expected: `workspace-single` restores directly; no synthesized fallback workspace behavior.
   - Run note (2026-02-19): Passed.

2. [x] **Multi-membership default recency + explicit chooser**
   - Action A: restore `workspace-beta`, then leave it; double-click node `A`.
   - Expected A: default routed open restores `workspace-beta` (most recent).
   - Action B: open node `A` context menu/radial and choose `Choose Workspace...`, select `workspace-alpha`.
   - Expected B: `workspace-alpha` restores.
   - Run note (2026-02-19): Passed (A and B).

3. [ ] **Zero-membership open remains in current workspace context**
   - Action: create a new node `E` and do not save any workspace that contains it; open `E`.
   - Expected: opens in current workspace context (tab open), no named workspace is created implicitly.
   - Run note (2026-02-19): Core behavior passed (no named workspace auto-created). Prompt gating behavior changed twice (sticky, then suppressed). Follow-up fixes landed: routed workspace-open no longer clears workspace prompt state before switch handling; `SetNodePosition` no longer flags unsaved state; session-autosave path no longer raises modal prompt (prompting is reserved for explicit workspace-switch flows). Re-validation reported no prompt at all in expected switch case; follow-up fix pending and requires headed re-validation.

4. [x] **Open with Neighbors synthesis cap**
   - Action: choose a hub node with many direct neighbors; run `Open with Neighbors`.
   - Expected: synthesized workspace contains hub + direct neighbors, capped at 12 opened tiles.
   - Run note (2026-02-19): Passed (12-tile cap observed).

5. [x] **Workspace delete removes routing target immediately**
   - Action: delete `workspace-beta`, then open node `A` via default route and via `Choose Workspace...`.
   - Expected: `workspace-beta` is never selected/returned; chooser does not list it.
   - Run note (2026-02-19): Passed.

6. [x] **Startup membership scan before first frame**
   - Action: restart app.
   - Expected: membership badges/tooltips (`N`) are correct immediately on first render, without needing a manual workspace switch.
   - Run note (2026-02-19): Passed.

7. [ ] **Empty-restore fallback warning path**
   - Action: trigger restore of a named workspace snapshot that prunes to empty after stale-key cleanup.
   - Expected: app logs fallback warning and opens target node in current workspace instead of failing.
   - Note: this case may require a stale-layout fixture or manual store edit to force an empty post-prune restore.
   - Run note (2026-02-19): Not yet reproduced.

8. [x] **Radial pair-edge parity with keyboard `G`**
   - Action: with a valid pair context, trigger pair edge creation from radial menu (`Edge -> Pair`) and compare with keyboard `G`.
   - Expected: radial path creates the same `UserGrouped` edge as keyboard command.
   - Run note (2026-02-19): Reported mismatch (`G` works, radial pair did not). Follow-up fixes landed: pair context honors explicit node context target; graph interactions are disabled while command menu is open; right-click now opens a compact node context menu (radial remains keyboard/F3 path), avoiding immediate close on right-button release. Re-validation passed via current context-menu path.

9. [x] **Node context menu hierarchy**
   - Action: right-click a node in graph view.
   - Expected: context menu presents grouped hierarchy (`Workspace`, `Edge`, `Node`) via submenu-style entries, and closes on `Esc` or action selection.
   - Keyboard: while open, `Left/Right` switches group, `Up/Down` moves action focus (including `Close`), `Enter` executes highlighted action or closes when `Close` is focused.
   - Run note (2026-02-19): Passed; follow-up polish applied to rely on arrow-key navigation only and include keyboard-selectable close.

10. [x] **Add node tab to existing workspace**
   - Action: right-click node `E` -> `Workspace` -> `Add To Workspace...` and pick `workspace-alpha`.
   - Expected: `workspace-alpha` snapshot now includes `E` as a tab on next restore/load, and `E` workspace badge count increments accordingly.
   - Run note (2026-02-19): Passed; wording/label clarity for workspace-route action remains a UX follow-up.

11. [ ] **Unified context-aware pin control**
   - Action A: with graph/workspace focus (no active pane focus), use Persistence Hub `Pin Workspace`; mutate layout and verify highlight toggles off; restore using `Load Pin... -> Workspace Pin` and verify previous layout restores.
   - Action B: with an active pane focus, use Persistence Hub `Pin Pane`; mutate layout and verify highlight toggles off; restore using `Load Pin... -> Pane Pin` and verify previous layout restores.
   - Expected: single pin control adapts to focus context (`Workspace` vs `Pane`) and renders active state only when current layout matches saved pin snapshot.
   - Run note (2026-02-19): Workspace pin flow passed. Pane pin restore semantics remain unclear in graph-only context, and pane pin persistence/visibility after view switching needs follow-up.

---

## Graph UX Polish: Phase 1.1 / 1.2 (Headed Manual)

**Source**: `implementation_strategy/2026-02-19_graph_ux_polish_plan.md`

**Context**: Automated coverage exists for input/intent/app semantics. Headed-manual validation is
required for end-to-end viewport behavior and interaction feel.

**Keyboard model update**:
- `C` keyboard fit shortcut is retired.
- `Z` is the single smart-fit key:
  - `2+` selected nodes: fit selected bounds.
  - `0` or `1` selected node: fit full graph.

1. [ ] **Keyboard zoom in/out/reset**
   - Action: in graph view (no text field focused), press `+` a few times, `-` a few times, then `0`.
   - Expected: zoom increases, decreases, then resets to `1.0x` in overlay.

2. [ ] **Keyboard zoom blocked during text entry**
   - Action: focus URL bar (`Ctrl+L`), then press `+`, `-`, `0`, `Z`.
   - Expected: graph zoom/fit does not trigger while text input owns keyboard focus.

3. [ ] **Smart-fit: zero selection**
   - Action: clear selection and press `Z`.
   - Expected: full-graph fit.

4. [ ] **Smart-fit: single selection**
   - Action: select exactly one node and press `Z`.
   - Expected: full-graph fit (not single-node framing).

5. [ ] **Smart-fit: multi-selection**
   - Action: select two or more distant nodes and press `Z`.
   - Expected: viewport fits selected-node AABB with padding.

6. [ ] **Wheel/trackpad zoom feel**
   - Action: use Ctrl+wheel or Ctrl+trackpad pinch/scroll.
   - Expected: zoom responsiveness feels slightly faster than previous `0.01` setting and remains controllable.

7. [ ] **Zoom bounds**
   - Action: repeatedly zoom out, then repeatedly zoom in.
   - Expected: zoom clamps at configured bounds; no runaway scale.

8. [ ] **Interaction regression sweep**
   - Action: after zoom/fit operations, drag/select nodes and open nodes from graph.
   - Expected: graph interactions remain stable (no stuck input state).

---

## Graph UX Polish: Carry-Over Manual Checks (Current)

**Source**: Active UX polish + follow-up implementation sessions

**Context**: These checks were identified during rapid implementation and need explicit headed
verification before closing the UX-polish tranche.

1. [ ] **Wheel vs trackpad tuning target**
   - Action: validate `Ctrl+wheel` and precision trackpad pinch/scroll on at least one wheel mouse and one trackpad.
   - Expected: zoom feels controllable on both devices with no over/under-shoot bias.

2. [ ] **Tab-click warm -> active routing**
   - Action: from workspace tabs, click a warm node tab and then navigate.
   - Expected: node reliably promotes to active and page loads in the expected pane context.

3. [ ] **Lasso reliability under dense node overlap**
   - Action: generate a dense graph cluster and perform repeated right-drag lasso sweeps.
   - Expected: lasso selection set is deterministic and matches visual rectangle containment.

4. [ ] **Omnibar non-`@` ordering policy**
   - Action: test non-`@` query flow for: local tabs present, connected matches present, provider fallback, and global fallback.
   - Expected: ordering follows configured settings (scope + non-`@` order preset) and non-local caps are respected.

5. [ ] **Omnibar provider failure-state visibility**
   - Action: disable network or force provider error responses while typing non-`@` and `@g/@b/@d` queries.
   - Expected: dropdown surfaces clear provider status (loading / error) without blocking input or crashing.

---

## Warning/Stub Revival Watchlist

**Source**: `cargo check -p graphshell --lib` warning sweep + code scan (2026-02-20)

**Context**: Keep these visible so transitional code paths do not silently rot. Prioritize items
that indicate dormant feature producers/consumers and integration seams that may need revival.

1. [ ] **Test-only helpers leaking into non-test builds**
   - Files: `ports/graphshell/desktop/gui.rs`, `ports/graphshell/desktop/persistence_ops.rs`
   - Signal: unused import/function warnings for test wrappers and helpers.
   - Validation expectation: move wrappers/imports behind `#[cfg(test)]` and confirm no behavior change.

2. [ ] **Dormant intent producers vs handled intent variants**
   - File: `ports/graphshell/app.rs`
   - Variants: `CreateUserGroupedEdgeFromPrimarySelection`, `WebViewScrollChanged`, `SetNodeFormDraft`
   - Signal: variants are handled but currently not constructed in lib build.
   - Validation expectation: either wire active producers in runtime flows or explicitly mark as deferred bridge paths in plan/docs.

3. [ ] **Potentially stale GUI bridge helpers**
   - File: `ports/graphshell/desktop/gui.rs`
   - Symbols: `active_tile_webview_id`, semantic-event conversion helper wrappers.
   - Signal: dead-code warnings suggest old integration seams.
   - Validation expectation: remove if obsolete, or reattach to intended flow and verify usage under headed run.

4. [ ] **Scaffold marker still present in tile path**
   - File: `ports/graphshell/desktop/tile_kind.rs`
   - Signal: explicit scaffold/dead-code allowance comment.
   - Validation expectation: confirm this is still intentional in current phase, or retire the allowance when tile flow is fully active.

5. [ ] **Graph model dead fields/methods that may reflect deferred features**
   - Files: `ports/graphshell/graph/mod.rs`, `ports/graphshell/graph/egui_adapter.rs`
   - Symbols: `Node.velocity`, `get_node_by_id`, `has_edge_between`, `EguiGraphState::from_graph`.
   - Signal: dead-code warnings; may indicate partially implemented or superseded paths.
   - Validation expectation: decide revive/remove/annotate; if revive, add explicit validation scenarios tied to the owning plan.

6. [ ] **Recovery TODO/FIXME that can impact UX and correctness**
   - Files: `ports/graphshell/desktop/dialog.rs`, `ports/graphshell/desktop/headed_window.rs`, `ports/graphshell/panic_hook.rs`, `ports/graphshell/lib.rs`, `ports/graphshell/prefs.rs`
   - Signal: active TODO/FIXME markers in runtime code.
   - Validation expectation: each marker either receives a tracked owner/plan link or a scoped deferral note with revalidation trigger.

---

## Navigation: Back/Forward Delegate Event Ordering

**Source**: `delegate_trace_back_forward_burst_http.log` (captured 2026-02-17)

**Finding**: During back/forward navigation, Servo fires `url_changed` *before* `history_changed`
reflects the new stack position — and the URL in `url_changed` is the **source URL**, not the
destination. Trace evidence (all events at t_ms=131, same frame):

```text
seq=9:  url_changed → ?step=2           (pushState arrives)
seq=10: history_changed len=3 current=2 (stack confirmed at step=2)
seq=11: url_changed → ?step=2           ← spurious? fires before back completes
seq=12: history_changed len=3 current=1 ← back navigation; now at step=1, but url above said step=2
seq=13: url_changed → ?step=2           ← fires before forward completes
seq=14: history_changed len=3 current=2 ← forward; back at step=2
```

Going back from `?step=2` to `?step=1`: `url_changed(?step=2)` fires first, then
`history_changed(current=1)`. `sync_webviews_to_graph()` reads `url_changed` to detect new
navigations and create nodes/edges — it will see `?step=2` as an apparent navigation event even
though the back transition is to `?step=1`.

**Risk**: spurious node creation or misclassified edge type (`Hyperlink` instead of `History`)
for back/forward transitions that emit `url_changed` for a URL that already exists.

1. [ ] **No spurious node on back navigation**: browse to A → B → C, go back to B — confirm only
   nodes A, B, C exist (no duplicate C created during the back transition's `url_changed`).
2. [ ] **Back/forward edge type**: back/forward transitions are classified as `History` edges, not
   `Hyperlink` edges, even when `url_changed` fires with the source URL before `history_changed`.
3. [ ] **Burst scenario — node count invariant**: run `scenario_back_forward_burst.html`
   (pushState ×3, back ×1, forward ×1) — confirm final node count is 4 (base + step=0,1,2),
   not 5 or 6 from spurious url_changed events.
4. [ ] **Burst scenario — no duplicate edges**: same run — confirm no duplicate `History` or
   `Hyperlink` edges exist between the burst nodes after the sequence completes.

---

## F1: Multi-Pane Grouping Behavior

**Source**: `archive_docs/checkpoint_2026-02-19/2026-02-17_f1_multi_pane_validation_checklist.md`

**Context**: F1 automated tests pass; headed split/grouping trigger validation not yet recorded.

1. [ ] **Split trigger creates edge**
   - Select node A, then `Shift+Double-click` node B.
   - Expected: exactly one `UserGrouped` edge A→B is created.

2. [ ] **Drag-into-same-tab-group trigger**
   - Open nodes A and B in separate detail panes.
   - Drag one pane into the same `Tabs` container as the other.
   - Expected: exactly one `UserGrouped` edge created (no duplicates).

3. [ ] **Explicit group-with-focused action**
   - Focus node A (pane A active).
   - Run the "Group with Focused" command on node B (command palette or `G` key).
   - Expected: exactly one `UserGrouped` edge A→B created.

4. [ ] **No-trigger paths**
   - Switch pane focus, switch tabs, navigate URLs normally.
   - Expected: no new `UserGrouped` edges created from those actions alone.

---

## Step 4d: @ Omnibar Scope Validation

**Source**: `2026-02-18_edge_operations_and_cmd_palette_plan.md` (active plan)

**Context**: Scope implementation complete; headed validation not yet executed.

1. [ ] Graph mode, Enter cycling: type `@term`, press Enter repeatedly — active match cycles through all results and wraps at end.
2. [ ] Detail mode, multi-pane: type `@term`, press Enter — each press focuses/opens the matched node in the correct pane/tab context.
3. [ ] `@t term` in detail mode: only currently open tab/pane-backed nodes are cycled.
4. [ ] `@T term` in detail mode: active and saved workspace tab matches cycled deterministically.
5. [ ] `@n term` in graph mode: only active-graph-context matches are cycled.
6. [ ] `@N term` in graph mode: active + saved graph matches cycled deterministically.
7. [ ] `@e term` in graph mode: only active-graph searchable edge matches cycled and selected.
8. [ ] `@E term` in graph mode: active + saved-graph searchable edge matches cycled deterministically.
9. [ ] Clear query after cycling: counter/session reset, normal URL submit behavior resumes.
10. [ ] Mode switch during active query: graph ↔ detail — no panic, no stale focus target, no incorrect pane navigation.

---

## F6: EGL Explicit Targeting — Extended Tests

**Source**: `archive_docs/checkpoint_2026-02-19/2026-02-18_f6_explicit_targeting_plan.md`

**Context**: Exit criteria met (two focused unit tests pass). Optional extended harness tests not yet written.

1. [ ] **End-to-end wrapper dispatch**: call `load_uri_for_webview`, `go_back_for_webview`, and at least two input `_for_webview` methods from a simulated host-caller harness. Verify: correct webview receives each command, no fallback warning logged, other active webview unaffected.
2. [ ] **Multi-webview isolation**: with two active webviews, route input events to each via `_for_webview`. Confirm no cross-contamination between webviews.
3. [ ] **Fallback warning rate-limit**: configure `preferred_input_webview_id` to return `None`. Confirm deprecation warning emitted once and rate-limited on repeated calls within the cooldown window.

---

## Undo/Redo Validation

**Source**: Spec in `2026-02-18_edge_operations_and_cmd_palette_plan.md` §Global Undo/Redo Boundary

**Context**: Undo/redo is functional but has no dedicated plan doc, no feature entry in IMPLEMENTATION_ROADMAP, and no recorded validation against the grouping/exclusion rules in the spec. A feature entry and these tests should be added when the roadmap is next updated.

1. [ ] **Single graph mutation**: add a node → undo → node is removed; redo → node reappears.
2. [ ] **Multi-intent command as one undo step**: run `ConnectBothDirections` (creates two edges) → undo → both edges removed in one step (not two separate undos).
3. [ ] **Workspace/graph restore is atomic**: load a named workspace snapshot → undo → prior workspace layout is restored as one transaction.
4. [ ] **Webview navigation is not undoable**: navigate a webview to a new URL via link click → undo → URL change is NOT reverted (only explicit user commands are undoable).
5. [ ] **Physics simulation is not undoable**: run physics until nodes settle → undo → node positions are NOT reverted; only explicit layout commands (e.g. `Fit`) are undoable.
6. [ ] **Undo survives mode switch**: perform an action in graph mode → switch to detail mode → undo → action is reversed (undo stack persists across view modes).
7. [ ] **Persistence parity after undo/redo cycle**: create `UserGrouped` edge → undo removes it → redo re-adds it → restart app → edge state reflects the final post-redo state in the persisted log.

---

## Navigation Control-Plane Regression Sweep

**Source**: `archive_docs/checkpoint_2026-02-16/2026-02-15_navigation_control_plane_plan.md` (§Required Validation Artifacts)

**Context**: Archived plan called for deterministic integration validation around targeted navigation.
These checks remain relevant for current omnibar + tile-focused routing behavior.

1. [ ] **Omnibar URL submit in graph mode**: submit a URL from graph mode and verify deterministic target behavior (new/opened node, correct selection/focus outcome).
2. [ ] **Omnibar URL submit in detail mode**: with two panes, focus pane A then pane B and submit distinct URLs; verify each submit targets the focused pane only.
3. [ ] **`@query` no-match feedback**: enter a query with no matches and verify UI feedback is explicit, stable, and does not trigger unintended navigation.
4. [ ] **Back/Forward/Reload target isolation**: with distinct histories in two panes, invoke controls and verify only the focused pane is affected.
5. [ ] **Mode and focus regression pass**: switch graph/detail modes, switch active tiles/tabs, and repeat submit/navigation actions; verify no stale target leakage.

---

## Physics and Selection Behavior (Legacy Carry-Over)

**Source**: `archive_docs/checkpoint_2026-02-16/2026-02-12_physics_selection_plan.md` (§Validation Tests)

**Context**: These behaviors still exist and are user-visible; archived validation list remains useful as manual/integration guardrails.

1. [ ] **Physics toggle responsiveness**: `T` key reliably pauses/resumes layout updates with no stuck intermediate state.
2. [ ] **Physics panel parameter effect**: changing force parameters in panel produces observable layout response without instability/panic.
3. [ ] **Pinned-node invariance under motion**: pinned nodes remain fixed while surrounding unpinned nodes continue to move.
4. [ ] **Selection durability through interactions**: selection state remains coherent across drag, focus changes, and node delete operations.
5. [ ] **Mid-size convergence sanity**: graph with ~50 nodes reaches a visually stable layout in a reasonable interval on baseline hardware.
