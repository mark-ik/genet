# GRAPHSHELL AS A WEB BROWSER

**Purpose**: Detailed specification for how Graphshell operates as a functional web browser.

**Document Type**: Behavior specification (not implementation status)
**Status**: Core browsing graph functional; delegate-driven desktop navigation, three-tier lifecycle (Active/Warm/Cold), LRU eviction, workspace routing in development
**See**: [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) for actual code status

---

## Design Principle: Unified Spatial Tab Manager

Graphshell is a spatial tab manager with three authority domains:

- **Graph**: semantic state (node identity, lifecycle, edges).
- **Tile tree**: layout/focus/visibility state.
- **Webviews**: runtime rendering instances reconciled from graph lifecycle.

- **Graph view**: Overview and organizational control surface. Drag nodes between clusters, create edges, delete nodes - all affect the tile tree and webviews.
- **Tile panes**: Focused working contexts. Each pane's tab bar shows the nodes in that pane's cluster. Closing a tab tile closes the webview and demotes the node to `Cold` (node remains in graph unless explicitly deleted).
- **Tab bars**: Per-pane projections of graph clusters. Active tabs (with webview) are highlighted; inactive tabs (no webview) are dimmed and reactivatable.

**Key invariant**: semantic truth lives in graph/intents; tile and webview runtime state are coordinated through explicit intent/reconciliation boundaries.

---

## 1. Graph-Tile-Webview Relationship

### Node Identity

Each node IS a tab. Node identity is the tab itself, not its URL.

- **URLs are mutable**: Within-tab navigation changes the node's current URL. The node persists.
- **Duplicate URLs allowed**: The same URL can be open in multiple tabs (multiple nodes). Each is independent.
- **Stable ID**: Nodes are identified by a stable UUID (not URL, not petgraph NodeIndex). Persistence uses this UUID.
- **Per-node history**: Each node has its own back/forward stack. Servo provides this via `notify_history_changed(webview, entries, index)`.

### Servo Signals

Servo provides two distinct signals that drive the graph (no Servo modifications required):

| User action | Servo delegate method | Graph effect |
|-------------|----------------------|--------------|
| Click link (same tab) | `notify_url_changed(webview, url)` | Update node's current URL and title. Push to history. No new node. |
| Back/forward | `notify_url_changed(webview, url)` | Update node's URL. History index changes. No new node. |
| Ctrl+click / middle-click / window.open | `request_create_new(parent_webview, request)` | Create new node. Create edge from parent node. Add to parent's tab container. |
| Title change | `notify_title_changed(webview, title)` | Update node's title. |
| History update | `notify_history_changed(webview, entries, index)` | Store back/forward list on node (from Servo, not custom). |

---

## Research Conclusions (2026-02-15)

The architecture plan identified a previous mismatch (URL-polling assumptions and fragmented routing). For desktop tile flow, this has been addressed: navigation semantics are delegate-driven, structural node creation is not polling-driven, and mutations route through intent/reconciliation boundaries. Remaining deferred scope is EGL/WebDriver explicit-target parity. See [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md).

### Edge Types

**Current implementation** uses `EdgeType` enum:

| Edge type | Created by | Meaning |
|-----------|-----------|---------|
| `Hyperlink` | `request_create_new` (new tab from parent) | User opened a new tab from this page |
| `History` | Back/forward detection (existing reverse edge) | Navigation reversal |
| `UserGrouped` | Explicit split-open grouping gesture (`Shift + Double-click` in graph) | User deliberately associated two nodes |

**Edge Traversal Model** (planned replacement): `EdgeType` will be replaced by `EdgePayload` containing `Vec<Traversal>` records. Each traversal captures the full navigation event (from_url, to_url, timestamp, trigger). This model preserves repeat navigations, timing data, and enables commutative P2P sync. See [2026-02-20_edge_traversal_impl_plan.md](implementation_strategy/2026-02-20_edge_traversal_impl_plan.md) for implementation timeline.

### Pane Membership and Workspaces

- **Tile tree is the authority** on which node lives in which pane.
- **Navigation routing**: New nodes from `request_create_new` are added to the parent node's tab container.
- **New root node** (N key, no parent): Creates a new tab container in the tile tree.
- **Tab move** (drag between panes): Moves the tile. `UserGrouped` creation for drag-move is follow-up work; current explicit grouping trigger is split-open.

**Workspace routing** (in development): Opening a node predictably restores the expected workspace context. Nodes track workspace membership (UUID → set of workspace names). Routing resolver uses recency and membership index to decide whether to restore an existing workspace or open in current workspace. See [2026-02-19_workspace_routing_and_membership_plan.md](implementation_strategy/2026-02-19_workspace_routing_and_membership_plan.md).

### Node Lifecycle

**Current code** implements a three-tier lifecycle model (see `graph/mod.rs` `NodeLifecycle` enum):

| State | Has webview? | Shown in tab bar? | Shown in graph? | LRU eviction |
|-------|-------------|-------------------|-----------------|-------------|
| **Active** | Yes (visible) | Yes (highlighted) | Yes (full color) | LRU cache (default: 4 slots) |
| **Warm** | Yes (backgrounded) | Yes (dimmed) | Yes (medium color) | LRU cache (default: 12 slots) |
| **Cold** | No (metadata only) | Yes (dimmed) | Yes (dimmed) | Unlimited |

**Lifecycle transitions:**
- Focus a node → `PromoteToActive` intent → creates/reactivates webview
- Navigate away from a tab → `DemoteToWarm` intent → webview persists but not visible
- Active LRU overflow → `DemoteToWarm` intent (oldest non-pinned active node)
- Warm LRU overflow → `EvictWarmToCold` intent → webview destroyed, metadata retained
- Memory pressure (warning/critical) → stronger eviction cascade (Active → Warm → Cold)
- Close tab tile → `DemoteToWarm` or `EvictWarmToCold` (depending on warm cache fullness)
- Delete node → `RemoveNode` intent → node removed from graph entirely

**Lifecycle Intent Vocabulary**: `PromoteToActive`, `DemoteToWarm`, `EvictWarmToCold`, `Reactivate`, `Destroy` drive webview creation/destruction through the reconciliation phase.

See [2026-02-19_persistence_hub_plan.md](implementation_strategy/2026-02-19_persistence_hub_plan.md) for LRU budget policy details and [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md) for lifecycle intent flow.

### Intent-Based Mutation

All user interactions produce intents processed at a single sync point per frame. No system directly mutates another mid-frame.

**Two-Phase Apply Model:**

1. **Phase 1 — Pure Reducer** (state mutation): Processes `GraphIntent` enum, updates graph structure and node metadata, emits `WebViewReconciliationIntent` for runtime side effects.
2. **Phase 2 — Reconciliation** (webview lifecycle): Creates/destroys webviews based on lifecycle intents, navigates webviews to updated URLs, updates tile tree focus/layout.

**Phase gap invariant**: Sub-frame gap between phases prevents contradictory webview state (e.g., navigating a destroyed webview, creating duplicate webviews for the same node).

Sources of intents:
- **Graph view**: drag-to-cluster, delete node, create edge, select
- **Tile/tab bar**: close tab, reorder tabs, drag tab to other pane
- **Keyboard**: N (new node), Del (remove), T (physics toggle), etc.
- **Servo callbacks**: `request_create_new`, `notify_url_changed`, `notify_title_changed` → converted to GraphIntent

**Delegate-driven routing**: Servo callbacks → GraphIntent emission → reducer application → reconciliation effects. No polling, no fragmented mutation paths.

See [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md) for detailed phase specification and [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) for the architectural summary.

---

## 2. Navigation Model

### Within-Tab Navigation (Link Click)

**Scenario**: User is in a pane viewing node A (github.com), clicks a link to github.com/servo.

**Behavior**: The node's URL updates. No new node is created. Servo's `notify_url_changed` fires.

- Node A's `current_url` changes to github.com/servo
- Node A's title updates when `notify_title_changed` fires
- Node A's history stack gains an entry (provided by `notify_history_changed`)
- The tab bar entry for A updates to show the new title/URL
- No edge created, no new node

### Open New Tab (Ctrl+Click, Middle-Click, window.open)

**Scenario**: User Ctrl+clicks a link on node A, opening it in a new tab.

**Behavior**: A new node is created with an edge from A. Servo's `request_create_new` fires.

- New node B created with the target URL
- Edge A → B created (type: Hyperlink)
- B's tile added to A's tab container (same pane)
- B becomes the active tab in that pane
- A becomes inactive (no webview, still in tab bar)

### Back/Forward Navigation

**Scenario**: User presses back button in the browser UI.

**Behavior**: Servo traverses its own history stack. `notify_url_changed` fires with the previous URL. The node's URL updates. No new node.

Servo provides the full back/forward list via `notify_history_changed(webview, entries, index)`. Graphshell stores this on the node or reads it from the WebView on demand — no need to maintain a custom history stack.

### New Root Tab (N Key)

**Scenario**: User presses N to create a blank tab.

**Behavior**: New node created with `about:blank`. New tab container created in tile tree. No parent, no edge.

---

## 3. Bookmarks Integration

**Current status**: Manual edge creation; bookmarks as node metadata.

**Planned implementation** (Persistence Hub Plan Phase 1):

- **Bookmarks are tags**: `Node.tags: Vec<String>` field stores bookmark folder paths (e.g., `["bookmarks", "work", "research"]`).
- **Settings page**: `graphshell://settings/bookmarks` provides bookmark management UI (add/remove tags, bulk import, export).
- **Import/export**: Firefox `bookmarks.html` import creates nodes with tag metadata. Export generates standard bookmark HTML.
- **Search integration**: Omnibar searches node tags (fuzzy match via nucleo).
- **Visual indicator**: Bookmark icon overlay on tagged nodes in graph view.

See [2026-02-19_persistence_hub_plan.md](implementation_strategy/2026-02-19_persistence_hub_plan.md) Phase 1 and [2026-02-20_settings_architecture_plan.md](implementation_strategy/2026-02-20_settings_architecture_plan.md) for UI delivery.

---

## 4. Downloads & Files

**Scenario**: User downloads a file from a webpage.

- Download tracked with source node reference
- Downloads page (`graphshell://settings/downloads`) shows in-progress + completed downloads
- Download metadata stored per-node for provenance

**Implementation note**: Download tracking integrates with settings architecture (`graphshell://` internal URL scheme). See [2026-02-20_settings_architecture_plan.md](implementation_strategy/2026-02-20_settings_architecture_plan.md).

---

## 5. Search & Address Bar

**Omnibar** serves dual purpose: graph search + URL navigation.

- **URL input** (`http://...`): Navigates the current tab (within-tab navigation).
- **Text search**: Fuzzy search via `nucleo` matcher (FT6 in roadmap, now implemented).
  - Searches node titles, URLs, and tags (bookmark folders).
  - Score-ranked results with keyboard navigation.

**Graph search modes** (Ctrl+F panel):

- **Highlight mode** (default): Matching nodes highlighted in gold, dimmed non-matches remain visible.
- **Filter mode** (toggle in search panel): Hide non-matching nodes entirely, collapse edges to preserved nodes.

**Faceted search** (planned): Filter by lifecycle state, edge type, traversal recency, tag, visit count. See [2026-02-19_graph_ux_polish_plan.md](implementation_strategy/2026-02-19_graph_ux_polish_plan.md) Phase 4 for DOI/relevance weighting integration.

---

## Summary: How Graphshell Differs from Traditional Browsers

| Feature | Firefox | Graphshell |
|---------|---------|-------|
| **Primary UI** | Tab bar | Force-directed graph + tiled panes |
| **Tab management** | Linear tab strip | Spatial graph (drag, cluster, edge) |
| **Navigation** | Click link → same tab or new tab | Same: within-tab nav or new tab |
| **History** | Global linear history | Per-node history (from Servo) + graph edges + traversal log |
| **Tab grouping** | Manual tab groups | Graph clusters = pane tab bars |
| **Bookmarks** | Folder tree | Node tags (folder paths as metadata) |
| **Lifecycle** | Active/discarded binary | Active/Warm/Cold three-tier with LRU |
| **Workspaces** | Window-based sessions | Named workspace snapshots with membership routing |
| **Settings** | Preferences dialog | `graphshell://` internal pages as nodes |

**Core difference**: The graph is the organizational layer. Tab bars are projections of graph clusters. What you do in the graph is what the tile tree becomes. Nodes track workspace membership; opening a node restores expected context.

---

## Related Documentation

**Core architecture:**
- [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) — implementation status, data structures, key decisions
- [IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md) — feature targets and validation tests

**Implementation plans:**
- [2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md) — two-phase apply model, lifecycle intents
- [2026-02-20_edge_traversal_impl_plan.md](implementation_strategy/2026-02-20_edge_traversal_impl_plan.md) — EdgeType → EdgePayload migration
- [2026-02-19_workspace_routing_and_membership_plan.md](implementation_strategy/2026-02-19_workspace_routing_and_membership_plan.md) — workspace membership index and routing resolver
- [2026-02-20_settings_architecture_plan.md](implementation_strategy/2026-02-20_settings_architecture_plan.md) — `graphshell://` internal URL scheme
- [2026-02-19_persistence_hub_plan.md](implementation_strategy/2026-02-19_persistence_hub_plan.md) — bookmarks (tags), node history, LRU lifecycle budgets
- [2026-02-19_graph_ux_polish_plan.md](implementation_strategy/2026-02-19_graph_ux_polish_plan.md) — search modes (Highlight/Filter), DOI/relevance

