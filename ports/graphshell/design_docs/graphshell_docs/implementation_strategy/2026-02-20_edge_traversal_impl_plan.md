<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Edge Traversal Model — Implementation Plan (2026-02-20)

**Status**: Draft — implementation not started.

---

## Plan

### Context

The edge traversal model research report (`2026-02-20_edge_traversal_model_research.md`) concluded
that all seven hypotheses are supported by the code survey. Repeat navigations and timing data are
currently discarded; `EdgeType` conflates trigger with structure; the `Hyperlink` edge fires on
webview spawn rather than URL commit. The traversal model fixes all of these.

This plan translates the research findings into three sequential phases:

1. **Phase 1 — Core PoC** (steps 1–8 from research §5): swap `EdgeType` → `EdgePayload`,
   wire `push_traversal`, add `AppendTraversal` log entry, display-layer deduplication, History
   Manager stub.
2. **Phase 2 — History Manager** (full UI and tiered storage): traversal archive, dissolution
   transfer, node history archive, export.
3. **Phase 3 — Temporal Navigation**: timeline scrubber using the WAL + traversal log.

Each phase is independently shippable. Phase 1 validates the data model before Phase 2 adds the
storage and UI complexity.

---

### Scope Absorption from Graph UX Polish Phase 5

The following Graph UX polish items are now coupled to traversal/history semantics and should be
implemented here (or in follow-on docs rooted here), not as standalone polish tasks:

1. Neighborhood focus/filter behavior that depends on true traversal topology and temporal context.
2. Edge-type/history filter semantics once `EdgePayload` replaces `EdgeType`.
3. Faceted search dimensions that include traversal-aware metadata (recency/count/trigger-aware
   views) instead of only static node fields.
4. DOI/relevance weighting that uses traversal frequency/recency as first-class inputs.

Graph UX polish retains non-traversal remnants (`Shift+Click` range-select decision, `R` manual
reheat, headed validation polish), while traversal-dependent filtering/searching is owned here.

---

### Phase 1: Core PoC

**Goal:** `EdgeType` eliminated, traversal accumulation working, History Manager stub visible in
UI. All 137 existing tests pass after migration (updated for the new types).

#### 1.1 Swap `EdgeType` → `EdgePayload`

**New types** (add to `graph/mod.rs`):

```rust
#[derive(Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq))]
pub struct Traversal {
    pub from_url: String,
    pub to_url:   String,
    pub timestamp: u64,   // Unix ms
    pub trigger:   NavigationTrigger,
}

#[derive(Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq))]
pub enum NavigationTrigger {
    ClickedLink,
    TypedUrl,
    GraphOpen,
    HistoryBack,
    HistoryForward,
    DraggedLink,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq))]
pub struct EdgePayload {
    pub user_asserted:          bool,
    pub traversals:             Vec<Traversal>,
    pub archived_traversal_count_ab: u64,  // A→B cold-tier count
    pub archived_traversal_count_ba: u64,  // B→A cold-tier count
}

impl EdgePayload {
    pub fn user_asserted() -> Self {
        Self { user_asserted: true, traversals: vec![], archived_traversal_count_ab: 0,
               archived_traversal_count_ba: 0 }
    }
    pub fn from_traversal(t: Traversal) -> Self {
        Self { user_asserted: false, traversals: vec![t], archived_traversal_count_ab: 0,
               archived_traversal_count_ba: 0 }
    }
    pub fn is_live(&self) -> bool {
        self.user_asserted || !self.traversals.is_empty()
            || self.archived_traversal_count_ab > 0 || self.archived_traversal_count_ba > 0
    }
}
```

Replace `StableGraph<Node, EdgeType, Directed>` with `StableGraph<Node, EdgePayload, Directed>`.

**Tasks**

- [ ] Add `Traversal`, `NavigationTrigger`, `EdgePayload` to `graph/mod.rs`.
- [ ] Change `inner: StableGraph<Node, EdgeType>` → `StableGraph<Node, EdgePayload>`.
- [ ] Update the 5–6 `add_edge` callsites in `graph/mod.rs` and `app.rs` to construct
  `EdgePayload` from the appropriate initial state.
- [ ] Remove the `EdgeType` enum (or alias it `#[deprecated]` for a single commit to make the
  diff reviewable, then remove in the next commit).

**Validation Tests**

- `test_edge_payload_is_live_user_asserted` — `user_asserted = true`, empty traversals → `is_live()`.
- `test_edge_payload_is_live_with_traversals` — traversals non-empty → `is_live()`.
- `test_edge_payload_not_live_empty` — all fields zero/empty → `!is_live()`.
- Update all 22 existing graph tests for `EdgePayload`.

---

#### 1.2 Replace `add_history_traversal_edge` with `push_traversal`

Remove the `!has_history_edge` guard. Every forward navigation between two known nodes appends a
`Traversal` to the edge payload.

```rust
/// Called from WebViewUrlChanged when navigation crosses to a different known node.
/// Inter/intra decision rule:
///   - prior_url must resolve to a NodeKey (otherwise: intra-node, skip)
///   - new_url must resolve to a different NodeKey (otherwise: intra-node, skip)
///   - prior_key == new_key (self-loop): skip
fn push_traversal(
    &mut self,
    prior_url: &str,
    new_url: &str,
    trigger: NavigationTrigger,
) {
    let from_key = self.graph.get_node_by_url(prior_url).map(|(k, _)| k);
    let to_key   = self.graph.get_node_by_url(new_url).map(|(k, _)| k);
    let (from_key, to_key) = match (from_key, to_key) {
        (Some(f), Some(t)) if f != t => (f, t),
        _ => return,  // intra-node or unknown destination
    };
    let t = Traversal {
        from_url:  prior_url.to_owned(),
        to_url:    new_url.to_owned(),
        timestamp: now_unix_ms(),
        trigger,
    };
    if let Some(edge) = self.graph.get_edge_mut(from_key, to_key) {
        edge.traversals.push(t);
    } else {
        self.graph.add_edge(from_key, to_key, EdgePayload::from_traversal(t));
    }
}
```

**URL capture ordering** (critical): `push_traversal` must be called with `prior_url` captured
*before* `update_node_url_and_log` overwrites the node's URL. In `apply_intent(WebViewUrlChanged)`:

```rust
let prior_url = self.graph.get_node(node_key).map(|n| n.url.clone());
self.update_node_url_and_log(...);
if let Some(prior) = prior_url {
    self.push_traversal(&prior, &new_url, trigger);
}
```

**Tasks**

- [ ] Add `push_traversal()` to `GraphBrowserApp`.
- [ ] Wire from `WebViewUrlChanged` intent with URL capture ordering guard.
- [ ] Remove `add_history_traversal_edge` and the `maybe_add_history_traversal_edge` call site.
- [ ] Keep `HistoryBack` / `HistoryForward` variants populated from the existing history-index
  change path; they now call `push_traversal` with the appropriate trigger rather than
  `add_history_traversal_edge`.
- [ ] Add `get_edge_mut(from: NodeKey, to: NodeKey) -> Option<&mut EdgePayload>` to `Graph` API.

**Validation Tests**

- `test_push_traversal_appends_to_existing_edge` — push twice A→B → edge has 2 traversals.
- `test_push_traversal_creates_edge_if_absent` — first push A→B → edge created with 1 traversal.
- `test_push_traversal_skips_self_loop` — prior_key == new_key → no edge created.
- `test_push_traversal_skips_unknown_destination` — new_url not in graph → no change.
- `test_push_traversal_url_capture_ordering` — `update_node_url_and_log` is called after prior_url
  is captured: prior_url correctly reflects the old URL.

---

#### 1.3 Add `AppendTraversal` to `LogEntry`

```rust
// persistence/mod.rs
pub enum LogEntry {
    AddNode { ... },
    AddEdge { ... },
    UpdateNodeTitle { ... },
    PinNode { ... },
    RemoveNode { ... },
    ClearGraph,
    UpdateNodeUrl { ... },
    // New:
    AppendTraversal { from_url: String, to_url: String, timestamp: u64, trigger: NavigationTrigger },
    AssertEdge { from_url: String, to_url: String },        // UserGrouped
    RetractEdge { from_url: String, to_url: String },       // UserGrouped retraction
}
```

In `push_traversal`, after appending to the in-memory edge, append an `AppendTraversal` entry to
the WAL. In WAL replay, `AppendTraversal` calls `push_traversal` (bypassing the URL-capture issue
since replay does not fire `WebViewUrlChanged`).

**Tasks**

- [ ] Add `AppendTraversal`, `AssertEdge`, `RetractEdge` variants to `LogEntry`.
- [ ] Implement `AppendTraversal` WAL write in `push_traversal`.
- [ ] Implement `AppendTraversal` WAL replay in `apply_log_entry`.
- [ ] Implement `AssertEdge` WAL write when `UserGrouped` edge is created via `G` key or context menu.
- [ ] Implement `RetractEdge` WAL write when `UserGrouped` is explicitly removed.
- [ ] Update 19 persistence tests for expanded `LogEntry` enum.

**Validation Tests**

- `test_append_traversal_log_entry_replays_correctly` — write `AppendTraversal` log entry, replay
  from scratch → edge has the traversal.
- `test_assert_edge_log_entry_creates_user_asserted_edge` — replay `AssertEdge` → `user_asserted = true`.
- `test_retract_edge_clears_user_asserted` — replay `AssertEdge` then `RetractEdge` →
  `user_asserted = false`.

---

#### 1.4 Display-Layer Deduplication

In `EguiGraphState::from_graph()`, when iterating petgraph edges, skip the reverse pair if already
processed. Show traversal count as stroke width; dominant direction as arrow.

**Dominant direction rule:**
- Count traversals where `from_url` matches the petgraph edge direction vs. the reverse.
- If one direction exceeds 60% of total → arrow points that way.
- Equal or below threshold → bidirectional arrows (or no arrow).
- `user_asserted` edges with zero traversals → no arrow (undirected).

**Tasks**

- [ ] In `EguiGraphState::from_graph()`: build a `HashSet<(NodeKey, NodeKey)>` of processed pairs;
  skip reversed pairs.
- [ ] Compute `traversal_count_ab` and `traversal_count_ba` per logical edge.
- [ ] Map traversal count to stroke width (suggest: `1.0 + log(1 + count) * 0.5`, capped at 4.0).
- [ ] Apply dominant-direction arrow rendering using `NavigationTrigger`-agnostic directional counts.
- [ ] `GraphEdgeShape` (from UX polish Phase 3.1): replace the current `EdgeType`-based branch with
  an `EdgePayload`-based branch (`user_asserted` → amber thick; traversals.len() > 0 → solid thin
  weighted; archived only → dashed thin).

**Validation Tests**

- `test_display_dedup_skips_reverse_pair` — A→B and B→A edges → only one logical edge rendered.
- `test_dominant_direction_above_threshold` — 7 A→B traversals, 3 B→A → arrow points A→B.
- `test_dominant_direction_below_threshold` — 5 A→B, 5 B→A → bidirectional.
- `test_traversal_count_drives_stroke_width` — 1 traversal vs. 10 traversals → different stroke
  width values.

---

#### 1.5 Edge Inspection Stub

Clicking an edge in graph view shows a tooltip with traversal count, dominant direction ratio, and
most-recent timestamp. Full inspection UI is deferred.

**Tasks**

- [ ] In `render/mod.rs`: detect hovered edge (egui_graphs `hovered_edge()` or equivalent).
- [ ] Show egui tooltip on hover: "N traversals (A→B: x, B→A: y) | Last: <relative time>".
- [ ] For `user_asserted` edges with zero traversals: "User-asserted | No traversals recorded."

**Validation Tests**

- Headed: hover an edge → tooltip appears with correct counts and timestamp.

---

#### 1.6 History Manager Stub

A menu entry that opens a panel listing recent traversal records. No persistence, delete, or
export in the PoC — proves the data flows from `push_traversal` to the UI.

**Tasks**

- [ ] Add "History" entry to the toolbar or hamburger menu.
- [ ] Open a non-modal `egui::Window` ("Traversal History") listing the last 50 traversals from
  all edges, sorted by timestamp descending: `from_url → to_url | trigger | time`.
- [ ] Clicking a row pans the graph to the source node (emits `GraphIntent::FocusNode`).

**Validation Tests**

- Headed: navigate between two nodes; open History panel → the traversal appears in the list.

---

### Phase 2: History Manager

**Goal:** Full traversal archive UI with dissolution transfer, node history archive, tiered
hot/cold storage, auto-curation, and export.

#### 2.1 Tiered Hot/Cold Storage

**Hot tier**: `Vec<Traversal>` in `EdgePayload` — traversals within the last N days (default 90).
Serialized in the rkyv snapshot. Fast random access.

**Cold tier**: fjall keyspace `traversal_archive`, key format:
`<from_uuid_bytes><to_uuid_bytes><timestamp_be_u64>`.

At snapshot time, move traversals older than the hot-tier threshold to cold tier:
1. Collect traversals to archive from each edge's `traversals` vec.
2. Write all collected records to fjall `traversal_archive` in a single transaction.
3. **fsync the fjall transaction** (WAL ordering constraint).
4. Only after confirmed fsync: remove archived records from `traversals` vec and increment
   `archived_traversal_count_ab` / `_ba`.
5. Write updated snapshot.

**Recovery without snapshot**: range-count cold records per `(from_uuid, to_uuid)` pair to
restore `archived_traversal_count`.

**Tasks**

- [ ] Add `traversal_archive` keyspace to `PersistenceState`.
- [ ] Implement `archive_cold_traversals(edge, threshold_days)` at snapshot time.
- [ ] Implement `restore_archived_counts()` — startup recovery scan (only when snapshot absent).
- [ ] Add `hot_tier_days: u32` (default 90) to persistence config; expose in
  `graphshell://settings/persistence`.
- [ ] Write fjall-ordering integration test: verify `archived_traversal_count` == fjall record
  count after a simulated crash mid-archive (requires test harness that kills the process at an
  injected fault point).

**Validation Tests**

- `test_archive_pass_moves_old_traversals_to_fjall` — traversal older than threshold → removed
  from hot tier, present in fjall, `archived_traversal_count` incremented.
- `test_archive_pass_preserves_recent_traversals` — traversal within threshold → untouched in hot
  tier.
- `test_recovery_scan_restores_counts` — snapshot absent; fjall has 5 records for edge A→B →
  after scan, `archived_traversal_count_ab == 5`.

---

#### 2.2 Dissolution Transfer

When an edge is fully dissolved or a node is removed, transfer all records (hot and cold) to the
History Manager's `dissolved_archive` keyspace — keyed by `dissolved_at_timestamp_be` —  before
petgraph removal.

**Node removal ordering** (strict):

```
1. Enumerate all incident edges of the node.
2. For each incident edge: transfer hot-tier traversals + cold-tier fjall records to dissolved_archive.
3. fsync dissolved_archive writes.
4. Remove edges from petgraph.
5. Remove node from petgraph.
6. Write RemoveNode to WAL.
```

**Tasks**

- [ ] Implement `transfer_edge_to_dissolved_archive(from_key, to_key)`.
- [ ] In `apply_intent(RemoveNode)`: run dissolution transfer for all incident edges before
  calling petgraph `remove_node`.
- [ ] In History Manager UI: show dissolved edges in a separate "Dissolved" tab with
  `dissolved_at` timestamp.

**Validation Tests**

- `test_dissolve_edge_transfers_all_traversals` — edge with 3 hot traversals + 2 cold records →
  dissolved_archive has 5 records; petgraph edge is gone.
- `test_remove_node_transfers_incident_edges_first` — node with 2 incident edges removed →
  dissolved_archive contains traversals from both edges.

---

#### 2.3 History Manager UI

Full `graphshell://history` page (or egui panel — see settings architecture plan):

- **Timeline tab**: all traversals from all edges, sorted by timestamp. Paginated (50 per page).
  Columns: From, To, Trigger, Time. Clicking a row pans graph to source node.
- **Dissolved tab**: dissolved edges and nodes, with `dissolved_at` timestamp. Restore action
  re-creates the edge/node structure (with traversals) via a batch of `AddNode`/`AddEdge`/
  `AppendTraversal` log entries.
- **Delete**: individual record delete (removes from fjall `traversal_archive`). Bulk delete by
  date range.
- **Auto-curation**: configurable "delete records older than N days" cronjob (runs at startup if
  last run > 24h ago).
- **Export**: serialize selected records to JSON or CSV.

**Tasks**

- [ ] Implement History Manager panel with Timeline and Dissolved tabs.
- [ ] Implement restore-from-dissolved action.
- [ ] Implement per-record and bulk delete.
- [ ] Implement auto-curation with configurable retention window.
- [ ] Implement JSON/CSV export.

---

### Phase 3: Temporal Navigation

**Goal:** A timeline slider that scrubs the graph state to any past moment using the WAL. Past
states render with desaturated "ghost" colors; a "Return to present" button restores live state.

The WAL already has timestamped `LogEntry` records. With Phase 1 complete, `AppendTraversal`
entries are also logged. Replaying the WAL up to any timestamp gives a valid graph state at that
moment — including traversal counts, edge existence, and node presence. This is the mechanism.

#### 3.1 Timeline Index

At startup (or lazily on first timeline open), build a `Vec<(u64, LogEntryPosition)>` from the
WAL — a sorted list of `(timestamp_ms, byte_offset_in_wal)`. This is the timeline index.

For interactive scrubbing (drag slider), use the index to find the nearest WAL entry at the target
timestamp, then replay from the nearest snapshot before that timestamp.

**Tasks**

- [ ] Build timeline index from WAL at startup (amortized O(N) scan, N = WAL entries).
- [ ] Add `timeline_index: Vec<(u64, WalPosition)>` to `PersistenceState`.
- [ ] Implement `replay_to_timestamp(target_ms: u64) -> GraphBrowserApp` — loads nearest
  snapshot, replays forward through WAL entries up to `target_ms`, returns the resulting app state.

---

#### 3.2 Preview Mode

The timeline viewer operates on a *copy* of the graph state, not the live state. No persistence
writes, no webview lifecycle events, no navigation from the preview state.

```rust
pub struct TimelinePreviewState {
    pub graph: GraphBrowserApp,  // cloned from live at preview_at timestamp
    pub preview_at: u64,         // timestamp being previewed
}
```

**Tasks**

- [ ] Add `timeline_preview: Option<TimelinePreviewState>` to top-level app state.
- [ ] In `apply_intents()`: when `timeline_preview` is Some, skip all intent processing that
  would trigger persistence writes or webview lifecycle events.
- [ ] Render graph from `timeline_preview.graph` instead of live graph when preview is active.

---

#### 3.3 Timeline UI

A horizontal slider in the toolbar (visible only in graph view) spanning from the earliest WAL
timestamp to `now()`. Dragging it triggers `replay_to_timestamp` on slider-release (not on every
tick — replay is too expensive for per-frame updates).

**Visual:**

- Past state: all nodes and edges rendered at 40% opacity (ghost effect via `Color32` alpha).
- Edge stroke widths reflect traversal counts at the preview timestamp.
- Toolbar shows "Previewing: 2026-01-15 14:32" with a "Return to present" button.
- Node positions: the snapshot's recorded positions are used; physics is paused in preview.

**Tasks**

- [ ] Add timeline slider widget to graph toolbar; visible when `timeline_preview.is_some()` or
  on explicit toggle.
- [ ] On slider release: call `replay_to_timestamp(target_ms)`, store result in
  `timeline_preview`.
- [ ] Ghost rendering: apply 40% alpha modifier to all node/edge colors in preview mode.
- [ ] "Return to present" button: clear `timeline_preview`, resume live graph.
- [ ] Add `GraphIntent::OpenTimelinePreview`, `ScrubTimeline(u64)`, `CloseTimelinePreview`.

**Validation Tests**

- `test_replay_to_timestamp_produces_subset_of_full_graph` — replay to halfway point → only log
  entries before that timestamp are reflected in the result graph.
- `test_preview_mode_does_not_write_wal` — in preview mode, applying intents does not produce WAL
  entries.
- `test_close_timeline_preview_restores_live_state` — close preview → live graph is unchanged.
- Headed: drag timeline slider to a past date → graph shows nodes/edges that existed then;
  return to present → live graph restored.

---

## Findings

### Phase Dependency Graph

```
Phase 1 (PoC) — self-contained
    ↓
Phase 2 (History Manager) — requires Phase 1 EdgePayload and push_traversal
    ↓
Phase 3 (Temporal Navigation) — requires Phase 1 AppendTraversal log entries
                               — Phase 2 dissolved archive is useful but not required
```

Phase 3 can technically start from Phase 1 alone. Phase 2 (dissolution transfer) enriches the
timeline — a dissolved node re-appears at its dissolution timestamp — but the basic scrubber works
with Phase 1 only.

### Performance Notes

- `replay_to_timestamp()` is called on slider release, not on every frame. Typical WAL: 1,000–
  10,000 entries. Replay is O(entries) with fast fjall reads + rkyv deserialization. Expected
  latency: 50–200ms for a medium graph with 6 months of history. Acceptable for a slider-release
  event.
- The timeline index eliminates the need to scan the entire WAL to find the start point — only
  entries after the nearest snapshot need replaying.
- Ghost rendering is a simple `Color32` alpha multiply — no additional rendering passes.

### Research Cross-References

- Phase 1.1: §3.1 (EdgePayload types), §3.2 (NavigationTrigger)
- Phase 1.2: §4.6 (inter/intra decision rule, URL capture ordering, self-loop skip)
- Phase 1.3: §3.4 (persistence additions: AppendTraversal, AssertEdge, RetractEdge)
- Phase 1.4: §3.3 (display-layer deduplication, dominant direction >60% threshold)
- Phase 2.1: §4.3a (hot/cold tiered storage, WAL ordering, recovery reconciliation)
- Phase 2.2: §3.5 (History Manager, dissolution transfer, node removal ordering)
- Phase 2.3: §3.5 (timeline O(n) limitation, auto-curation, export)

---

## Progress

### 2026-02-20 — Session 1

- Plan created from research report §5 (PoC scope), §3 (proposed model), §4 (technical viability),
  §7 (cons/risks), and §9 (resolved decisions).
- Phase 3 (Temporal Navigation) added: derived from §15.1 of the UX research report; blocker is
  Phase 1 `AppendTraversal` log entries (basic structural time travel works from existing WAL
  even without Phase 1 complete, but edge weight history requires it).
- Implementation not started.
