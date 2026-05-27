# Hekate lanes + observable planes (cross-engine, for review)

**Status (2026-05-17, revised PM-3):** proposed; lane decomposition tightened to three peer lanes (Nematic, Serval, Scrying). Earlier framings split Serval into "Middlenet/static HTML" and "Serval fullweb" — that was an artificial cut, since "static HTML, no JS" is a *profile tier within Serval*, not a separate engine. The tier decision is Serval-internal; Hekate sees one Serval lane and chooses the profile when handing the document to it.

**Universal framing rule:** observable planes are defined as **snapshots / query surfaces**, not engine-owned structs. Internal storage stays private to each lane; the public ABI is a trait family (`FragmentQuery`, `InteractionQuery`, etc.) that lanes implement and the host consumes. This applies to *every* plane in the vocabulary — not just Fragment and Interaction. Nematic, Serval, and the system-webview lane are free to store their internals differently while mere/apparatus gets stable affordances.

This doc lives in `serval/docs/` because the immediate consumer is the serval-layout architecture, but its scope is **ecosystem-wide** — Nematic, mere/apparatus, and host code all rely on it. Sibling reads:

- [2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md) — serval-layout's piece (Style + Layout + Fragment + Paint planes for HTML).
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md) — the HTML-specific DOM-side contract.

---

## The mistakes this fixes

Earlier docs went through two rounds of correction:

**Round 1 (mid-2026-05-17):** framed Hekate as "three heads of Serval" — extract / middlenet / fullweb, all served by serval-layout. Two category errors:

1. **Made Nematic an alternate path** rather than a first-class engine. Smolweb protocols (Gemini, Scroll, Markdown, etc.) shouldn't route through HTML to reach a renderer. Nematic does protocol-faithful direct rendering. It's a *peer* to Serval, not a special case of it.
2. **Made extract a render lane.** Extraction (readability scoring, classification, cheap facts) isn't a peer engine. It's cross-cutting — it can extract from HTML, Gemini, Markdown, feeds, future PDFs, whatever. Making it a Serval head implies the wrong shape.

**Round 2 (PM-3):** still had four lanes (Nematic, Middlenet/static HTML, Serval fullweb, system-webview). Two more cleanups:

1. **Middlenet was an artificial cut.** "HTML without JS" is a profile tier within Serval, not a separate engine. The profile-ladder plan from 2026-05-12 already had this right: `serval-static-html` / `serval-interactive-html` / `serval-scripted` / `serval-fullweb` are tier crates within Serval, all sharing `serval-layout`. From Hekate's view, there's one Serval lane; Serval picks the tier when given the document.
2. **system-webview was over-abstracted.** We have one implementation library (`scrying`), and it's not a generic system-webview-with-pluggable-backend — it's specifically `scrying`. Make it its own lane named after what it is.

The fix: **three peer lanes** (Nematic, Serval, Scrying). Serval has internal tiering. Hekate routes by *which engine handles this source*; the within-Serval tier is Serval's own choice (informed by Hekate's extract hints).

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
                          ┌───────────────┼───────────────┐
                          ▼               ▼               ▼
                ┌────────────────┐ ┌──────────────┐ ┌──────────────┐
                │    Nematic     │ │    Serval    │ │   Scrying    │
                │ (Gemini/Scroll │ │ (HTML, tiered│ │ (fallback;   │
                │  /Markdown/    │ │  internally: │ │  wgpu-scry/  │
                │   feeds/...)   │ │   static →   │ │   scrying    │
                │                │ │   +CSS →     │ │   system     │
                │                │ │   +JS →      │ │   webview)   │
                │                │ │   fullweb)   │ │              │
                └────────────────┘ └──────────────┘ └──────────────┘
                          │               │               │
                          └───────┬───────┴───────────────┘
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

### Serval

The HTML/CSS/(JS) lane. Internally tiered — `serval-layout` is shared across all tiers; the tier crates wrap it with the right DOM provider and capability set.

- **Tiers** (per the [2026-05-12 profile-ladder plan](./2026-05-12_serval_profile_ladder_plan.md)):
  - `serval-static-html` — HTML parsing → `StaticDocument` → `serval-layout` planes → Paint. No JS. No incremental invalidation. The "reader-mode-shaped" tier — fast, audit-clean, the static-profile target.
  - `serval-interactive-html` — adds form/input/focus/accessibility hooks. Still no JS.
  - `serval-scripted` — adds the scripted-DOM provider, JS engine, DOM mutation. Incremental invalidation lit up.
  - `serval-fullweb` — adds browser APIs (storage, workers, WebGL, etc.).
- **Tier selection:** Hekate hands Serval the document plus a tier *hint* (informed by extract: noscript content available? app-shell-shaped? domain memory says JS is needed?). Serval picks the tier to instantiate. The tier choice can re-escalate mid-session if the static tier proves insufficient (link click that needs JS, etc.) — Serval reports back to Hekate, which records the re-escalation as route-hint evidence.
- **From Hekate's view:** one Serval lane. The tier is Serval's internal concern.
- **What it publishes:** Source/Semantic + Style + Layout/Fragment + Paint + Interaction + Loading planes. (Scripted+ tiers also publish mutation/invalidation events.)

The tier ladder also matters for the audit canary: `serval-static-html` and `serval-interactive-html` carry **no JS engine at all** (no `script-engine-*`, no `nova_vm`, no `mozjs`); `serval-scripted` and `serval-fullweb` are where the engine lives.

**Updated 2026-05-21:** the primary engine is now **Nova** (pure-Rust, wasm-clean), not SpiderMonkey — see [script-engine plan Part 6](./2026-05-20_serval_script_engine_plan.md). The no-engine tiers stay engine-free for **attack-surface + bundle-size + DOM-as-library** reasons, *not* wasm-safety: Nova compiles to wasm too, so the wasm *target* is now orthogonal to the capability *tier*. SpiderMonkey, if it ever returns, is a native-fullweb-only backend.

### Scrying

System-webview fallback for content the above lanes can't faithfully handle (origin-locked sites with hostile JS, DRM media, complex enterprise web apps that hard-require a real browser, situations where rendering fidelity matters more than transparency).

**Lane name:** `Scrying` in docs, `ScryingLane` in code. Backed by the `scrying` crate (`repos/wgpu-scry/scrying`), our wgpu-integrated system-webview wrapper. Not abstracted as "system-webview-with-pluggable-backend" — we have one implementation and Scrying *is* the lane. If a second system-webview backend ever becomes load-bearing, that's a sibling lane (e.g., `Wry`-the-lane), not a Scrying-internal swap.

**Degenerate observable matrix** (explicit, so the host knows what it can rely on):

| Plane | Provided | Notes |
| --- | --- | --- |
| Source/Semantic | URL + title-ish metadata only | The webview is opaque to us; we can't traverse its DOM. |
| Style | none | n/a |
| Layout/Fragment | one opaque root rect | The whole webview is a single fragment matching the window/viewport. |
| Paint | external/native texture | scrying paints itself; the lane returns a paint handle (wgpu texture via scrying's compositing) we composite. |
| Interaction | mostly scrying-owned | Pointer/keyboard events route into scrying, not our InteractionPlane. The lane reports back focus/selection at the granularity scrying exposes. |
| Loading | real, if scrying exposes it | The lane translates scrying's loading events into the shared LoadingPlane vocabulary. |

This avoids host special-cases (mere doesn't need a separate code path for "is this a webview") while telling apparatus *"you can't inspect inside this thing"* via the explicit none/opaque slots in the matrix.

**The matrix is a ceiling, not a floor.** [2026-05-26_scrying_dom_bridge.md](./2026-05-26_scrying_dom_bridge.md) proposes lifting the Source/Semantic, Fragment, and Interaction rows out of degenerate via a page-injected `web-sys`-shaped DOM bridge — a low-frequency introspection/control channel separate from scrying's existing zero-copy pixel path. It feeds the `SemanticQuery` / `FragmentQuery` / `InteractionQuery` traits this doc defines, carries a page-attested provenance flag (lower trust than a first-party DOM), and unblocks a11y + graph ingestion for the webview lane.

---

## Observable plane vocabulary (cross-engine)

Each lane implements a subset of this vocabulary. The host (mere/apparatus/inker) consumes the same shape regardless of which lane produced it.

### Source/Semantic Plane

Source spans, protocol nodes, links, headings, roles, language, document title, anchors.

- **Nematic:** the parsed `SemanticDocument` (gemtext blocks, markdown nodes, feed entries).
- **Serval:** the DOM tree (`LayoutDom` view).
- **Scrying:** the URL + protocol metadata only (the webview is opaque).

**Per Mark's correction:** don't force Nematic's `SemanticDocument` and Serval's DOM into one fake tree model. Use **common-minimum query trait + engine-specific extensions**. The common minimum is *facts the host can index/search/preview uniformly*; extensions let apparatus inspect native protocol shape.

```rust
/// Common minimum every lane that has a document publishes.
pub trait SemanticQuery {
    type NodeId: Copy + Eq + Hash;

    fn title(&self) -> Option<&str>;
    fn language(&self) -> Option<&Lang>;
    fn headings(&self) -> Box<dyn Iterator<Item = HeadingInfo> + '_>;
    fn links(&self) -> Box<dyn Iterator<Item = LinkInfo> + '_>;
    fn anchors(&self) -> Box<dyn Iterator<Item = AnchorInfo> + '_>;
    fn nodes_by_role(&self, role: SemanticRole)
        -> Box<dyn Iterator<Item = Self::NodeId> + '_>;
    fn text_range(&self, node: Self::NodeId) -> Option<&str>;
    fn source_range(&self, node: Self::NodeId) -> Option<SourceRange>;
}

/// Serval-specific extension: DOM-ish element/query details.
pub trait HtmlSemanticExt: SemanticQuery {
    fn element_name(&self, node: Self::NodeId) -> Option<&QualName>;
    fn attribute(&self, node: Self::NodeId, ns: &Namespace, local: &LocalName)
        -> Option<&str>;
    fn query_selector(&self, selector: &str) -> Result<Option<Self::NodeId>, ParseError>;
    fn query_selector_all(&self, selector: &str)
        -> Result<Box<dyn Iterator<Item = Self::NodeId> + '_>, ParseError>;
}

/// Nematic extension: protocol block types, Scroll/Gemini-specific structure.
pub trait NematicSemanticExt: SemanticQuery {
    fn block_kind(&self, node: Self::NodeId) -> Option<NematicBlockKind>;
    // Scroll-specific frontmatter, Gemini preformatted-block alt-text, etc.
}

/// Feed-specific extension (RSS/Atom).
pub trait FeedSemanticExt: SemanticQuery {
    fn entries(&self) -> Box<dyn Iterator<Item = EntryInfo> + '_>;
    fn authors(&self) -> Box<dyn Iterator<Item = AuthorInfo> + '_>;
}
```

Hekate's indexing/search/preview pipeline consumes `SemanticQuery` only — uniform across lanes. Apparatus's inspector pane can downcast/dispatch to the lane-specific extension when present (`if let Some(html) = doc.as_html_ext() { ... }`), showing protocol-native shape when meaningful.

### Style Plane (optional, style-capable formats only)

Computed style, atomized id/class, selector flags, inline-style cache.

- **Nematic:** not implemented (Gemini/Scroll/etc. have no CSS).
- **Serval:** Stylo-computed `ElementData` keyed by `D::NodeId`.
- **Scrying:** not implemented.

What's queryable:

- `computed_style(node) -> Option<&ComputedValues>` (Stylo value type; the only Stylo type that crosses the firewall — see planes architecture doc)
- `visibility(node) -> Visibility` (visible / hidden / collapse)
- `display_kind(node) -> DisplayKind`

### Layout/Fragment Plane

Boxes, rects, scroll containers, text runs, hit targets, line boxes.

- **Nematic:** simple text-flow fragments (Gemini line + indent levels become rectangles).
- **Serval:** Taffy-computed boxes + parley line boxes (FragmentPlane in the planes doc).
- **Scrying:** opaque; one root fragment matching the window rect.

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

Display list / render scene. **See [2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md) (revised PM-2) for the full design.**

Briefly: paint has three distinct layers — producer-facing `PaintList` trait (what engines emit), transport-friendly wire payload (the same `PaintList`, since it's `Serialize`), and renderer-private `netrender::Scene` (NetRender owns lowering). Common-minimum vocabulary + engine-specific extensions, **mirroring SemanticQuery's pattern**.

Extensions are **typed serializable payloads** per engine (`ServalPaintExt`, `NematicPaintExt`), not callbacks. NetRender has a registered renderer for each engine's extension variants and lowers them into its internal scene. This keeps paint transportable (no `dyn` across IPC), capture/replay-able, tile-cacheable.

- **Nematic:** `NematicPaintList: impl PaintList`. Common items only initially (text, rect, stroke, gradient, image). `NematicPaintExt` empty until protocol-shaped items earn a slot.
- **Serval:** `ServalPaintList: impl PaintList` (renamed from `ServalDisplayList`). Common items for most box content; `ServalPaintExt` variants for paint worklets, mix-blend-mode regions, masks, native form controls.
- **Scrying:** `ScryingPaintList: impl PaintList` with one `DrawExternalTexture(scrying_texture)` command. NetRender composites natively.

Common vocabulary includes gradients (linear/radial/conic), shadows, text runs, strokes — anything NetRender already renders natively. Graduation rule: if NetRender supports the primitive, it's common; engine extensions are reserved for items the renderer doesn't natively know.

Text shaping happens in serval-layout (parley) — paint carries shaped glyph runs. NetRender owns font registration + glyph cache + scene emission, **does not reshape**. This keeps layout and paint from drifting.

Direct Vello access is an **escape hatch outside the PaintList pipeline**: lanes that want it use Vello directly and hand NetRender the resulting texture via `DrawExternalTexture` (what Scrying already does). The pipeline doesn't force a callback-into-Vello model.

NetRender lives on its own terms in its own crate, knowing nothing about Serval / Nematic / Scrying internals. Each pairing (engine + NetRender) delivers real value via the shared trait surface.

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

#### Lifecycle: lane handle resolved once, queried per-event

Hekate is the routing/session authority, **not** a pointer-event broker. Mere does not ask Hekate on every input event — the lane is resolved when the route/session is created, and mere stores a lane handle per tile/session.

Hot path:

```text
mere input event
  → active tile/session (already holds the lane handle)
  → lane.interaction.affordances_at(point) / activation_target(point)
  → lane.command(...) or lane.event(...) (e.g., "click link", "extend selection")
  → updated observable snapshot
  → mere redraws, re-queries display list / fragment plane via generation_id
```

The Hekate touchpoint is at session creation:

```rust
let route_decision = hekate.route(url_or_source);
let lane_handle = route_decision.spawn();   // returns Box<dyn Lane> or similar
tile.set_lane(lane_handle);                 // mere stores it
```

Once `tile.lane` is set, mere never asks Hekate about it again until the session ends or a navigation occurs. This keeps Hekate off the hot path and Hekate's role coherent: routing authority, not event broker.

### Loading/Network Plane (added 2026-05-17)

Request state, redirects, MIME/content-type, TLS/security summary, download progress, cache hit/miss, protocol errors, final source identity. Matters for *all* lanes including Nematic and Scrying — it's route/session evidence, not render data.

**Ownership boundary:** lanes (or their protocol/network adapters) **emit** loading events; Hekate **records** the normalized LoadingPlane snapshot; the host **displays** status/errors/progress.

```text
network adapter / lane     →    Hekate                    →    host
emits LoadingEvents              records LoadingPlane            displays status
                                 snapshot per session            errors, progress,
                                                                 TLS badge
```

Query surface:

```rust
pub trait LoadingQuery {
    fn state(&self) -> LoadingState;            // Pending / InProgress / Done / Failed
    fn progress(&self) -> Option<LoadProgress>; // bytes_received / bytes_total
    fn final_url(&self) -> Option<&Url>;        // after redirects
    fn redirect_chain(&self) -> &[Url];
    fn mime(&self) -> Option<&str>;
    fn tls_summary(&self) -> Option<TlsSummary>;
    fn cache_origin(&self) -> CacheOrigin;      // CacheHit / CacheMiss / NotCacheable
    fn error(&self) -> Option<&LoadError>;      // protocol/network/cert errors
}
```

LoadingPlane is read primarily by mere's chrome (URL bar, security indicator, loading spinner) and by Hekate itself (route hints + extract decisions can depend on MIME/protocol). Apparatus reads it for network debugging.

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

### E3/E4 escalation signals (for future Hekate)

Implementation deferred — Hekate starts with E0–E2 only and escalates by hand-tuned heuristics — but the **signal classes** are defined now so the future hook has a known shape:

- **Low confidence:** E2 readability/main-content score uncertain (close to threshold, multiple plausible main-content candidates).
- **Contradiction:** title, headings, and text density disagree about what's "the article."
- **CSS suspicion:** evidence that visibility/display affects what counts as visible (heavy `display:none` use, `hidden` attribute, CSS-only collapse patterns).
- **User action:** user opens reader mode and it looks wrong, user clicks "show original," selection target absent from extracted view.
- **Domain memory:** Hekate has learned that prior pages from this origin needed style/layout-assisted extraction.
- **Security/content hint:** noscript-heavy, app-shell HTML (empty `<div id="root">`), paywall-ish patterns, SPA skeleton with no real content in the initial HTML.

These don't get heuristic implementations now — just enum variants in a `ExtractEscalationReason` type so the API records *why* it escalated, even if the decision is initially manual or per-domain configured.

### Extract result storage

Durable extract artifacts belong in **eidetic** (mere's content store; cf. eidetic's current crate scope — caches, typed payloads, schema engrams, traversal/log memory, content-addressed artifacts).

**Boundary** (don't make eidetic decide routes):

```text
Hekate: live extraction/session cache + routing decisions
eidetic: durable typed extract records, indexes, route hints, source snapshots
```

Hekate is the producer + active cache; eidetic is the durable substrate. Route decisions stay in Hekate's hands — eidetic stores route-hint *evidence* (e.g., "extracts from this origin escalated to E3 12 times in the last week") that Hekate's router consumes, but Hekate makes the call.

This is phrased as *consistent with eidetic's current crate scope*, not *roadmap-confirmed* — eidetic's roadmap wasn't deep-scanned during this decision. If eidetic later picks a narrower scope, Hekate may need its own persistent substrate.

---

## What this means for each repo

### serval-layout

- Drops the `extract.rs` module I had in the planes doc's module layout. Extract is Hekate's, not Serval's.
- Implements the `FragmentQuery` + `InteractionQuery` traits over its FragmentPlane + StylePlane data. The internal plane storage stays serval-layout's implementation detail.
- Provides `Serval`'s impl of `ExtractCapableLane` (structure / style-assisted / layout-assisted).
- Lives as one of several lanes; doesn't need to know about Hekate's routing decisions or other lanes.

### serval-static-html / serval-interactive-html / serval-scripted / serval-fullweb (tier crates)

- These are Serval's profile-tier crates. From Hekate's view, they're all "Serval" — Hekate picks which tier when handing the document over.
- All wrap `serval-layout` + a tier-appropriate DOM provider.
- Static and interactive tiers carry no JS engine — no `script-engine-*` / `nova_vm` / `mozjs` (audit-canary load-bearing; rationale updated 2026-05-21 to attack-surface/bundle-size, not wasm — see the Serval-lane note above).
- Publish observables via the trait API; tier just controls what's enabled (e.g., scripted+ adds mutation/invalidation events).

### nematic

- Implements its own observable planes (Source/Semantic + Fragment + Paint).
- No Stylo, no Style Plane.
- Implements `ExtractCapableLane::extract_structure` cheaply; returns None for E3/E4.
- Direct-rendering pipeline: protocol parser → SemanticDocument → text-flow fragments → simple display list.

### Scrying lane wrapper

- Whatever crate wraps `scrying` (`repos/wgpu-scry/scrying`) into the lane trait family. Probably lives in mere alongside hekate, since it's a host-side concern (the lane bridges into host compositing).
- Implements the cross-engine observable traits with the degenerate matrix.
- Publishes the wgpu texture handle for composition into the host's render scene.

### Hekate (new crate)

- Lives at `repos/mere/components/hekate/` (decided 2026-05-17 — just `hekate`, not `mere-hekate`).
- Owns: source sniffing, capability detection, route choice (between Nematic / Serval / Scrying), extract tiers (E0–E2 directly, escalates E3/E4 to lanes), observables cache/index. For Serval, also passes a tier hint (informed by extract).
- Calls into lanes via the trait API. Doesn't render. Doesn't parse HTML or Gemini itself (lanes own their parsers).
- Apparatus reads from Hekate's observables cache for the inspector view.

### mere / mere-host / apparatus

- The host consumes the observable-plane traits (`FragmentQuery`, `InteractionQuery`, `SemanticQuery`, `LoadingQuery`, `ExtractCapableLane` extras).
- mere doesn't care whether the engine behind a given route is Nematic, Serval (at whatever tier), or Scrying — observable contracts are the same.
- Apparatus (inspector) reads observable planes via the trait API; works uniformly for HTML pages (Serval, at any tier), Gemini pages (Nematic), or Scrying-rendered pages (with degenerate observables).

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

## Open questions (mostly resolved 2026-05-17)

All six original open questions are now resolved. New open items below.

1. ~~Hekate's crate home.~~ **Resolved:** `repos/mere/components/hekate/`.
2. ~~Source/Semantic plane shape.~~ **Resolved:** common-minimum `SemanticQuery` trait + engine-specific extensions (`HtmlSemanticExt`, `NematicSemanticExt`, `FeedSemanticExt`). See section above for the trait sketch.
3. ~~InteractionPlane lifecycle.~~ **Resolved:** Hekate resolves the lane at route/session creation; mere stores the lane handle per tile and queries `InteractionQuery` directly on input. Hekate is *not* on the per-event hot path.
4. ~~system-webview as a lane.~~ **Resolved (and revised PM-3):** Scrying is its own peer lane (not abstracted as "system-webview-with-pluggable-backend"). Treat-as-lane with explicit degenerate-observables matrix. Avoids host special-cases.
5. ~~Extract result storage.~~ **Resolved (with caveat):** durable extracts go to eidetic, consistent with eidetic's current crate scope. Boundary: Hekate owns live extraction/session cache + routing; eidetic owns durable typed extract records, indexes, route hints, source snapshots. Don't make eidetic decide routes. Caveat: not roadmap-confirmed against eidetic, just consistent with its current scope.
6. ~~E3/E4 triggers.~~ **Resolved:** implementation deferred, but six signal classes defined (low confidence, contradiction, CSS suspicion, user action, domain memory, security/content hint) so the future hook has a known shape.

Still open:

- **Cross-engine vocab crate location.** Where do `SemanticQuery`, `FragmentQuery`, `InteractionQuery`, `LoadingQuery` live? Lean **A**: new `engine_observables_api` crate in serval/components/shared/. Lets nematic, serval-layout, mere/hekate all depend without cross-repo dep awkwardness. (Alternatives: in mere; serval-traits with mere re-export.) Decide at implementation time; easy to move once both sides actually consume it.
- **Composition shape vs nematic's existing design.** Nematic was previously scoped before the observable-planes framing arrived. If nematic's current internal design assumes a non-engine-observable shape, the trait family above needs reconciling. Worth a quick read of nematic's existing design docs before the engine_observables_api crate is stood up.
- **Loading/Network plane: which lane owns the network adapter?** Most likely the lane spawns its own network adapter (Nematic for gemini://, Serval for https://, Scrying when Hekate routes there). But shared HTTP cache, cookie jar, TLS config across lanes is a real concern. Probably solved by a host-owned "network broker" service that lanes use. Defer until a second lane needs the same shared resource.

---

## Review checklist

- [ ] Are the **Source/Semantic extension traits** the right cut (`HtmlSemanticExt`, `NematicSemanticExt`, `FeedSemanticExt`), or do we want finer-grained categories? (Candidate addition: `MarkdownSemanticExt` for markdown-specific block types if `NematicSemanticExt` is too coarse.)
- [ ] Is the **LoadingPlane query surface** complete? Missing items I can think of: HTTP status code (separate from `LoadError`), response headers (for content inspection), websocket/SSE/streaming state (for live-updating sources).
- [ ] Is the **session/lane handle abstraction** in mere actually a real existing concept, or do I need to flag it as a new mere-side concept? Sketched as `tile.set_lane(lane_handle)` — verify against mere's panel/session existing model.
- [ ] Is the **eidetic boundary** right? "Hekate owns live cache + decisions; eidetic owns durable artifacts" feels right but eidetic's actual scope may push back.
- [ ] Where does the **Scrying lane wrapper crate** live? Probably mere-side (alongside hekate) since it bridges into host compositing — but could be its own crate `mere-scrying-lane` or could live inside hekate. Resolve at implementation time.
- [ ] Does Serval's **tier escalation mid-session** need a public protocol with Hekate, or is it a pure-Serval concern that just reports back as an event? Lean event-based (Serval tells Hekate "I had to upgrade tier," Hekate records as route-hint evidence).

---

## Decision log

- **Decided 2026-05-17:** Hekate is a router + document-intelligence layer, not a renderer. **Three peer lanes** (revised PM-3): Nematic, Serval, Scrying. Serval has internal profile tiering (`serval-static-html` / `serval-interactive-html` / `serval-scripted` / `serval-fullweb`) — Hekate picks the tier when handing the document over. Extract is Hekate's own work, with tiers E0–E4 that escalate into lanes only when style/layout are required.
- **Decided 2026-05-17:** Shared observable-plane vocabulary across engines (Source/Semantic, Style, Layout/Fragment, Paint, Interaction, Loading/Network). Each lane implements the subset that applies; host consumes the same trait API regardless of lane.
- **Decided 2026-05-17:** Observable planes are defined as snapshots/query surfaces — `FragmentQuery`, `InteractionQuery`, `SemanticQuery`, `LoadingQuery`. Raw plane storage stays implementation detail of each lane.
- **Decided 2026-05-17:** A11y is built by fusing Source/Semantic Plane + Style Plane + Fragment Plane. Builder lives in mere/apparatus, not in any render lane.
- **Decided 2026-05-17:** Hekate's crate home is `repos/mere/components/hekate/`.
- **Decided 2026-05-17:** Source/Semantic plane = common-minimum `SemanticQuery` + engine extensions. Don't force one fake tree model across lanes.
- **Decided 2026-05-17:** InteractionPlane lifecycle = lane handle resolved at session creation, mere stores it, Hekate is off the per-event hot path.
- **Decided 2026-05-17 (PM-3):** Scrying is its own peer lane, named after the implementation (the `scrying` crate at `repos/wgpu-scry/scrying`). Not abstracted as "system-webview-with-pluggable-backend." If a second system-webview backend ever becomes load-bearing, that's a sibling lane (e.g., `Wry`-the-lane), not a Scrying-internal swap. Lane in code: `ScryingLane`.
- **Decided 2026-05-17:** Loading/Network plane added to vocabulary. Lanes emit loading events; Hekate records normalized snapshot; host displays.
- **Decided 2026-05-17:** E3/E4 escalation signals defined as six classes (low confidence, contradiction, CSS suspicion, user action, domain memory, security/content hint). Implementation deferred; enum variants stand in.
- **Decided 2026-05-17:** Extract storage = eidetic. Boundary: Hekate live + decisions; eidetic durable + indexes. Don't let eidetic decide routes.
- **Open:** cross-engine vocab crate location (lean `engine_observables_api` in serval/shared/); composition reconciliation with nematic's existing internal design; network-adapter ownership when shared resources matter.
