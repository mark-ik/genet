# GRAPHSHELL AS A WEB BROWSER

**Purpose**: Detailed specification for how Graphshell operates as a functional web browser.

**Document Type**: Behavior specification (not implementation status)
**Status**: Core browsing graph functional, delegate-driven desktop navigation/control-plane implemented
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

| Edge type | Created by | Meaning |
|-----------|-----------|---------|
| `Hyperlink` | `request_create_new` (new tab from parent) | User opened a new tab from this page |
| `History` | Back/forward detection (existing reverse edge) | Navigation reversal |
| `UserGrouped` | Explicit split-open grouping gesture (`Shift + Double-click` in graph) | User deliberately associated two nodes |

### Pane Membership

- **Tile tree is the authority** on which node lives in which pane.
- **Navigation routing**: New nodes from `request_create_new` are added to the parent node's tab container.
- **New root node** (N key, no parent): Creates a new tab container in the tile tree.
- **Tab move** (drag between panes): Moves the tile. `UserGrouped` creation for drag-move is follow-up work; current explicit grouping trigger is split-open.

### Node Lifecycle

**Current code** implements `Active` and `Cold` (see `graph/mod.rs`). The target model below extends this:

| State | Has webview? | Shown in tab bar? | Shown in graph? | Code status |
|-------|-------------|-------------------|-----------------|-------------|
| **Active** | Yes | Yes (highlighted) | Yes (full color) | Implemented |
| **Cold** (spec: Inactive) | No (suspended) | Yes (dimmed) | Yes (dimmed) | Implemented as `Cold` |
| **Closed** | No (destroyed) | No | No | Not yet a distinct state; removal deletes the node |

- Navigate away from a tab: old node becomes **Cold** (no webview, still in graph and tab bar).
- Click cold tab: **reactivates** it (creates webview, navigates to its current URL).
- Close tab tile (from tab bar): node is demoted to `Cold` and can be reactivated.
- Delete node (graph action/keyboard delete): node is removed from graph.
- A distinct `Closed` lifecycle state remains planned but is not yet a separate runtime state.

### Intent-Based Mutation

All user interactions produce intents processed at a single sync point per frame. No system directly mutates another mid-frame.

Sources of intents:
- **Graph view**: drag-to-cluster, delete node, create edge, select
- **Tile/tab bar**: close tab, reorder tabs, drag tab to other pane
- **Keyboard**: N (new node), Del (remove), T (physics toggle), etc.
- **Servo callbacks**: `request_create_new`, `notify_url_changed`, `notify_title_changed`

All intents are collected, then applied at a single frame boundary, followed by runtime reconciliation. This prevents contradictory updates from fragmented mutation paths.

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

**Current**: Manual edge creation

**Expected**: Browser-like bookmark UI

- Ctrl+B toggles bookmark for current node
- Bookmarks are metadata on nodes (tag/flag), not separate entities
- Bookmark folders map to user-defined groupings
- Import bookmarks.html from Firefox creates nodes + edges

---

## 4. Downloads & Files

**Scenario**: User downloads a file from a webpage.

- Download tracked with source node reference
- Downloads sidebar (Phase 2) shows in-progress + completed
- Download metadata stored per-node for provenance

---

## 5. Search & Address Bar

- Omnibar serves dual purpose: graph search + URL navigation
- URL input (`http://...`) navigates the current tab (within-tab navigation)
- Text input searches node titles/URLs (fuzzy, via nucleo in FT6)

---

## Summary: How Graphshell Differs from Traditional Browsers

| Feature | Firefox | Graphshell |
|---------|---------|-------|
| **Primary UI** | Tab bar | Force-directed graph + tiled panes |
| **Tab management** | Linear tab strip | Spatial graph (drag, cluster, edge) |
| **Navigation** | Click link → same tab or new tab | Same: within-tab nav or new tab |
| **History** | Global linear history | Per-node history (from Servo) + graph edges |
| **Tab grouping** | Manual tab groups | Graph clusters = pane tab bars |
| **Bookmarks** | Folder tree | Node metadata (tags/flags) |

**Core difference**: The graph is the organizational layer. Tab bars are projections of graph clusters. What you do in the graph is what the tile tree becomes.

---

## Related

- Architecture and navigation plan: [implementation_strategy/2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md)
- Architecture and code status: [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md), [IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md)

