# Hekate lanes + observable planes (cross-engine, for review)

**Status (2026-05-17):** proposed. Captures Mark's reframing of the three-head architecture: Hekate is a capability router with its own document-intelligence layer, dispatching to engine lanes (Nematic, Middlenet/static HTML, Serval fullweb, perhaps Scry). Each lane publishes a shared observable-plane vocabulary that the host consumes.

This doc lives in `serval/docs/` because the immediate consumer is the serval-layout architecture, but its scope is **ecosystem-wide** — Nematic, mere/apparatus, and host code all rely on it. Sibling reads:

- [2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md) — serval-layout's piece (Style + Layout + Fragment + Paint planes for HTML).
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md) — the HTML-specific DOM-side contract.

---

## The mistake this fixes

Earlier docs (including the planes-architecture doc as initially written) framed Hekate as "three heads of Serval" — extract / middlenet / fullweb, all served by serval-layout. That's a category error in two directions:

1. **It made Nematic an alternate path you keep explaining around** rather than a first-class engine. Smolweb protocols (Gemini, Scroll, Markdown, etc.) shouldn't route through HTML to reach a renderer. Nematic does protocol-faithful direct rendering. It's a *peer* to Serval, not a special case of it.
2. **It made extract a render lane.** Extraction (readability scoring, classification, cheap facts) isn't a peer engine. It's cross-cutting — it can extract from HTML, Gemini, Markdown, feeds, future PDFs, whatever. Making it a Serval head implies the wrong shape.

The fix: Hekate isn't a renderer or a Serval-internal thing. Hekate is a **capability router + document intelligence** layer. Render lanes are downstream of Hekate's routing decision. Extract is part of Hekate's own work, not a render lane.

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────────────────┐
│                              Hekate                                     │
│  • sniffs source / capabilities                                         │
│  • extracts cheap document facts (E0–E4 tiers, see below)               │
│  • chooses render lane                                                  │
│  • stores reusable observables (extract results, route hints, index)    │
└─────────────────────────────────────────────────────────────────────────┘
                                  │
                  ┌───────────────┼───────────────┬───────────────┐
                  ▼               ▼               ▼               ▼
        ┌────────────────┐ ┌────────────┐ ┌──────────────┐ ┌──────────┐
        │    Nematic     │ │ Middlenet  │ │   Serval     │ │   Wry    │
        │ (Gemini/Scroll │ │ (static    │ │  (fullweb,   │ │  (system │
        │  /Markdown/    │ │  HTML,     │ │   HTML +     │ │  webview │
        │   feeds/...)   │ │  no JS)    │ │   JS +       │ │  fallback│
        │                │ │            │ │   browser    │ │  )       │
        │                │ │            │ │   APIs)      │ │          │
        └────────────────┘ └────────────┘ └──────────────┘ └──────────┘
                  │               │               │               │
                  └───────────────┴───────┬───────┴───────────────┘
                                          ▼
                          ┌──────────────────────────────┐
                          │   Observable plane vocab     │
                          │  (shared API to host/mere)   │
                          └──────────────────────────────┘
                                          │
                                          ▼
                                 mere / apparatus / inker
```

**Hekate's job:** "What is this thing, what can we know cheaply, and which engine should handle it?"

**Lane's job:** "Render this faithfully at the selected capability level, and publish observables."

**Host's job:** "Consume observables, present them, route interactions back."

---

## Lanes

### Nematic

Protocol-faithful direct render for smolweb sources where HTML/CSS would be a lossy funnel:

- **Sources:** Gemini, Gopher, Scroll, Spartan, Markdown, RSS/Atom, Finger, Nex, Mercury, Scorpion, Guppy, Molerat, Terse, FSP, SuperText, etc.
- **Pipeline:** source bytes → protocol-specific parser → `SemanticDocument` → text/layout/render observables.
- **What it doesn't do:** HTML at any fidelity, CSS, JS, browser APIs. Those route to Serval.
- **What it publishes:** Source/Semantic Plane + Fragment Plane + Paint Plane (no Style Plane — these formats have no CSS).

Per [project_nematic_scope](../../../.claude/projects/c--Users-mark--Code/memory/project_nematic_scope.md): Nematic = smolweb protocols only, word-processor-faithful.

### Middlenet / static HTML lane

HTML without JS, reader/static rendering. Optionally with style/layout when the source's visibility/structure depends on CSS.

- **Pipeline:** HTML bytes → `serval-static-dom::StaticDocument` → serval-layout planes → Paint.
- **Owner crate:** `serval-static-html` (the profile facade), wrapping `serval-layout` + `serval-static-dom`.
- **What it publishes:** Source/Semantic Plane (the parsed HTML structure) + Style Plane + Layout Plane + Fragment Plane + Paint Plane.

### Serval fullweb

Full browser semantics, HTML + CSS + JS, WebGL, mutation, origin/browser APIs.

- **Pipeline:** scripted-DOM provider → serval-layout planes (incremental invalidation lit up) → Paint.
- **Owner crate:** `serval-fullweb` (placeholder today; lives when the scripted DOM is rebuilt).
- **What it publishes:** all planes, with `invalidate` populated.

### Wry / system webview

Emergency fallback for content the above lanes can't handle (origin-locked sites with hostile JS, DRM media, whatever).

- **What it publishes:** Source plane only, with degenerate fragments (the system webview is opaque to us). No interaction-plane integration with apparatus.

---

## Observable plane vocabulary (cross-engine)

Each lane implements a subset of this vocabulary. The host (mere/apparatus/inker) consumes the same shape regardless of which lane produced it.

### Source/Semantic Plane

Source spans, protocol nodes, links, headings, roles, language, document title, anchors.

- **Nematic:** the parsed `SemanticDocument` (gemtext blocks, markdown nodes, etc.).
- **Serval:** the DOM tree (`LayoutDom` view).
- **Wry:** the URL + protocol metadata only (the system webview is opaque).

What's queryable:

- `nodes_by_role(role) -> Iterator<SourceNodeId>`
- `headings() -> Iterator<HeadingInfo>` (level + text + anchor)
- `links() -> Iterator<LinkInfo>` (href + label + source span)
- `language_of(node) -> Option<Lang>`
- `source_range(node) -> Option<SourceRange>`

### Style Plane (optional, style-capable formats only)

Computed style, atomized id/class, selector flags, inline-style cache.

- **Nematic:** not implemented (Gemini/Scroll/etc. have no CSS).
- **Serval:** Stylo-computed `ElementData` keyed by `D::NodeId`.
- **Wry:** not implemented.

What's queryable:

- `computed_style(node) -> Option<&ComputedValues>` (Stylo value type; the only Stylo type that crosses the firewall — see planes architecture doc)
- `visibility(node) -> Visibility` (visible / hidden / collapse)
- `display_kind(node) -> DisplayKind`

### Layout/Fragment Plane

Boxes, rects, scroll containers, text runs, hit targets, line boxes.

- **Nematic:** simple text-flow fragments (Gemini line + indent levels become rectangles).
- **Serval:** Taffy-computed boxes + parley line boxes (FragmentPlane in the planes doc).
- **Wry:** opaque; one root fragment matching the window rect.

**Query surface** (per Mark's correction — don't expose raw layout internals as a permanent ABI):

```rust
pub trait FragmentQuery {
    type FragmentId: Copy + Eq + Hash;

    /// Generation/epoch — invalidated on any relayout. Consumers cache
    /// against this; the value rolls when the plane regenerates.
    fn generation_id(&self) -> u64;

    /// Hit-test at a viewport point.
    fn hit_test(&self, point: Point) -> Option<FragmentHit>;

    /// CSS box-model for a source node (or fragment).
    fn box_model(&self, source_id: SourceNodeId) -> Option<BoxModel>;

    /// Fragments under a named anchor (e.g., #section-2).
    fn fragments_for_anchor(&self, anchor: &str)
        -> Box<dyn Iterator<Item = Self::FragmentId> + '_>;

    /// Reverse mapping: fragment → source span (for "what's selected").
    fn text_range_for_fragment(&self, fragment: Self::FragmentId)
        -> Option<SourceRange>;

    /// Selection → screen rects (for "where to paint the selection highlight").
    fn rects_for_selection(&self, range: SourceRange) -> Vec<Rect>;
}
```

That's the *permanent ABI*. Internal plane storage (IndexVec / HashMap, Fragment struct shape, etc.) can evolve freely. Consumers (apparatus, host, scroll-to-anchor, selection highlight, `getBoundingClientRect`) speak this trait.

### Paint Plane

Display list / render scene.

- **Nematic:** direct text + simple shape commands.
- **Serval:** `ServalDisplayList` (today; emitted from FragmentPlane + StylePlane).
- **Wry:** opaque (the system webview paints itself).

What's queryable: the display list itself (already a serializable data structure for IPC / NetRender consumption).

### Interaction Plane

Focus, selection, input affordances, activation targets. The bridge between observable geometry and user input.

- **All lanes:** publish current focus, current selection, hit-targets-with-affordances.
- The host (mere) routes pointer/keyboard events into the right lane, gets back updated interaction state.

Query surface:

```rust
pub trait InteractionQuery {
    fn focus_target(&self) -> Option<SourceNodeId>;
    fn selection(&self) -> Option<Selection>;
    fn affordances_at(&self, point: Point) -> Vec<Affordance>;  // link, button, scrollable, etc.
    fn activation_target(&self, point: Point) -> Option<SourceNodeId>;
}
```

---

## Hekate extract tiers (E0–E4)

Extraction has tiers to preserve the zero-layout promise for normal extraction while admitting that some HTML pages need style or layout to know what is actually visible.

| Tier | What | When invoked | Cost |
| --- | --- | --- | --- |
| **E0** | Source metadata: URL, protocol, MIME, title-ish signals | Always, preflight | trivial |
| **E1** | Structural extraction: headings, links, blocks, text, images, source spans | Most routes | parse-time only |
| **E2** | Semantic scoring: readability, main-content confidence, boilerplate score | Reader mode, indexing | parse + simple traversal |
| **E3** | Style-assisted extraction: optional, only if CSS visibility/display matters | Style-heavy pages where E2 alone gets wrong answers | full style cascade |
| **E4** | Layout-assisted extraction: rare, only if geometry affects the result | Pathological cases (visibility-by-layout, CSS-hide tricks) | full style + layout |

Most routes terminate at E1 or E2. E3 and E4 escalate **into the appropriate render lane** — Hekate doesn't run style or layout itself; it asks the lane to do the extraction work, which the lane can do efficiently because it would have run that work for rendering anyway.

So even at E3/E4, extract stays in Hekate's hands as the *requester*; the engine lane does the *computation* on Hekate's behalf and returns observables Hekate caches.

This means Hekate has a small surface against each lane:

```rust
pub trait ExtractCapableLane {
    /// E1 facts the lane can produce cheaply (always).
    fn extract_structure(&self, source: &Source) -> StructureFacts;

    /// E3: optional. None means "this lane can't do style-assisted extraction
    /// without rendering," in which case Hekate must request a full render and
    /// extract from the resulting observables.
    fn extract_with_style(&self, source: &Source) -> Option<StyledFacts>;

    /// E4: optional. Same caveat as E3 but for layout.
    fn extract_with_layout(&self, source: &Source) -> Option<LaidOutFacts>;
}
```

Nematic implements `extract_structure` cheaply (it knows protocol structure); E3/E4 not applicable (no style/layout in smolweb). Serval implements all three (E3 = run cascade only; E4 = run cascade + Taffy).

---

## What this means for each repo

### serval-layout

- Drops the `extract.rs` module I had in the planes doc's module layout. Extract is Hekate's, not Serval's.
- Implements the `FragmentQuery` + `InteractionQuery` traits over its FragmentPlane + StylePlane data. The internal plane storage stays serval-layout's implementation detail.
- Provides `Serval`'s impl of `ExtractCapableLane` (structure / style-assisted / layout-assisted).
- Lives as one of several lanes; doesn't need to know about Hekate's routing decisions or other lanes.

### serval-static-html / serval-fullweb (profile facades)

- These are the "lane entry points" Hekate dispatches to.
- They wrap serval-layout + their respective DOM providers.
- Publish observables via the trait API.

### nematic

- Implements its own observable planes (Source/Semantic + Fragment + Paint).
- No Stylo, no Style Plane.
- Implements `ExtractCapableLane::extract_structure` cheaply; returns None for E3/E4.
- Direct-rendering pipeline: protocol parser → SemanticDocument → text-flow fragments → simple display list.

### Hekate (new crate)

- Lives at `repos/mere/components/hekate/` (decided 2026-05-17 — just `hekate`, not `mere-hekate`).
- Owns: source sniffing, capability detection, route choice, extract tiers (E0–E2 directly, escalates E3/E4 to lanes), observables cache/index.
- Calls into lanes via the trait API. Doesn't render. Doesn't parse HTML or Gemini itself (lanes own their parsers).
- Apparatus reads from Hekate's observables cache for the inspector view.

### mere / mere-host / apparatus

- The host consumes the observable-plane traits (`FragmentQuery`, `InteractionQuery`, `ExtractCapableLane` extras).
- mere doesn't care whether the engine behind a given route is Nematic, Serval, or Wry — observable contracts are the same.
- Apparatus (inspector) reads observable planes via the trait API; works uniformly for HTML pages (Serval), Gemini pages (Nematic), or system-webview pages (Wry, with degenerate observables).

### layout_dom_api

- Stays specific to HTML/serval — it's the DOM-side trait that Stylo cascade and Taffy construction consume. **Not** the cross-engine observable vocabulary.
- The cross-engine vocabulary (the traits in this doc) lives elsewhere. Probably a new crate `engine_observables_api` in serval/components/shared/, OR in mere if the host is the canonical consumer. Decide at implementation time.

---

## Where this doc's vocabulary lives in code

The traits sketched here (`FragmentQuery`, `InteractionQuery`, `ExtractCapableLane`, plus `Source/SemanticDocument`-shaped traits, plus the data types like `FragmentHit`, `BoxModel`, `Affordance`, `Lang`, etc.) need a home.

Options:

- **A. New `engine_observables_api` crate** in `serval/components/shared/`. Stays serval-side; depended on by serval-layout, nematic (when nematic depends on it), mere-host.
- **B. Inside mere/components/shared/`. Stays mere-side; serval-layout depends on mere via published-crate or git dep. More awkward dependency direction.
- **C. Inside `serval-traits` or similar lane-side**, with mere re-exporting. Same shape as A but lane-flavored naming.

Lean **A**. Cross-engine observable contracts have no inherent serval/mere allegiance; putting them in a neutral crate (`engine_observables_api`) under serval/shared/ matches where the early lifting is happening and lets nematic and mere both consume without cross-repo dep awkwardness.

If the answer turns out to be "this belongs in mere," easy to move once both sides actually consume it.

---

## Open questions for review

1. ~~Hekate's crate home.~~ **Resolved 2026-05-17:** `hekate` crate in mere's workspace (`repos/mere/components/hekate/`). Not `mere-hekate` — just `hekate`. Sits alongside the other 11 mere workspace crates.
2. **Source/Semantic plane shape across engines.** Nematic's SemanticDocument and Serval's DOM have different shapes (block-oriented vs tree-oriented). Is there a meaningful common API across them, or do we just have two separate trait families and Hekate dispatches on type? Lean "common minimum trait (headings, links, text spans, source ranges) + engine-specific extensions."
3. **InteractionPlane integration with input routing.** Mere's input event loop needs to know which lane's interaction plane to query for a given point. Hekate's route-choice persists during the session; mere asks Hekate "for this URL/route, which lane is rendering it?" then queries that lane. Confirm this is the loop.
4. **Wry as a lane.** Real lane or escape hatch? If Wry is in the trait family, mere has to handle its degenerate observables (one fragment, no source plane). If Wry is outside the trait family, mere has special-case code for it. Lean treat-as-lane with degenerate observables — uniform consumption beats special-case host code.
5. **Extract result storage.** Hekate stores extracts. Where? Probably eidetic (the content store crate in mere). Confirm against eidetic's roadmap.
6. **E3/E4 trigger.** What signals to Hekate that E2's extraction got it wrong and E3 is warranted? Heuristics? Per-domain rules? User feedback signal ("this reader-mode rendering looks broken")? Defer answer until extract is real.

---

## Review checklist

- [ ] Is the lane decomposition right (Nematic / Middlenet / Serval-fullweb / Wry), or are there missing or over-decomposed lanes? (Candidate addition: a "PDF lane" later. Candidate cut: Wry — does the system webview really need to be in the same vocabulary?)
- [ ] Is the observable-plane vocabulary the right set (Source/Semantic / Style / Layout/Fragment / Paint / Interaction)? Missing plane: **Loading/Network plane** (download progress, error states, redirect chains)? Lean missing — add it.
- [ ] Is `FragmentQuery` the right API surface, or should it split (e.g., a separate `HitTestApi` trait)? Lean keep unified — these queries are tightly related.
- [ ] Extract tiers — are E0–E4 the right cut, or do we need finer granularity? (E.g., split E2 into "readability score" and "structural classification"?)
- [ ] Where does the cross-engine vocabulary crate live? (A / B / C above.)
- [ ] Does this composition shape match nematic's existing direction? If nematic's design has assumptions that conflict (e.g., it already designed itself around a non-engine-observable shape), this needs reconciling.

---

## Decision log

- **Decided 2026-05-17:** Hekate is a router + document-intelligence layer, not a renderer. Lanes are peer engines (Nematic, Middlenet, Serval fullweb, Wry). Extract is Hekate's own work, with tiers E0–E4 that escalate into lanes only when style/layout are required.
- **Decided 2026-05-17:** Shared observable-plane vocabulary across engines (Source/Semantic, Style, Layout/Fragment, Paint, Interaction). Each lane implements the subset that applies; host consumes the same trait API regardless of lane.
- **Decided 2026-05-17:** FragmentPlane has a stable public ABI in the form of `FragmentQuery` (hit-test, box-model, anchor lookup, source-range mapping, selection rects, generation/epoch). Raw plane storage stays implementation-detail.
- **Decided 2026-05-17:** A11y is built by fusing Source/Semantic Plane + Style Plane + Fragment Plane, not from any single plane.
- **Decided 2026-05-17:** Hekate's crate home is `repos/mere/components/hekate/`. Just `hekate` (no `mere-` prefix), sitting alongside mere's other workspace crates.
- **Open:** cross-engine vocab crate location (lean `engine_observables_api` in serval/shared/); system-webview-fallback lane name (Scry? Wry-as-lane?) and whether it's in the trait family or an escape hatch.
