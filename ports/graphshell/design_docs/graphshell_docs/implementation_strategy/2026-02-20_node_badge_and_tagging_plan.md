<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# Node Badge and Tagging Plan (2026-02-20)

**Status**: Draft — implementation not started.

**Prerequisites**: Persistence hub Phase 1 (tags data model: `tags: HashSet<String>` on `Node`,
`TagNode`/`UntagNode` log entries, `tag_index`). This plan covers the visual and interactive
layers on top of that data model.

---

## Plan

### Context

Tags are user-applied node attributes (persistence hub plan Phase 1). This plan covers:

1. **Badge visual system** — how tags are rendered on graph nodes and tab headers.
2. **Tag assignment UI** — the `T`-key floating panel for adding and removing tags.
3. **Icon system** — emoji (primary) and Lucide SVG (extended) icon sources.

#### Tag Namespace Convention

The `#` prefix is the **reserved system namespace**. Tags beginning with `#` may carry behavioral
effects. User-defined tags without `#` (`work`, `research`, `todo`) are purely organizational and
never trigger system behavior.

**Reserved special tags:**

| Tag | Responding system | Effect |
| --- | --- | --- |
| `#pin` | Physics simulation | Node not displaced by the simulation (physics anchor). |
| `#starred` | Search / omnibar | Soft bookmark; surfaces via `@b` scope and `is:starred` predicate. |
| `#archive` | Graph view | Hidden from default graph view; demoted to archive tier. Not deleted. |
| `#resident` | Lifecycle / cache | Never cold-evicted regardless of workspace. Global complement to `tile.is_resident`. |
| `#private` | Rendering / export | URL and title redacted in overlays when screen-sharing mode is active. Excluded from JSON export. |
| `#nohistory` | Traversal recording | Navigating through this node does not push a traversal entry. |
| `#monitor` | Background scheduler | Periodically reload and compare DOM hash; badge/toast on content change. |
| `#unread` | Badge / DOI | Auto-applied when a node is added or its URL changes; cleared on first activation. |
| `#focus` | DOI weight | Boosts this node's degree-of-interest score; floats it toward center in the layout. Short-term attention marker. |
| `#clip` | Node type | Node is a clipped DOM element. Distinct node shape/border in graph view; `is:clip` omnibar predicate. Cross-ref: clipping DOM extraction plan. |

The tag assignment UI warns (but does not block) when a user types a `#`-prefixed tag that is not
in the reserved list — the tag is accepted as user-defined and carries no system effect. This
forward-compatible: future versions can upgrade a previously-inert `#foo` tag to a reserved one
without breaking existing data.

---

### Phase 1: Badge Visual System

#### 1.1 Badge Types

```rust
#[derive(Debug, Clone)]
pub enum Badge {
    /// Node has crashed or Servo reported an error.
    Crashed,
    /// Node is a member of N workspaces (shown as a count chip).
    WorkspaceCount(usize),
    /// Node has the #pin tag (physics anchor).
    Pinned,
    /// Node has the #starred tag (soft bookmark).
    Starred,
    /// Node has the #unread tag (auto-applied; cleared on first activation).
    Unread,
    /// Node has a system or user tag with an icon and optional label.
    Tag { label: String, icon: BadgeIcon },
}

#[derive(Debug, Clone)]
pub enum BadgeIcon {
    /// Unicode emoji character (e.g. "📌", "⭐").
    Emoji(String),
    /// Lucide SVG icon identified by slug (e.g. "bookmark", "tag").
    Lucide(&'static str),
    /// No icon — label-only chip.
    None,
}
```

#### 1.2 Priority Order

Badges render in this priority order (highest first). When space is constrained, lower-priority
badges are hidden first:

1. `Crashed` — always visible, red indicator.
2. `WorkspaceCount` — shown when node belongs to 2+ workspaces.
3. `Pinned` — `#pin` tag present.
4. `Starred` — `#starred` tag present.
5. `Unread` — `#unread` tag present; rendered as a colored dot (distinct from icon chips).
6. Other system tags (`#focus`, `#monitor`, `#private`, `#archive`, `#resident`, `#nohistory`) —
   rendered as `Tag` chips using their default emoji; ordered by tag insertion order.
7. User-defined tags — ordered by insertion order.

`#archive` is a special case: the badge renders but the node itself is visually dimmed (reduced
opacity in graph view). `#unread` is auto-managed by the system and is the only badge the user
does not assign manually.

#### 1.3 At-Rest Display (Graph View)

On graph nodes, badges occupy a small overlay region at the node's top-right corner:

- At rest: up to **3** badges rendered as small icon-only circles (16×16 px). If more than 3
  badges exist, the third slot shows a `+N` overflow chip.
- `Crashed` uses a red ⚠ glyph and overrides any other badge in slot 1.
- `WorkspaceCount` renders as a small numeric chip (e.g. `2`).
- `Pinned` renders the 📌 emoji or a Lucide `pin` icon.
- `Starred` renders the ⭐ emoji or a Lucide `star` icon.
- User tags render the assigned `BadgeIcon`; if `None`, a small colored dot.

#### 1.4 Hover/Focus Expansion (Orbit Model)

When the cursor enters a node, or when the node is keyboard-focused:

- All badges expand from their at-rest corner position and orbit the node periphery.
- Expansion animation: 120 ms ease-out (respect `prefers-reduced-motion` → instant).
- Expanded badges show both icon and label (truncated at 12 chars).
- The orbit radius scales with node size (minimum 32 px from node center).
- Badges are non-interactive in orbit — clicking anywhere on the badge area opens the tag
  assignment panel (same as `T` key).

Implementation: store `badge_expand_t: f32` (0.0 → 1.0) per node in `GraphNodeShape` or a
parallel `HashMap<NodeKey, f32>`. Animate via `ctx.request_repaint()` each frame until 1.0.

#### 1.5 Tab Header Badges

In the detail-view tab bar, each tab header shows a compact badge row to the right of the title:

- At rest: icon-only, up to 2 badges (Crashed + one more).
- Crashed shows as a red dot suffix.
- `#starred` shows ⭐; `#pin` shows 📌.
- User tags: first tag icon only (no overflow — tab headers are narrow).

#### Tasks

- [ ] Define `Badge` and `BadgeIcon` enums in `graph/node.rs` or a new `graph/badge.rs`.
- [ ] Add `fn badges_for_node(node: &Node, workspace_count: usize) -> Vec<Badge>` helper.
- [ ] In `GraphNodeShape::ui()`: compute badges, render at-rest overlay (top-right corner).
- [ ] Add `badge_expand_t: HashMap<NodeKey, f32>` state to `GraphNodeShape` or the egui_adapter.
- [ ] Animate badge expansion on hover: increment `badge_expand_t` each frame, request repaint.
- [ ] Render expanded orbit layout when `badge_expand_t > 0`.
- [ ] Tab bar: render compact badge suffix per tab (existing tab rendering in `gui.rs` or
  `desktop/tab_bar_ui.rs`).
- [ ] In `GraphNodeShape::ui()`: nodes with `TAG_ARCHIVE` render at reduced opacity (0.35–0.45)
  when the "Show archived" graph view toggle is on. Excluded entirely when toggle is off.

#### Validation Tests

- `test_badges_for_pinned_node` — node with `#pin` tag → `badges_for_node` returns `[Pinned]`.
- `test_badges_for_starred_node` — node with `#starred` → `[Starred]`.
- `test_badges_priority_order` — node with `#pin`, `#starred`, `work` tag → order is Pinned,
  Starred, Tag.
- `test_crashed_badge_first` — Crashed + Pinned → Crashed is first.
- `test_at_rest_capped_at_three` — 5 badges → 3 rendered + `+2` overflow chip.

---

### Phase 2: Tag Assignment UI

#### 2.1 Trigger

The tag assignment panel opens when:

- `T` key is pressed with a node selected (graph view).
- Right-click context menu → "Tags…" (graph view).
- A tag chip on an expanded badge is clicked.

In detail view, `T` key targets the focused tab's node.

#### 2.2 Panel Layout

```text
┌─────────────────────────────────────────┐
│  Tags for "Servo Embedder Notes"         │
│  ┌────────────────────────────────────┐ │
│  │ 📌 #pin  ⭐ #starred  🔬 research × │ │
│  └────────────────────────────────────┘ │
│  ┌──────────────────────────────┐  [⊞] │
│  │  Add tag…                    │       │
│  └──────────────────────────────┘       │
│  ── Suggestions ──────────────────────  │
│    #work   #todo   research   todo      │
└─────────────────────────────────────────┘
```

- **Chip row**: existing tags as removable chips. Click ✕ on a chip → emit `UntagNode`.
- **Text field**: type to filter suggestions via nucleo fuzzy matching.
- **`[⊞]` icon button**: opens the full icon picker (§2.3).
- **Suggestions row**: top-5 results from nucleo against `tag_index` keys + emoji names.
- `Enter` or clicking a suggestion: add the tag (emit `TagNode`).
- `Esc`: close panel without changes.

The panel is non-modal, anchored near the node (below or to the right, whichever fits). It closes
automatically if the node is deselected or the user clicks outside.

#### 2.3 Icon Picker

Accessible via the `[⊞]` button in the tag assignment panel or by clicking the icon slot of an
existing user tag chip.

Layout: a 8×N scrollable grid of emoji, with a "Lucide" tab for SVG icons.

```text
┌─ Choose icon ──────────────────────────────┐
│  [Emoji ●]  [Lucide]     🔍 search icons…  │
│  ────────────────────────────────────────  │
│  ⭐ 📌 🔬 🗂 📎 📝 🌐 🔗                    │
│  🏷 📚 🗃 🧪 💡 🔑 🚀 📊                    │
│  …                                         │
│  [Cancel]                         [Select] │
└────────────────────────────────────────────┘
```

- Search field uses nucleo against emoji names (e.g. "bookmark" → 🔖 📑).
- Lucide tab: subset of ~200 most useful icons; search by slug name.
- Selecting an icon updates the tag chip preview in the parent panel before confirming.

#### 2.4 Nucleo Integration

[nucleo](https://github.com/helix-editor/nucleo) is a Rust fuzzy-matching library (MIT).

```rust
// In tag_panel.rs or omnibar.rs
use nucleo::{Matcher, Utf32Str};

fn fuzzy_rank_tags(query: &str, candidates: &[String]) -> Vec<(f32, &String)> {
    let mut matcher = Matcher::new(nucleo::Config::DEFAULT);
    candidates.iter().filter_map(|tag| {
        let score = matcher.fuzzy_match(
            Utf32Str::new(tag, &mut Vec::new()),
            Utf32Str::new(query, &mut Vec::new()),
        );
        score.map(|s| (s as f32, tag))
    })
    .collect()
}
```

The same matcher instance is reused across keystrokes (reset on panel open). Suggestions are
re-ranked on every keystroke.

#### Tasks

- [ ] Add `TagPanelState { node_key, text_input, suggestions, icon_picker_open }` to desktop
  state (in `gui.rs` or a new `desktop/tag_panel.rs`).
- [ ] `T` key in `KeyboardActions`: set `tag_panel_open = Some(selected_node_key)`.
- [ ] `render_tag_panel()`: egui `Window` anchored near node rect; chip row, text field,
  suggestions.
- [ ] On text field change: run nucleo against `tag_index` keys + static emoji name list.
- [ ] On `Enter` / suggestion click: emit `GraphIntent::TagNode { key, tag }`.
- [ ] On chip ✕ click: emit `GraphIntent::UntagNode { key, tag }`.
- [ ] `render_icon_picker()`: scrollable emoji grid + Lucide tab; search via nucleo.
- [ ] On icon selection: associate icon with the pending tag (stored in `TagPanelState`).
- [ ] Add `nucleo` to `Cargo.toml` dependencies.

#### Validation Tests

- `test_tag_panel_opens_on_t_key` — `T` key with node selected → `tag_panel_open == Some(key)`.
- `test_tag_panel_emits_tag_intent` — enter tag text, press Enter → `TagNode` intent emitted.
- `test_tag_panel_emits_untag_on_chip_remove` — click chip ✕ → `UntagNode` intent emitted.
- `test_nucleo_ranks_partial_match` — query "star" against ["#starred", "#work"] → "#starred"
  ranked higher.
- `test_tag_panel_closes_on_esc` — Esc key → `tag_panel_open == None`.

---

### Phase 3: Icon Resources

#### 3.1 Emoji (Primary)

Emoji are rendered via the system font — no asset bundling required. egui renders emoji as text
glyphs when the system font provides them (Windows 11: Segoe UI Emoji; macOS: Apple Color Emoji).

- No compile-time asset cost.
- Searchable via a static `EMOJI_NAMES: &[(&str, &str)]` list (slug → char), e.g.:
  `("star", "⭐")`, `("pin", "📌")`, `("bookmark", "🔖")`.
- The list can be generated from Unicode CLDR data or a curated subset (~500 entries covers
  common use).

#### 3.2 Lucide Icons (Extended)

[Lucide](https://lucide.dev) is an MIT-licensed icon set (~1500 SVG icons) derived from Feather.

Embedding strategy:

```rust
// In a generated icons.rs or icons/ module
pub static LUCIDE_BOOKMARK: &[u8] = include_bytes!("../assets/lucide/bookmark.svg");
pub static LUCIDE_TAG: &[u8] = include_bytes!("../assets/lucide/tag.svg");
// ... curated subset (~200 icons)
```

Rendering via egui `Image::from_bytes()` (egui 0.33 supports SVG via the `svg` feature of
`egui_extras`). Each icon is rasterized at the required size on first use and cached.

**Curated subset criteria:**
- Include: navigation, productivity, media, communication, and science categories.
- Exclude: brand/logo icons, deprecated/redundant aliases.
- Target: ~200 icons in the picker; full set available to advanced users via search.

#### 3.3 Default Tag Icons

Built-in tags have fixed default icons. These are the canonical glyphs shown in badges, the tag
assignment chip, and the icon picker's "system" section. Users cannot reassign system tag icons.

| Tag | Default icon | Badge variant | Notes |
| --- | --- | --- | --- |
| `#pin` | 📌 (Emoji) | `Pinned` | |
| `#starred` | ⭐ (Emoji) | `Starred` | |
| `#unread` | ● blue dot | `Unread` | Rendered as colored dot, not icon chip; auto-managed |
| `#archive` | 🗄 (Emoji) | `Tag` | Node also rendered at reduced opacity |
| `#resident` | 🏠 (Emoji) | `Tag` | Low-visibility; indicates lifecycle protection |
| `#private` | 🔒 (Emoji) | `Tag` | Triggers URL/title redaction in sharing mode |
| `#nohistory` | 🚫 (Emoji) | `Tag` | Behavioral only; badge shown in expanded orbit only |
| `#monitor` | 👁 (Emoji) | `Tag` | Pulses when content change detected |
| `#focus` | 🎯 (Emoji) | `Tag` | Renders with a soft highlight ring on the node |
| `#clip` | ✂️ (Emoji) | `Tag` | Distinct node shape/border in graph view |
| User tags | None (label chip) | `Tag` | User assigns icon via picker |

#### Tasks

- [ ] Create `assets/emoji_names.rs` or embed as a `const` slice in `badge.rs`:
  curated ~500 (slug, char) pairs.
- [ ] Download and add Lucide SVG curated subset to `assets/lucide/`. Add build step or
  `include_bytes!` references.
- [ ] Add `egui_extras` with `svg` feature to `Cargo.toml`.
- [ ] Implement `render_badge_icon(ui: &mut Ui, icon: &BadgeIcon, size: f32)` in `badge.rs`.
- [ ] Implement emoji search in icon picker: nucleo over `EMOJI_NAMES`.
- [ ] Implement Lucide search in icon picker: nucleo over icon slug list.

#### Validation Tests

- `test_emoji_icon_renders_without_panic` — `render_badge_icon` with `BadgeIcon::Emoji("⭐")` →
  no panic (headed test or mock `Ui`).
- `test_emoji_search_finds_star` — nucleo query "star" against emoji names → "⭐" in results.
- `test_lucide_slug_recognized` — `BadgeIcon::Lucide("bookmark")` → bytes available (non-empty).

---

## Findings

### Why Emoji First

Emoji require zero asset bundling, ship with every OS graphshell targets, and are already rendered
by egui's font pipeline. The only cost is the static name list for search. Lucide extends coverage
for users who want a more uniform visual language, but it is an opt-in extension, not a
requirement.

### Orbit Animation and Reduced Motion

The orbit expansion animation adds visual continuity but must not be required for usability.
Badge identity (type + label) must be readable at rest (icon alone suffices for `#pin`, `#starred`,
`Crashed`; user tags fall back to colored dot at rest, full label on hover). The `prefers-reduced-
motion` OS setting (readable from `winit` on macOS/Windows) should skip the animation entirely and
jump to the expanded state.

### Tab-Level Pinning Distinction

The `tile.is_resident` workspace-tile property (prevents webview eviction) is deliberately NOT
represented as a badge. It is a lifecycle management setting, not a semantic annotation — it has
no meaning in the graph view, only in the tab bar. Users do not need to see "this tab is resident"
as a node property; they interact with it via the tab right-click menu.

### `#monitor` Requires a Dedicated Plan

`#monitor` is listed as a reserved tag but its implementation is substantially more complex than
the other system tags (which are pure read-time behaviors). It requires:

- A background scheduling mechanism (periodic wake-up independent of the webview lifecycle).
- A DOM hash comparison strategy (full-page hash vs. main-content-only heuristic).
- A throttle policy (minimum reload interval, backoff on repeated no-change).
- A notification path (badge pulse animation + toast on change detection).

This scope warrants a separate implementation plan before work begins. The tag name `#monitor` is
reserved now so data written before the plan exists is upgrade-compatible. Do not implement
`#monitor` behavioral effects as part of persistence hub Phase 1 — only reserve the constant.

### Nucleo Dependency

Nucleo is the fuzzy matcher used by the Helix editor. It is MIT-licensed, has no unsafe outside of
its SIMD hot path, and is a single-crate dependency. It is already considered for the omnibar
(UX polish plan §5.3). Adding it here to `Cargo.toml` is consistent with that future use.

---

## Progress

### 2026-02-20 — Session 1

- Plan created from the unified tag system design discussion.
- Badge visual system (orbit model, priority slots, tab header badges) designed.
- Tag assignment UI (floating panel, nucleo autocomplete, icon picker) designed.
- Icon system: emoji (primary, zero cost) + Lucide (extended, SVG, MIT) decided.
- Implementation not started.
