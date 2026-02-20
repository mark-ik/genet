# Edge Traversal Model — Research Report

**Date:** 2026-02-20
**Status:** Research / Pre-implementation

---

## 1. Premise

The following claims are stated as hypotheses to prove or disprove through code analysis
and technical planning:

1. **Edges should be collections of traversals, not typed singletons.** Rather than a simple
   `EdgeType` enum, each edge between two nodes should accumulate a `Vec<Traversal>`, where each
   entry records a directed navigation event (from_url, to_url, timestamp, trigger). The edge is
   the grouping; traversal records are the data.

2. **`EdgeType::Hyperlink` is not a distinct edge kind — it is a traversal trigger.** Clicking a
   hyperlink on page A that leads to page B is a directed navigation event A→B, indistinguishable
   in kind from any other traversal. The fact that it originated from an anchor element is trigger
   metadata, not a structural property of the edge.

3. **`EdgeType::UserGrouped` becomes an assertion, not a type.** A user-asserted edge (`user_asserted:
   bool`) may have zero traversal records. It exists because the user declared the relationship. All
   three current edge types collapse into one `EdgePayload` struct.

4. **Edges are visually undirected; traversals are directed.** The visual graph shows one edge
   between A and B. The directed information (who navigated to whom, when) lives in traversal
   records. Petgraph remains a `Directed` graph — its structure does not change.

5. **Multiple petgraph edges between the same node pair are concatenated at the display layer.**
   Petgraph `EdgeIndex` keys, `out_neighbors`, `in_neighbors`, and all query semantics are
   preserved unchanged. The "undirected logical edge" is a rendering abstraction above petgraph,
   not a schema change.

6. **Intra-node navigation (within a tab's context) belongs to the node, not to edges.** A node
   already tracks `history_entries: Vec<String>` (its tab's back/forward stack). This is the
   correct home for intra-node traversal data — version-control semantics for the tab's URL
   history. Only cross-node navigations create edge traversal records.

7. **The current model loses traversal frequency and timing.** Repeat navigations between A and B
   are silently discarded after the first (`!has_history_edge` guard). No timestamp is stored on
   edges. This is a data loss the traversal model would fix.

---

## 2. Current State (Code Survey)

### 2.1 EdgeType

```rust
// graph/mod.rs:104
pub enum EdgeType {
    Hyperlink,    // link opened a new webview from a parent
    History,      // navigation moved between two known nodes
    UserGrouped,  // user explicitly connected two nodes
}
```

`EdgeType` is a bare enum with no payload. The edge weight in petgraph is this enum value, nothing
more. No timestamp, no from/to URL snapshot, no count.

```rust
pub(crate) inner: StableGraph<Node, EdgeType, Directed>
```

### 2.2 Edge creation paths (app.rs)

**Hyperlink:** Created when a new webview spawns from a parent webview (`WebViewCreated` intent,
line 1117–1118). The edge represents "this tab opened from that tab" — not that A's HTML actually
links to B. If the user types a new URL into the spawned tab, the Hyperlink edge remains and is now
semantically wrong.

**History:** Created in `maybe_add_history_traversal_edge` (line 2493) when the browser history
index changes (back/forward navigation). The function resolves old/new URLs to node keys, then:

```rust
let has_history_edge = self.graph.edges().any(|edge| {
    edge.edge_type == EdgeType::History && edge.from == from_key && edge.to == to_key
});
if !has_history_edge {
    let _ = self.add_edge_and_sync(from_key, to_key, EdgeType::History);
}
```

**This is the key data loss:** subsequent traversals between the same pair are silently dropped.
Frequency, recency, and temporal ordering are irrecoverable.

**UserGrouped:** Created by explicit user action (`CreateUserGroupedEdge`). Also guarded by
`already_grouped` check — a second explicit grouping of the same pair is silently a no-op.

### 2.3 Node already has intra-node history

```rust
pub struct Node {
    pub url: String,
    pub title: String,
    pub position: Point2D<f32>,
    pub is_pinned: bool,
    pub last_visited: std::time::SystemTime,
    pub history_entries: Vec<String>,   // tab's back/forward URL stack
    pub history_index: usize,           // current position in that stack
    ...
}
```

`history_entries` is already the intra-node version-control history — the tab's URL stack as
reported by the browser engine. This directly supports Hypothesis 6: intra-node navigation already
has a home. The proposal would deepen it (add timestamps per entry) but not invent a new concept.

### 2.4 Persistence schema

```rust
pub enum LogEntry {
    AddEdge    { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType },
    RemoveEdge { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType },
    // ... node variants ...
}
```

No timestamp on `AddEdge`. No traversal count. Removing an edge removes the entire relationship —
there is no "remove one traversal" granularity.

---

## 3. Proposed Model

### 3.1 Core types

```rust
/// A single directed navigation event between two known nodes.
struct Traversal {
    from_url: String,   // URL of the source at time of navigation (snapshot — node URL may drift)
    to_url: String,     // URL of the destination at time of navigation
    timestamp: u64,     // Unix ms
    trigger: NavigationTrigger,
}

enum NavigationTrigger {
    ClickedLink,        // followed an anchor href
    TypedUrl,           // URL bar commit
    GraphOpen,          // opened from graph UI (node double-click, etc.)
    HistoryBack,
    HistoryForward,
    DraggedLink,        // future: dragged a link from one node onto another tab
    Unknown,            // Servo did not expose the navigation cause; stored but not filterable
}

/// Replaces EdgeType. Stored as petgraph edge weight.
struct EdgePayload {
    user_asserted: bool,            // true = UserGrouped equivalent
    traversals: Vec<Traversal>,     // hot tier: recent traversals (last N days), in RAM
    archived_traversal_count: u64,  // count of cold-tier traversals for THIS directed edge in fjall
                                    // (per-direction — A→B and B→A edges each have their own count)
}
```

Edge exists in the graph when `user_asserted || !traversals.is_empty() || archived_traversal_count > 0`.

Note: `archived_traversal_count > 0` must be included — after a hot→cold archive pass, `traversals`
may be empty while cold records still exist. Omitting this would silently dissolve edges with
archived history.

### 3.2 Petgraph stays Directed and unchanged

`StableGraph<Node, EdgePayload, Directed>` — only the weight type changes, not the graph type.
`EdgeKey = EdgeIndex` is unchanged. All neighbor queries work identically.

Navigating A→B appends a traversal to the `EdgePayload` on petgraph edge A→B. Navigating B→A
appends to petgraph edge B→A (or creates it). The display layer merges both into one visual edge.

### 3.3 Display-layer merging and dominant direction (egui_adapter)

When building the egui_graphs visual graph, the adapter iterates petgraph edges. For each undirected
pair (A, B), it finds all petgraph edges in both directions and renders one visual edge.

The arrow direction on the visual edge reflects the **dominant traversal direction** — whichever
directed edge has more traversal records:

```
ab_count = edge_a_to_b.traversals.len() + edge_a_to_b.archived_traversal_count
ba_count = edge_b_to_a.traversals.len() + edge_b_to_a.archived_traversal_count

if ab_count > ba_count  → arrow points A→B
if ba_count > ab_count  → arrow points B→A
if equal                → bidirectional arrow (or no arrow)
```

This is computed at state-sync time (when `egui_state_dirty = true`), not per frame. The directed
information (exact from_url, to_url per event) is fully preserved in traversal records regardless
of which visual direction is shown. The arrow conveys behavioral tendency; the records convey fact.

Total visual weight (stroke width) is derived from the combined count across both directions:
`ab_count + ba_count`. Traversal records from both directions are available for the inspection UI.

### 3.4 Node history deepened (not redesigned)

`history_entries: Vec<String>` becomes `history_entries: Vec<NodeHistoryEntry>`:

```rust
struct NodeHistoryEntry {
    url: String,
    timestamp: u64,  // Unix ms — when this URL became current in this tab
}
```

The back/forward stack semantics are unchanged; entries gain timestamps. This enables "what was
this tab browsing at 14:30?" queries without touching edges.

**Reconciliation on browser history events:** The browser engine reports a full history list on
each `WebViewHistoryChanged` event, replacing the prior list. When merging into
`Vec<NodeHistoryEntry>`, timestamps from matching URLs in the prior list must be preserved — only
genuinely new URLs get fresh timestamps. A URL present at index N in both old and new lists keeps
its prior timestamp; a URL appearing for the first time gets `now()`. This prevents re-stamping
the entire list on every history event.

### 3.5 History Manager

The History Manager is a UI surface and data layer that provides user-visible, user-controllable
access to the full traversal archive — including records from dissolved edges and deleted nodes that
would otherwise be inaccessible or silently orphaned.

**The problem it solves:** When an edge is dissolved (via `RemoveNode`, `ClearGraph`, or explicit
user removal), its cold-tier records in fjall have no live edge to attach to. Without the History
Manager, these records accumulate invisibly and are never accessible. With it, dissolution is an
explicit transfer rather than an implicit orphan.

**Data model:** The `traversal_archive` fjall keyspace is unified — it holds both:
- **Live cold records:** cold-tier traversals from edges that still exist in the graph
- **Dissolved records:** traversals from edges that have been dissolved

Each value in the keyspace carries a status tag:
```
status: Live | Dissolved { dissolved_at: u64, reason: DissolutionReason }

enum DissolutionReason {
    NodeRemoved,
    GraphCleared,
    UserRetracted,   // user explicitly removed the edge
}
```

Key format is unchanged: `from_uuid (16 bytes) | to_uuid (16 bytes) | timestamp_be (8 bytes)`.
Dissolved records are distinguishable by their value's status field, not their key.

**Dissolution transfer:** When an edge is dissolved (for any reason), before removing it from
petgraph:
1. Move all hot-tier `traversals` to fjall `traversal_archive` with `status: Dissolved { ... }`
2. Mark any existing cold records for that edge as `Dissolved` (range update on the key prefix)
3. Remove the petgraph edge

**Node removal ordering:** When `RemoveNode` fires, petgraph automatically drops all incident
edges. The dissolution transfer must happen *before* `self.graph.inner.remove_node(key)` — after
that call the edge payloads are gone and the traversal records cannot be recovered. The node
removal handler must enumerate all incident edge payloads, transfer them, then call remove_node.

`ClearGraph` marks all records for all edges as `Dissolved { reason: GraphCleared }`.

**Node version control:** When a node is deleted, its `NodeHistoryEntry` records are also
transferred to the History Manager — serialized into a separate fjall keyspace
`node_history_archive` keyed by `node_uuid | timestamp_be`. These represent the tab's intra-node
browsing history, preserved even after the node is gone.

**User controls:**
- **Timeline view:** all traversal records sorted by timestamp, filterable by URL, domain, date
  range, or dissolution status
- **Restore:** re-create a dissolved edge if both endpoint nodes still exist (or re-add the node
  from its archived URL)
- **Delete individual records:** permanent removal from fjall
- **Auto-curation policy:** "automatically delete records older than N days" — this is destructive
  (unlike the hot→cold archive pass, which is lossless). Requires explicit user opt-in.
- **Export:** JSON or CSV of the full traversal history, suitable for external tools, LLM context
  building, or cross-device sync

**Timeline query efficiency:** The cold store key `from_uuid | to_uuid | timestamp_be` is
optimised for edge-scoped queries ("all traversals for A↔B") but requires a full keyspace scan
for the History Manager's global timeline view ("all records sorted by time"). For the PoC stub
this is acceptable. At scale, a secondary index keyspace `traversal_timeline` keyed by
`timestamp_be | from_uuid | to_uuid` would enable O(log n) timeline queries with no full scan.
This secondary index duplicates data but does not affect correctness — add it when the full scan
becomes measurably slow.

**Tokenization / portability:** The traversal archive is structurally well-suited to export as
semantic context: each record has URLs, timestamps, directionality, and trigger. Clustered by
time window (browsing sessions) or by domain, it can produce a compact, meaningful representation
of browsing behavior. This is a future-facing direction; the data model accommodates it without
requiring it now.

### 3.6 Persistence additions

```rust
pub enum LogEntry {
    // Existing — unchanged, remain for migration compatibility
    AddEdge    { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType },
    RemoveEdge { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType },

    // New
    AppendTraversal {
        from_node_id: Uuid,
        to_node_id: Uuid,
        from_url: String,
        to_url: String,
        timestamp: u64,
        trigger: PersistedNavigationTrigger,
    },
    AssertEdge {
        from_node_id: Uuid,
        to_node_id: Uuid,
    },
    RetractEdge {         // removes user_asserted flag; edge dissolves if no traversals remain
        from_node_id: Uuid,
        to_node_id: Uuid,
    },
}
```

Old `AddEdge`/`RemoveEdge` entries are replayed as before during recovery; new entries use the
traversal-aware variants. No migration of existing data required — additive schema extension.

---

## 4. Technical Viability Analysis

### 4.1 Petgraph compatibility — Low risk

Only `EdgeType` (the weight type `W` in `StableGraph<N, W, Ty>`) changes. The graph topology,
`NodeIndex`/`EdgeIndex` keys, all traversal methods (`edges()`, `find_edge()`, `neighbors()`) are
unaffected. The change is a weight type swap, not a graph restructure.

The one behavioral change: `remove_edges(from, to, edge_type)` in `graph/mod.rs` currently filters
by `edge_type`. With `EdgePayload`, removal semantics change:
- For user-asserted edges: clear `user_asserted = true`, dissolve if no traversals remain
- For traversal edges: edges dissolve when the last traversal is removed or the `user_asserted`
  flag is false and traversals empty

This is a small number of callsites (5–6 in app.rs).

### 4.2 Display-layer deduplication — Medium complexity

Currently egui_adapter's `EguiGraphState::from_graph` adds every petgraph edge as a separate egui
edge. With the new model, A→B and B→A petgraph edges must be deduplicated into one visual edge.

The deduplication pass is an O(n) scan of petgraph edges, building a `HashSet<(min_key, max_key)>`
to skip already-added pairs. This runs only when `egui_state_dirty = true`, not every frame.

The merged edge's visual weight (stroke width) is derived from the full count including archived:
`(ab.traversals.len() + ab.archived_traversal_count) + (ba.traversals.len() + ba.archived_traversal_count)`.
This is computed once at state-sync time, not per frame.

**Dominant direction threshold:** A simple majority (ab > ba) is too sensitive — with 3 A→B and
3 B→A traversals the result is "equal" and flips to bidirectional, which will be common for
back-and-forth pairs. A relative threshold is more stable: dominant direction requires >60% of
total traversals in one direction; otherwise bidirectional. This prevents the arrow from flickering
between states on early symmetric use.

**Concern:** egui_graphs currently assigns one `EdgeKey` per petgraph edge. With two petgraph edges
merged into one visual edge, the egui edge key must map back to both petgraph edges for inspection.
The adapter can store a `HashMap<EguiEdgeKey, Vec<EdgeKey>>` for this lookup.

### 4.3 Persistence growth — Low practical risk, managed by tiered storage

Traversal records are small text. A realistic estimate per record:

- `from_url`: 50–150 bytes
- `to_url`: 50–150 bytes
- `timestamp`: 8 bytes
- `trigger`: 1 byte

~200–300 bytes per record. 10,000 traversals across all edges ≈ 2–3 MB. This is negligible for
both RAM and disk. Unlimited inline storage is correct for the PoC — measure actual growth before
adding any policy.

The concern is not bytes but RAM residency and snapshot serialization time at extreme scale (years
of heavy use, hundreds of thousands of traversals). This is addressed by tiered storage rather than
truncation — nothing is ever deleted.

### 4.3a Tiered storage — hot/cold split

Traversal history is split into two tiers based on age:

**Hot tier** — recent traversals, inline in `EdgePayload.traversals: Vec<Traversal>`:
- Traversals from the last N days (default: 90 days, configurable)
- Loaded into RAM at startup with the graph snapshot
- Available immediately for visual weight, dominant-direction computation, and recency display
- Serialized into the rkyv snapshot; snapshot size stays bounded regardless of total history length

**Cold tier** — archived traversals, in fjall keyspace `traversal_archive`:
- Traversals older than the hot-tier threshold
- Never loaded into RAM automatically
- Fetched on demand via range scan when an edge is inspected
- Key format: `from_uuid (16 bytes) | to_uuid (16 bytes) | timestamp_be (8 bytes)`
  — big-endian timestamp makes entries lexicographically sortable by time within each node pair,
  enabling efficient range queries ("all traversals for A↔B before date X")

**Archiving trigger** — runs at `take_snapshot()` time (the natural checkpoint):
1. Scan all edges in the graph
2. Partition `edge.traversals` by age threshold
3. Write expired records to fjall `traversal_archive` keyspace with `status: Live`
4. Increment `edge.archived_traversal_count` by the count moved
5. Retain only recent records in `edge.traversals`
6. **Then** truncate the WAL log (after fjall writes are committed)

**WAL ordering is critical:** the fjall `traversal_archive` writes must be committed before the WAL
is truncated. If the WAL is truncated first and the process crashes before fjall commits, cold
records are permanently lost. The archive pass must be atomic from the WAL's perspective: write
fjall, fsync, then truncate WAL.

This runs off the main frame loop and is bounded by total hot-tier traversal count.

**Visual weight** uses the full count without a fjall fetch:
```
total = traversals.len() + archived_traversal_count  // both from in-memory EdgePayload
```

**Edge inspection UI** shows hot traversals immediately, with a "Load older history" control
that triggers a fjall range scan on `(from_uuid, to_uuid)` prefix, paginated by timestamp.

**Recovery when snapshot is lost but cold records exist:** If the snapshot is deleted but fjall
`traversal_archive` still holds cold records, WAL replay rebuilds the hot tier correctly but
`archived_traversal_count` starts at 0 — making visual weights and dominant-direction ratios
understated. Recovery must include a reconciliation pass: after WAL replay without a snapshot,
range-count actual cold records per `(from_uuid, to_uuid)` prefix in fjall and set
`archived_traversal_count` on each edge accordingly. This pass is O(edges) range queries against
fjall — fast, but must be explicitly implemented in the recovery path.

### 4.4 Snapshot serialization (rkyv) — Low risk

`EdgePayload` with `Vec<Traversal>` is straightforwardly rkyv-serializable. `Vec<T>` where T is
`Archive` is supported. The `String` fields (from_url, to_url) serialize as `ArchivedString`.
No alignment issues beyond the existing patterns.

`NavigationTrigger` is a fieldless enum — archives cleanly.

### 4.5 `maybe_add_history_traversal_edge` replacement — Low risk

The existing function (line 2493) is the primary callsite for History edge creation. It is replaced
by a `push_traversal` operation:

```rust
fn push_traversal(&mut self, from_key: NodeKey, to_key: NodeKey, traversal: Traversal) {
    // Find or create petgraph edge from_key → to_key
    if let Some(edge_idx) = self.graph.inner.find_edge(from_key, to_key) {
        self.graph.inner[edge_idx].traversals.push(traversal);
    } else {
        let payload = EdgePayload { user_asserted: false, traversals: vec![traversal] };
        self.graph.inner.add_edge(from_key, to_key, payload);
    }
    // Log the traversal (AppendTraversal log entry)
    self.log_traversal_mutation(from_key, to_key, &traversal);
    self.egui_state_dirty = true;
}
```

The O(n) `has_history_edge` scan that currently discards all repeat traversals is eliminated.

### 4.6 `push_traversal` wiring — decision rule, ordering, and self-loops

**Primary wiring gap:** Currently `maybe_add_history_traversal_edge` only fires from
`WebViewHistoryChanged` (back/forward navigation). Forward navigation to a new URL — the most
common case — goes through `WebViewUrlChanged`, which only updates the node's current URL and
`last_visited`. No traversal record is created for new forward navigations.

Under the new model, `WebViewUrlChanged` must call `push_traversal`. The trigger classification:
- `WebViewUrlChanged` with a prior known URL → `NavigationTrigger::ClickedLink` or `TypedUrl`
  (heuristic; see §7 on trigger classification limits) or `Unknown` if cause unavailable
- `WebViewHistoryChanged` with index moving back → `NavigationTrigger::HistoryBack`
- `WebViewHistoryChanged` with index moving forward on the same list → `NavigationTrigger::HistoryForward`

**Inter-node vs. intra-node decision rule:** `push_traversal` is only called when the navigation
crosses to a *different known node*. The full decision tree at `WebViewUrlChanged`:

1. Capture `prior_url = node.url` before any update
2. If `prior_url == new_url`: no-op (URL didn't change)
3. Resolve `prior_url` → `from_key`, resolve `new_url` → `to_key`
4. If `to_key` is `None` (new URL unknown): intra-node navigation — update `node.url`, no traversal
5. If `from_key == to_key` (same node): intra-node — update `node.url`, no traversal (self-loop skip)
6. If `from_key != to_key` and both `Some`: inter-node — call `push_traversal(from_key, to_key, ...)`
   and do NOT update the current node's URL (the webview has moved to a different known node)

**URL capture ordering constraint:** `push_traversal` needs `prior_url` (the node's current URL
before navigation). This must be captured *before* `update_node_url_and_log` overwrites it.
The current `WebViewUrlChanged` handler reads `node.url` then updates it — `push_traversal` must
be inserted in that window, not after the update.

### 4.7 Hyperlink edge creation — Correctness fix

The current Hyperlink edge (created when a new webview spawns from a parent, line 1118) is
semantically inaccurate: it labels the relationship based on UI action (new tab from parent),
not actual navigation. Under the new model, the edge is created when a URL commits in the new
tab and the destination resolves to a known node. The trigger `NavigationTrigger::ClickedLink`
(or `GraphOpen`, or `TypedUrl`) records how it happened without misrepresenting the relationship.

This is a behavioral correction, not just a cosmetic rename.

---

## 5. Proof of Concept Scope

To prove or disprove viability, the minimum implementation that would provide evidence:

1. **Change `EdgeType` to `EdgePayload`** — swap the weight type in `StableGraph`. Update the 5–6
   callsites in `graph/mod.rs` and `app.rs`.

2. **Update existing tests** — 22 graph tests and 19 persistence tests reference `EdgeType`
   directly. All must be updated for `EdgePayload`. Budget this explicitly; it is not free.

3. **Replace `add_history_traversal_edge` with `push_traversal`** — record a timestamp and trigger
   on each navigation. Remove the `!has_history_edge` guard.

4. **Wire `push_traversal` from `WebViewUrlChanged`** — resolve prior URL + new URL to node keys;
   create traversal record for all new forward navigations, not just back/forward history events.
   This is the primary traversal-creation path.

5. **Add `AppendTraversal` to `LogEntry`** — log traversal events. Replay them on recovery.

6. **Display-layer deduplication** — in `EguiGraphState::from_graph`, skip reversed pairs. Show
   traversal count as visual weight, dominant direction as arrow (>60% threshold).

7. **Edge inspection stub** — clicking an edge in graph view shows a tooltip with traversal count,
   dominant direction ratio, and most-recent timestamp. Does not require full inspection UI.

8. **History Manager stub** — a menu entry that opens a panel listing the last N traversal records
   from `traversal_archive`, with timestamps and URLs. No delete/export yet.

This scope is 1–2 weeks of implementation. The result either demonstrates that:
- Traversal accumulation works correctly without performance degradation (**validates**)
- Persistence growth is unacceptable, display merging has correctness issues, or the model
  adds complexity without proportional benefit (**refutes or qualifies**)

---

## 6. Pros

- **Data fidelity.** Traversal frequency and timing are no longer discarded. The first and most
  recent navigation between any pair are recoverable. Browsing sessions can be reconstructed.

- **Consistent edge creation.** The `Hyperlink`/`History`/`UserGrouped` type distinction is
  eliminated in favor of a single `push_traversal` operation. Edge generation inconsistency
  (Hyperlink created on webview spawn, not URL commit) is corrected.

- **Visual weight as a free feature.** Traversal count drives stroke width. Frequently navigated
  paths are visually prominent without any additional UI design. This is meaningful at a glance.

- **UserGrouped edges become precise.** An asserted edge with zero traversals clearly communicates
  "I declared this relationship, I haven't navigated it." An asserted edge with 20 traversals
  communicates "I declared this and use it constantly." This distinction was invisible before.

- **Edge inspection becomes worthwhile.** Once edges carry timestamped traversal records, clicking
  an edge to inspect it has obvious value. The deferred edge hit-targeting feature has a clear
  payoff.

- **Non-destructive migration.** Old `AddEdge` log entries are replayed unchanged. No existing
  persisted data is lost. The schema extension is additive.

- **Complete history, never truncated.** The hot/cold tiered model retains all traversal records
  indefinitely — recent ones in RAM, older ones on disk. No data is discarded. A user can always
  recover the full traversal history for any edge, arbitrarily far back.

- **Node history alignment.** `NodeHistoryEntry` with timestamps deepens an existing structure
  (`history_entries`) rather than introducing a new concept. Intra-node and inter-node history
  are cleanly separated without redundancy.

---

## 7. Cons and Risks

- **Tiered storage adds operational complexity.** The hot/cold archiving pass at snapshot time is
  a new maintenance operation with no equivalent today. The fjall `traversal_archive` keyspace must
  be kept consistent with `archived_traversal_count` on each edge. A crash mid-archive could leave
  counts stale (fixable by a recovery scan, but non-trivial to implement correctly). The WAL must
  be truncated only after fjall writes are committed and fsynced — this ordering constraint has no
  equivalent in the current codebase and must be implemented carefully.

- **Display deduplication correctness.** Merging A→B and B→A petgraph edges into one visual egui
  edge requires a stable convention for which petgraph edge "owns" the visual edge, and a reliable
  reverse-lookup from egui edge events back to petgraph edges. Edge events from egui_graphs return
  an `EdgeIndex` — if two petgraph edges are merged, the event may identify either one. The adapter
  must handle both.

- **`remove_edges` semantics change.** Currently, deleting an edge is a clean removal of a petgraph
  edge. With `Vec<Traversal>`, there are now multiple granularities of deletion: remove a single
  traversal, clear all traversals (but keep user_asserted), or fully dissolve the edge. The existing
  `RemoveEdge` intent needs to be qualified. Cold-tier records in fjall must also be cleaned up on
  full edge dissolution.

- **Snapshot size is bounded but not zero.** The hot tier (last 90 days of traversals) is
  serialized into the rkyv snapshot. For a heavily used graph this could be tens of thousands of
  records — still likely under 10MB, but larger than the current snapshot. Worth measuring.

- **Testing surface increases substantially.** The existing 22 graph tests and 19 persistence tests
  reference `EdgeType` directly — all must be updated. New test cases needed: push to existing edge,
  merge deduplication, dominant-direction threshold, recovery from `AppendTraversal` log entries,
  hot→cold archive pass correctness, dissolution transfer to History Manager, WAL ordering. This is
  the largest single cost item in the PoC.

- **`NodeHistoryEntry` timestamp reconciliation is non-trivial.** The browser reports a complete
  history list on every `WebViewHistoryChanged` event. When merging into `Vec<NodeHistoryEntry>`,
  timestamps from matching URLs in the prior list must be preserved and only genuinely new URLs
  stamped with `now()`. A naive replace-all would re-stamp the entire list on every event, making
  timestamps useless.

- **`NavigationTrigger` classification is heuristic.** The current code path from Servo does not
  always provide a reliable signal for whether a navigation was a link click vs. a typed URL. The
  `trigger` field may default to `Unknown` in many cases until the WebView API provides richer
  navigation cause data.

- **Node removal is a multi-step operation with strict ordering.** `RemoveNode` must enumerate
  and transfer all incident edge payloads to the History Manager *before* calling petgraph's
  remove_node — after which the payloads are gone. This is a fragile ordering constraint with no
  equivalent in the current codebase.

- **Recovery reconciliation adds startup cost.** When recovering without a snapshot,
  `archived_traversal_count` must be reconstructed by range-counting fjall cold records per edge.
  For large archives this adds startup latency. In practice only relevant after snapshot deletion
  or corruption — acceptable for the PoC, but the reconciliation path must exist.

---

## 8. Open Questions

1. ~~**Separate traversal store or inline Vec?**~~ **Resolved:** Hybrid tiered storage (§4.3a).
   Hot tier inline in `EdgePayload.traversals`; cold tier in fjall `traversal_archive` keyspace.
   No truncation — all records preserved.

2. ~~**Traversal truncation policy.**~~ **Resolved:** No truncation. Age-based archiving to cold
   tier at snapshot time. Hot tier threshold: 90 days (configurable).

3. **Navigation wiring scope.** `push_traversal` must fire from `WebViewUrlChanged` (new forward
   navigation) as the primary path and from `WebViewHistoryChanged` (back/forward) as secondary.
   The open sub-question is trigger classification: Servo's WebView API does not reliably expose
   whether a URL change was from a link click vs. a typed URL. `trigger` may default to `Unknown`
   in many cases; this degrades trigger-based filtering in the History Manager but does not break
   the core model.

4. ~~**Petgraph edge direction convention.**~~ **Resolved:** A→B and B→A remain separate petgraph
   edges, each accumulating their own traversal list. Display layer merges them into one visual
   edge with arrow pointing in the dominant direction (whichever side has more total traversals,
   including archived count). Equal counts → bidirectional or no arrow.

5. ~~**UserGrouped removal semantics.**~~ **Resolved via History Manager:** Removing a user
   assertion clears `user_asserted` only. If the edge has no traversals and no archived count, it
   dissolves and its records (if any) transfer to the History Manager as
   `Dissolved { reason: UserRetracted }`. The user can inspect, restore, or delete from there.
   Traversal history is never silently discarded.

6. **Drag-link-onto-tab routing.** A future feature where dragging a link from page A's content
   onto node B's tile causes B's tab to navigate to the link's destination. This creates both an
   intra-node history entry on B (B changed URLs) and a potential inter-node traversal record
   if the destination resolves to a known node. The traversal's `from_url` should be B's URL
   at time of navigation, not A's; the trigger would be `DraggedLink`. This feature should be
   designed alongside the traversal model, not after.

7. **Hot-tier threshold calibration.** 90 days is a reasonable default but untested. The PoC
   should log hot-tier traversal counts per edge in a real session to validate that 90 days
   keeps the snapshot size acceptable. If a typical active edge accumulates more than ~500
   hot-tier records, the threshold may need tightening or the visual weight scale needs
   normalizing to avoid all edges looking the same weight.

8. **History Manager auto-curation scope.** The auto-curation policy ("delete records older than
   N days") is destructive and irreversible. It should be a separate, explicit user setting, clearly
   distinct from the lossless hot→cold archiving pass. The default should be off. The PoC should
   not implement it; the History Manager stub only needs read + manual delete.

9. **Export / tokenization format.** "Tokenize for portability" is intentionally vague. Candidate
   formats: JSON (generic), CSV (spreadsheet-friendly), PKCE-compatible sync blob (cross-device),
   LLM context window format (session-clustered summaries). This is a post-PoC decision; the data
   model does not need to change to accommodate any of them.

---

## 9. Summary Assessment

The hypotheses are **supported by the code survey** with the following qualifications:

- Hypotheses 1, 3, 6 are confirmed directly by the existing `!has_history_edge` guard and the
  `history_entries` structure: data is currently being discarded, and the intra-node concept
  already exists.

- Hypothesis 2 (Hyperlink as trigger, not type) is confirmed: the current Hyperlink edge creation
  is semantically incorrect — it fires on webview spawn, not URL commit — and the traversal model
  corrects this naturally.

- Hypothesis 4–5 (visual undirected, petgraph unchanged) are technically feasible with the
  display-layer deduplication pass. Medium complexity, no architectural risk.

- Hypothesis 7 (current model loses data) is confirmed: repeat traversals and all timing
  information are silently discarded after the first edge creation.

**Resolved design decisions:**
- Storage: hybrid hot/cold tiered model (§4.3a). No truncation, no data loss.
- Direction: A→B and B→A remain separate petgraph edges; arrow points toward dominant traversal
  direction at display time using a >60% threshold to avoid flicker; equal/ambiguous → bidirectional.
- `EdgeType` enum eliminated; all edge kinds collapse into `EdgePayload`.
- Edge existence condition: `user_asserted || !traversals.is_empty() || archived_traversal_count > 0`.
- Dissolved edges and deleted nodes transfer records to the History Manager (§3.5) — no silent
  orphaning. UserGrouped retraction semantics resolved: assertion-only, history preserved.
- WAL truncation must follow fjall commit (ordering constraint, see §4.3a).
- Node removal must enumerate and transfer incident edge payloads before calling petgraph remove_node.
- `push_traversal` primary path is `WebViewUrlChanged`, not history events.
- Inter/intra-node decision rule: only navigate to a different known node creates an edge traversal;
  same-node or unknown destination is intra-node; self-loops (`from_key == to_key`) are skipped.
- `push_traversal` must capture `prior_url` before `update_node_url_and_log` overwrites it.
- Recovery without snapshot requires an `archived_traversal_count` reconciliation pass against fjall.
- History Manager timeline view is O(n) scan in the PoC; secondary index deferred to when needed.
- `NavigationTrigger::Unknown` added for when Servo does not expose navigation cause.

**Recommended next step:** Proof-of-concept implementation (scope in §5), prioritizing steps 1–3
(type swap, push_traversal, log entry). Steps 4–5 (display merging with dominant-direction arrow,
inspection stub) can follow once the core data model is validated. The tiered archiving pass (§4.3a)
is deferred until after the PoC validates the hot-tier data model — implement it when snapshot size
becomes measurable.

The remaining open questions (back/forward as inter-node traversals, UserGrouped retraction
semantics, hot-tier threshold calibration) are best answered empirically during the PoC rather
than decided upfront.
