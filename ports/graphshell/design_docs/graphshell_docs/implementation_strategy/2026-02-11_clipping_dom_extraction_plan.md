# Clipping DOM Extraction Plan (2026-02-11)

**Data model update (2026-02-20):**
This plan predates the unified tag system, edge traversal model, and universal node content
model. The following table maps old concepts to current architecture:

| Old concept | Current equivalent |
| --- | --- |
| "Create node with clip metadata" | `AddNode` intent + `TagNode { tag: TAG_CLIP }` + `TagNode { tag: TAG_STARRED }` |
| HTML snippet / screenshot storage | Universal Node Content Model §7 (content snapshot on cold node) |
| "Edge from source node to clip node" | `AddEdge` with `EdgePayload { user_asserted: true }` (edge traversal plan) |
| Phase 4 refresh logic | `#monitor` tag — reserved; dedicated implementation plan pending |
| Auto-tag `#unread` | Applied automatically by `apply_intent()` on `AddNode` (existing behavior) |

**Disambiguation:** "Clipping" here means *DOM element capture* (web clipping): the user
right-clicks a page element and saves it as its own graph node. This is unrelated to the
node *culling/dissolution* logic in persistence hub Phase 5 (which uses "clipping" for cold
node removal). These are different features that share a word.

**Proposed new system tag:** `TAG_CLIP = "#clip"` (see §Tag Proposal below). Needs to be
added to the reserved tags table in the persistence hub plan and badge plan once confirmed.

---

## Clipping DOM Extraction Plan

- Goal: Clip a DOM element into its own node with snapshot and metadata.
- Scope: Servo right-click context menu, `AddNode`/`TagNode`/`AddEdge` intent emission,
  cold node with content snapshot.
- Dependencies: Servo inspector APIs (element bounding box), screenshot capture,
  Universal Node Content Model §7 (snapshot storage), `#monitor` plan (refresh).
- Phase 1: DOM selection
  - Enable right-click element selection in Servo webview.
  - Capture CSS selector or DOM path for the target element (used as content locator
    in the node's content snapshot — cross-ref Universal Node Content Model §7).
  - Context menu option "Clip this element" visible on right-click anywhere in webview.
- Phase 2: Snapshot
  - Capture element bounding box and render to image (screenshot crop via Servo's
    compositor pipeline).
  - Store HTML snippet (outerHTML of target element, sanitized) and/or text summary.
  - The clip node starts in Cold state with a content snapshot attached (HTML + image);
    no active renderer needed. Cross-ref Universal Node Content Model §7 for snapshot
    storage format.
- Phase 3: Node and edge creation via intents
  - Emit `AddNode` for clip node (URL = content-addressed identifier or source URL
    fragment; title = first text content or page title + "clip").
  - Emit `TagNode { tag: TAG_CLIP }` — marks node as a clipped element (display badge,
    filter predicate `is:clip`).
  - Emit `TagNode { tag: TAG_STARRED }` — clip = intentional save; treat as bookmarked.
  - `#unread` is auto-applied by `apply_intent()` on `AddNode`; cleared on first
    activation (existing behavior, no extra work needed).
  - Emit `AddEdge` for source node → clip node with `EdgePayload { user_asserted: true }`.
    No traversal entry is created — this is a structural edge, not navigation.
- Phase 4: Refresh logic (deferred — `#monitor`)
  - Refresh when source page changes is the `#monitor` tag use case (background DOM
    hash comparison, change notification path, throttle policy).
  - This phase is a placeholder until the `#monitor` dedicated plan is implemented.
  - Clip nodes tagged `#monitor` would re-fetch and re-snapshot the source element on
    a schedule; change detected → node title gets `[updated]` prefix or `#unread` is
    re-applied.

## Tag Proposal: `#clip`

| Constant | Value | Behavior |
| --- | --- | --- |
| `TAG_CLIP` | `"#clip"` | Node is a clipped DOM element (not a navigated page) |

Badge: `Tag { label: "#clip", icon: BadgeIcon::Emoji("✂️") }` (same slot as other system tags).

Omnibar predicate: `is:clip` (like `is:starred`, `is:pinned`).

Graph behavior: clip nodes render with a distinct node shape or border to distinguish them
from full-page nodes. No graph-view exclusion (unlike `#archive`).

Export behavior: HTML snippet and screenshot included in export; source edge preserved.

This tag needs to be added to:

- Persistence hub plan Phase 1 special tags table
- Badge plan Phase 3.3 default tag icons table

## Validation Tests

- Right-click on text element → clip node created; `tag_index[TAG_CLIP]` contains it.
- Clip node has `#starred` tag; `tag_index[TAG_STARRED]` contains it.
- Clip node has `#unread` tag on creation; cleared when node first transitions to Active.
- Edge from source node to clip node exists; `EdgePayload.user_asserted == true`.
- Clip node persists across restart (Cold node with snapshot; no renderer needed).
- HTML snippet stored with node; rendered in detail view without reloading source page.
- `is:clip` omnibar predicate returns only clipped nodes.

## Outputs

- Context menu integration in Servo webview.
- `TAG_CLIP` constant and tag table additions (persistence hub + badge plans).
- Content snapshot storage path (coordinated with Universal Node Content Model §7).

## Findings

- (See data model update note at top of file.)
- Clip nodes are permanently Cold unless the user opens them; the content snapshot is
  the primary view, not a loaded renderer. This aligns with the content model vision
  (node = content container, renderer = optional view layer).
- Phase 4 refresh and the `#monitor` tag are the same feature. Deferral is correct —
  the monitor background scheduler is non-trivial and out of scope for this cycle.

## Progress

- 2026-02-11: Plan created.
- 2026-02-20: Aligned with unified tag system (`#clip`, `#starred`, `#unread` intents),
  edge traversal model (`EdgePayload { user_asserted: true }`), Universal Node Content
  Model §7 (snapshot storage), and `#monitor` plan (Phase 4 deferral). Added `#clip`
  tag proposal and disambiguation note (DOM capture ≠ persistence hub Phase 5 culling).
