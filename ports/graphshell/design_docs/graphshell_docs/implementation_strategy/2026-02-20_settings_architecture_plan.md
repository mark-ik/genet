<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Settings Architecture Plan (2026-02-20)

**Status**: Draft — implementation not started.

**Bridge note (2026-02-20):**
Current in-app `Settings` menu work in `desktop/toolbar_ui.rs` is the bridge implementation:
persisted preferences, input bindings, and omnibar controls are already user-configurable there.
This plan is the structural next step to unify those controls behind a page-based settings model,
not a replacement claim that settings do not exist today.

---

## Plan

### Context

GraphShell currently scatters settings across floating panels (`P` for physics, `Ctrl+F` for
search, `Persistence Hub` for data management) and egui dialogs. There is no unified settings
model, no consistent navigation between settings surfaces, and no way to persist which panel a
user prefers for a given category.

This plan introduces a `graphshell://` internal URL scheme where each settings category is a
named page. Settings pages can be opened like any URL in the detail view, making them composable
with the rest of the workspace model: a settings page is just a node that happens to have a
`graphshell://` URL. Users can pin settings pages to workspaces, open them in split panes, and
configure whether each category prefers a full-page, sidebar, or floating panel display.

The core architectural claim: **settings are nodes, not dialogs.**

### Selection Policy Cross-Reference (2026-02-20)

Selection interaction policy is shared with UX polish decisions and should stay consistent across
settings/keybinding surfaces:

- `Shift+Click` is the cross-domain range-select gesture for ordered UI surfaces
  (tabs, settings lists, omnibar/list rows, and other ordered collections).
- Graph-view spatial selection remains box/lasso first (`Right+Drag` default), with optional
  additive spatial behavior on `Shift+Right+Drag`.
- Source of record for this decision: UX polish plan Phase 5.5 in
  `2026-02-19_graph_ux_polish_plan.md`.

When implementing `graphshell://settings/keybindings`, treat this as required behavior for command
tables and any ordered settings lists that support multi-select/range operations.

---

### Phase 1: `graphshell://` URL Scheme Handler

**Goal:** The Servo embedder intercepts `graphshell://` navigations and serves bundled content.
`graphshell://settings` opens a settings index page. The graph treats it as any other node.

#### 1.1 Scheme Interception

Servo's `AllowNavigationRequest` embedder message fires before every navigation. When the URL
scheme is `graphshell`, the embedder intercepts rather than forwarding to the network stack.

```rust
// desktop/webview_controller.rs (or a new desktop/internal_scheme.rs)
pub fn handle_internal_url(url: &str) -> Option<InternalPageContent> {
    match url {
        "graphshell://settings"               => Some(SETTINGS_INDEX_HTML),
        "graphshell://settings/persistence"   => Some(PERSISTENCE_HTML),
        "graphshell://settings/keybindings"   => Some(KEYBINDINGS_HTML),
        "graphshell://settings/appearance"    => Some(APPEARANCE_HTML),
        "graphshell://settings/physics"       => Some(PHYSICS_HTML),
        "graphshell://settings/downloads"     => Some(DOWNLOADS_HTML),
        "graphshell://settings/bookmarks"     => Some(BOOKMARKS_HTML),
        "graphshell://settings/history"       => Some(HISTORY_HTML),
        "graphshell://settings/workspaces"    => Some(WORKSPACES_HTML),
        "graphshell://settings/notifications" => Some(NOTIFICATIONS_HTML),
        "graphshell://settings/about"         => Some(ABOUT_HTML),
        _ => None,
    }
}
```

`InternalPageContent` is a `&'static str` of HTML content bundled with `include_str!` at compile
time. When intercepted, the embedder responds to Servo with the HTML body directly (via
`webview.load_html(content)`).

**Two-tier implementation strategy:**

- **Tier 1 (fast):** egui panels rendered as an overlay when the URL is recognized, before Servo
  ever loads. The URL bar shows `graphshell://settings/X`, the node appears in the graph, but the
  content is egui (no HTML authoring required). Looks like a browser page; runs as egui.
- **Tier 2 (later):** Replace individual pages with real HTML/CSS/JS served via the scheme
  handler. JS communicates back to the embedder via `window.ipc.postMessage(json)` — a custom
  JavaScript bridge registered on the webview. Enables richer UI without recompiling.

Phase 1 ships Tier 1. Tier 2 is an incremental upgrade per page.

**Tasks**

- [ ] Add `InternalSchemeHandler` struct to `desktop/internal_scheme.rs`.
- [ ] In `webview_controller.rs`: before calling `webview.load_url(url)`, check
  `InternalSchemeHandler::is_internal(url)`. If true, emit `GraphIntent::OpenInternalPage(url)`
  instead of a Servo navigation.
- [ ] `OpenInternalPage` intent: create a node with the `graphshell://` URL (via normal `AddNode`
  path), then attach an `InternalPageOverlay` to that tile (an egui-rendered panel that displays
  over the tile area instead of a Servo webview).
- [ ] The node appears in the graph, can be pinned, added to workspaces, and opened in split
  panes — all via existing mechanisms. No special-casing required in the graph layer.

**Validation Tests**

- `test_internal_scheme_recognized` — `graphshell://settings` → `is_internal()` returns true.
- `test_regular_url_not_internal` — `https://servo.org` → `is_internal()` returns false.
- Headed: type `graphshell://settings` in omnibar → settings index panel opens in detail view;
  node appears in graph with the `graphshell://settings` URL.

---

#### 1.2 Settings Node Identity

`graphshell://` nodes are treated as regular nodes for all graph purposes. The URL is stable and
unique — two navigations to `graphshell://settings/physics` open the same node (URL-deduplication,
existing behavior via `get_node_by_url`). The node's title is set by the internal page name
(e.g., "Physics Settings").

**Special behavior:**
- Settings nodes are never auto-deleted (they carry `is_pinned = true` by default, or a new
  `is_internal: bool` flag prevents deletion).
- They do not generate traversal records (navigating *to* a settings page from a content node
  does not push a traversal — `graphshell://` URLs are excluded from `push_traversal`).

**Tasks**

- [ ] In `push_traversal()`: add guard `if new_url.starts_with("graphshell://") { return; }`.
- [ ] In node creation path for internal pages: set `node.is_pinned = true` (or add
  `node.is_internal: bool` if a softer separation is preferred).
- [ ] Exclude `graphshell://` nodes from "clear graph" operations.

---

### Phase 2: Settings Pages

Each page is an egui panel (Tier 1) covering the same content as the existing settings surfaces,
reorganized into a coherent hierarchy. New categories (downloads, bookmarks, history) are added.

#### 2.1 `graphshell://settings` — Index

A table-of-contents page listing all settings categories with one-line descriptions and
navigation links. The first settings page a user ever opens.

Layout (egui):
```
GraphShell Settings
─────────────────────────────────────────
Persistence          Data directory, backup, snapshot interval
Keybindings          Customize keyboard shortcuts and commands
Appearance           Themes, node colors, font size, edge styles
Physics              Layout presets and simulation parameters  ← replaces P panel
Downloads            Download history and source-node links
Bookmarks            Starred nodes and reading lists
History              Traversal archive and timeline
Workspaces           Default workspace, auto-save behavior
Notifications        Toast settings and event subscriptions
About                Version, build info, licenses
```

Each row is a clickable link that navigates the current tile to the target `graphshell://` URL.

---

#### 2.2 `graphshell://settings/persistence`

Moves the Persistence Hub content into a full settings page. Adds:
- Data directory chooser (with restart-required toast if changed).
- Snapshot interval (minutes).
- WAL retention (days before compaction).
- Hot-tier traversal retention (days — from edge traversal plan §2.1).
- "Open data directory in file manager" action.
- "Export full backup (zip)" action.
- "Import backup" action.

---

#### 2.3 `graphshell://settings/keybindings`

Replaces the current `Settings → Input` panel. Shows a table of all registered commands with
their current binding. Clicking a binding field captures the next keypress as the new binding.

Columns: Command | Category | Current Binding | Default

Integrates with the existing `InputBindings` persisted struct (from UX polish plan, Session 13).
Adds:
- "Reset all to defaults" action.
- Chord binding entry (Tab → secondary key).
- Search/filter bar above the table.

---

#### 2.4 `graphshell://settings/appearance`

New page (no existing equivalent):
- Theme selector (Dark / Light / System).
- Node color scheme (current hardcoded colors become editable).
- Edge stroke width base.
- Font size multiplier.
- Physics panel style (floating panel vs. settings page — see Phase 3).

---

#### 2.5 `graphshell://settings/physics`

Moves the existing `P` panel into a settings page. The `P` key can still open this page (as a
navigation to `graphshell://settings/physics` rather than toggling a floating panel).

Content is identical to the current physics panel but with more space: preset buttons, parameter
sliders, degree repulsion toggle, domain clustering toggle (from layout advanced plan).

The current floating physics panel is **deprecated** in favor of this page; `P` now navigates to
it. For users who prefer the sidebar, this page supports sidebar display mode (Phase 3).

---

#### 2.6 `graphshell://settings/downloads`

New page:
- List of all downloads with filename, source URL, source node (graph-linked), timestamp, size.
- Clicking source node → pans graph + selects the node (emits `GraphIntent::FocusNode`).
- Download directory setting.
- "Open downloads folder" action.
- Requires: Servo embedder download interception (record `(filename, url, node_key, timestamp)` on
  each `EmbedderMsg::DownloadStarted` or equivalent).

---

#### 2.7 `graphshell://settings/bookmarks`

Bookmarks are metadata tags on nodes (`#starred`, `#reading-list`, `#archive`, user-defined).
This page is the management surface.

- Add tag field `tags: Vec<String>` to `Node`. Persist via new `LogEntry::TagNode { url, tag }` /
  `UntagNode`.
- List all tagged nodes grouped by tag.
- Clicking a node row: navigate graph to that node.
- Add/remove tags from this page.
- Export bookmarks as HTML (Netscape bookmark format — universal import target).

Bookmarked nodes get a visual indicator in graph view (star icon or ring — from UX research §15.3).
The faceted search `is:starred` predicate (UX polish §5.3) uses this tag.

---

#### 2.8 `graphshell://settings/history`

The History Manager UI from edge traversal plan §2.3 (Timeline, Dissolved, Delete, Auto-curation,
Export). This is the graphical front-end for the `traversal_archive` fjall keyspace.

Cross-reference: edge traversal implementation plan Phase 2.3.

---

#### 2.9 `graphshell://settings/workspaces`

- List of all saved workspaces with names and last-modified timestamps.
- Rename, delete, duplicate workspace actions.
- Default workspace setting (loaded on startup).
- Auto-save interval.
- Cross-reference: workspace routing plan.

---

#### 2.10 `graphshell://settings/notifications`

- Toggle toast for each event class: clipboard, save, navigation, crash, workspace switch,
  settings apply.
- Toast duration (seconds).
- Toast position (top-right / bottom-right / top-left / bottom-left).
- Toast max count (prevent stack overflow).

---

#### 2.11 `graphshell://settings/about`

- GraphShell version, build date, commit hash.
- Servo version.
- License text (MIT/MPL).
- "Check for updates" action (future).

---

### Phase 3: Display Mode Configuration

**Goal:** Each settings category can be displayed as a full page (tile in detail view), a sidebar
panel, or a floating panel. The user's preference per category is persisted.

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SettingsDisplayMode {
    /// Opens in detail view as a tile — default for most categories.
    FullPage,
    /// Docked sidebar (left or right edge of the window).
    Sidebar,
    /// Non-modal floating panel (like the current physics panel).
    FloatingPanel,
}

pub struct SettingsDisplayPreferences {
    pub per_page: HashMap<String, SettingsDisplayMode>,
    // Key = the URL path, e.g. "settings/physics"
}
```

**Default display modes** (before user customization):

| Page | Default mode |
| --- | --- |
| `settings` (index) | FullPage |
| `settings/persistence` | FullPage |
| `settings/keybindings` | FullPage |
| `settings/appearance` | FullPage |
| `settings/physics` | Sidebar |
| `settings/downloads` | FullPage |
| `settings/bookmarks` | FullPage |
| `settings/history` | FullPage |
| `settings/workspaces` | FullPage |
| `settings/notifications` | FloatingPanel |
| `settings/about` | FullPage |

**Tasks**

- [ ] Add `SettingsDisplayPreferences` to persistent app config.
- [ ] In `OpenInternalPage` intent handler: check `SettingsDisplayPreferences` for the URL; open
  as FullPage (existing tile), Sidebar (`egui::SidePanel`), or FloatingPanel (`egui::Window`).
- [ ] Add a display mode toggle (icon button: `[⊞ Full] [◫ Sidebar] [⊟ Panel]`) at the top of
  each settings page.
- [ ] Persist preference on toggle.

**Validation Tests**

- `test_display_mode_persisted` — set physics page to Sidebar; reload app config → Sidebar is
  still the stored preference.
- `test_display_mode_default_for_unknown_page` — page with no stored preference → FullPage default.
- Headed: open `graphshell://settings/physics` → opens as Sidebar by default; toggle to FullPage
  → reopens as tile.

---

## Settings Surface Migration

Existing settings surfaces and their destinations:

| Current surface | Migrated to |
| --- | --- |
| Persistence Hub (inline panel) | `graphshell://settings/persistence` |
| Physics panel (`P` key) | `graphshell://settings/physics` |
| Search display mode toggle | `graphshell://settings/appearance` |
| Settings → Input (lasso, shortcuts) | `graphshell://settings/keybindings` |
| Help panel (`F1`) | Unchanged — help is a transient overlay, not a settings page |
| Graph info overlay | Unchanged — graph view HUD, not settings |

During migration, the old surfaces remain functional until their replacement page is shipped
(one category at a time). The old shortcut keys (`P`) navigate to the new page rather than
toggling a panel.

---

## Findings

### Why Settings as Nodes

Every feature of the graph layer applies automatically to settings pages at zero implementation
cost:

- **Workspace membership**: pin `graphshell://settings/physics` to the "Physics exploration"
  workspace — it appears in that workspace automatically.
- **Split pane**: open `graphshell://settings/keybindings` alongside a content page to
  reconfigure shortcuts while browsing.
- **Search**: `domain:graphshell` in `Ctrl+F` surfaces all settings pages in the graph.
- **History traversal**: if the user navigates from a content node to a settings page, that
  shows up in the timeline — useful for "when did I last change this setting?"
- **Node sharing**: future P2P collaboration — share a `graphshell://settings/appearance`
  node to share a theme configuration.

### Bootstrap Settings Constraint

A small set of settings must be accessible before the graph layer or Servo initializes:
data directory and crash-recovery mode. These remain as startup egui dialogs (existing behavior)
and are not migrated to `graphshell://` pages. Everything else migrates.

### Research Cross-References

- Phase 1: §15.3 (integrated browser panels concept), §15.4 (unified omnibar — `graphshell://`
  in the omnibar is the same `graphshell://settings/X` navigation)
- Phase 2.7: §15.3 (bookmarks as node tags)
- Phase 2.8: edge traversal implementation plan Phase 2.3

---

## Progress

### 2026-02-20 — Session 1

- Plan created from discussion of §15.3 (integrated browser panels) from UX research report.
- Extended to cover all existing settings surfaces (physics panel, persistence hub, input
  settings) under a unified `graphshell://` scheme.
- Display mode configuration (Phase 3) added to support user-configurable sidebar / full-page /
  panel preferences per category.
- Implementation not started.
