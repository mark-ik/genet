# Graph-Tile Unification Plan (2026-02-13, Refined 2026-02-14)

## Graph-Tile Unification Plan

- Goal: unify graph semantics and tile/workspace behavior without enforcing structural parity.
- Scope: UUID identity migration, Servo signal wiring, sync_to_graph replacement, mutation pipeline, pane/tab behavior.
- Dependencies: complete physics migration + selection consolidation first.
- Behavioral reference: [GRAPHSHELL_AS_BROWSER.md](../GRAPHSHELL_AS_BROWSER.md)
- Validation baseline: [2026-02-14_servo_architecture_constraints.md](../2026-02-14_servo_architecture_constraints.md)

### Architecture Premises (Validated)

1. Servo delegate callbacks are processed through `Servo::spin_event_loop()` dispatch flow.
2. `notify_url_changed` exists in Servo API but is not overridden in current Graphshell `RunningAppState`.
3. `request_create_new` is implemented in Graphshell and creates/activates a new WebView.
4. Current `sync_to_graph` polling path creates a new graph node on URL change; this is semantically wrong for same-tab navigation.
5. `notify_history_changed` is currently handled only as UI invalidation (`set_needs_update()`), not graph history update.

### Non-goals

- No assumption that graph and tile tree must be structurally identical.
- No unvalidated thread-model redesign.
- No Servo-core patch unless a proven API gap appears.

---

### Phase 1: Node Identity Migration (URL -> UUID)

**Problem**

Current graph identity is URL-index-driven (`url_to_node: HashMap<String, NodeKey>`), which blocks duplicate URL tabs and conflates identity with mutable location.

**Implementation**

1. Add stable `uuid::Uuid` field to `Node`.
2. Keep URL index as lookup aid, not identity source.
3. Change URL index to support duplicates (`HashMap<String, Vec<NodeKey>>` or equivalent).
4. Add UUID lookup index (`HashMap<Uuid, NodeKey>`).
5. Extend snapshot/log persistence to include UUID-based identity.
6. Do not add legacy snapshot migration paths; Graphshell is pre-user and UUID-only.

**Tests**

- Duplicate URL nodes coexist with distinct UUIDs.
- UUID survives snapshot roundtrip.
- URL lookup can return multiple candidates.
- Replay logic resolves by UUID, not URL.

---

### Phase 2: Servo Signal Wiring for Correct Navigation Semantics

**Problem**

Current behavior infers navigation from polling and URL diffs in `sync_to_graph`, creating structural mutations from observation.

**Target semantics**

1. `notify_url_changed`: update mapped node URL/history state in-place.
2. `request_create_new`: create new node + `Hyperlink` edge from parent mapping.
3. `notify_history_changed`: store/update node history metadata.

**Implementation**

1. Override `notify_url_changed` in `running_app_state.rs`.
2. Extend `notify_history_changed` handling beyond UI invalidation.
3. Extend `request_create_new` flow to emit graph-meaningful operation (directly or via mutation queue).
4. Keep title/load/focus/favicon handlers as they are unless needed for consistency.

**Important correction**

- Do not rely on the incorrect assumption that delegate callbacks run on a compositor callback thread.
- Any queue/reducer added here is for deterministic ordering and conflict handling, not because cross-thread callback dispatch is mandatory.

**Tests**

- Same-tab URL change does not create a new node.
- New-tab action creates exactly one node and one `Hyperlink` edge.
- History callbacks update node history state.

---

### Phase 3: Replace sync_to_graph Structural Mutation

**Problem**

`sync_to_graph` currently handles too much and contains semantically wrong structural creation logic.

**Implementation**

1. Remove URL-change -> new-node creation path from `sync_to_graph`.
2. Retain only reconciliation duties that are still needed (stale mapping cleanup, active highlight sync, compatibility glue).
3. Move semantic structural mutations to signal-driven and/or reducer-driven paths.
4. Keep tile remap reconciliation only where required by current runtime behavior.

**Tests**

- No node creation from URL polling path.
- Cleanup logic still removes stale mappings.
- Existing tile remap and favicon ingestion flows remain correct.

---

### Phase 4: Mutation Pipeline (Deterministic Apply Boundary)

**Problem**

Graph, tile, keyboard, and Servo-sourced operations can conflict when applied ad-hoc in-frame.

**Implementation**

1. Introduce/extend unified mutation intents (`GraphIntent` or equivalent).
2. Collect operations from:
   - Servo delegate event handlers,
   - graph interactions,
   - tile/tab interactions,
   - keyboard actions.
3. Apply at one sync boundary in deterministic order.
4. Define conflict policy (for example: delete dominates metadata updates on same node in same batch).

**Notes**

- Transport mechanism (direct queue vs channel) should follow measured need and code simplicity.
- Deterministic mutation boundary is the requirement; specific transport is an implementation choice.

**Tests**

- Conflicting same-frame operations resolve deterministically.
- Any operation source can propagate updates to graph + tile + runtime mappings.
- Persistence logging reflects successfully applied mutations only.

---

### Phase 5: Pane Membership and Tab Behavior (Semantic Parity, Not Structural Parity)

**Model**

- Graph stores semantics and identity.
- Tile tree stores workspace layout and visibility.
- Runtime webviews are ephemeral instances.

**Rules**

1. Every tile node reference must resolve to an existing graph node.
2. Graph node may exist without a tile.
3. Layout-only operations (reorder, resize, move between panes) should not implicitly mutate graph semantics.
4. Semantic operations should be explicit (for example, explicit grouping command creates `UserGrouped`).

**Implementation**

1. Normalize lifecycle naming and semantics (`Active`, `Inactive`, `Closed` policy, with clear state transitions).
2. Update tab rendering to reflect lifecycle and focus state.
3. Keep close/delete semantics explicit in intent definitions (avoid implicit coupling).

**Tests**

- Tile references are pruned when graph nodes are deleted.
- Graph nodes can remain without tiles.
- Explicit semantic grouping creates expected edges; layout-only movement does not unless requested.

---

### Phase 6: Edge Semantics Review (`UserGrouped`)

**Status question to resolve first**

`ARCHITECTURAL_OVERVIEW.md` and this plan previously diverged on whether `UserGrouped` already exists. Resolve code truth first, then implement any missing piece.

**Implementation (if missing)**

1. Add `UserGrouped` to edge enum and persistence type.
2. Add rendering distinction in graph adapter.
3. Wire explicit creation command/gesture.

**Tests**

- Serialization roundtrip for `UserGrouped`.
- Distinct visual style.
- Explicit command creates edge.

---

### Validation Matrix

**Identity**

- [ ] Duplicate URL tabs with unique UUIDs.
- [ ] UUID-based persistence roundtrip.

**Navigation semantics**

- [ ] `notify_url_changed` updates URL in-place.
- [ ] `request_create_new` creates new node + edge.
- [ ] No same-tab node inflation.

**Mutation discipline**

- [ ] Deterministic conflict resolution.
- [ ] No direct cross-subsystem mutation bypassing apply boundary.

**Graph/tile consistency**

- [ ] Tile node reference always valid.
- [ ] Graph node may exist without tile.
- [ ] Stale tile references pruned on restore/update.

---

### Research Questions (Before or During Implementation)

1. What is the minimal migration path that removes `sync_to_graph` semantic mutation without regressing current tile remap behavior?
2. What ordering guarantees are observed between `notify_url_changed`, `notify_page_title_changed`, and `notify_history_changed` under redirects and SPA transitions?
3. Do we need an explicit buffering queue for performance/ordering in practice, or is current event-loop serialization sufficient with a local intent batch?
4. What close-tab policy should be canonical for Graphshell phase scope: close-node, deactivate-node, or mode-dependent?

---

## Findings (Validated)

- `notify_url_changed` is not implemented in `ports/graphshell/running_app_state.rs`.
- `request_create_new` is implemented and creates/activates WebViews, but graph node creation still depends on polling-based sync path.
- `notify_history_changed` exists in Graphshell delegate impl but currently only invalidates UI.
- `sync_to_graph` in `ports/graphshell/desktop/webview_controller.rs` creates a new node on URL change.
- Current architecture documents contain stale contradictions (identity model, lifecycle naming, and `UserGrouped` status); this plan assumes code truth must drive cleanup.

## Progress

- 2026-02-13: initial unification plan drafted.
- 2026-02-14: refined to validated baseline; removed incorrect threading assumptions and converted unverified claims to explicit research questions.
