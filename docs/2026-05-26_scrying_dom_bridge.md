# Scrying DOM bridge — lifting the degenerate-observable row

**Status (2026-05-26):** proposed; design brief for review. Extends the Scrying
lane section of [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md).
No code yet. The target trait surface already exists
(`components/shared/engine-observables-api`, 562 LOC, four query traits); this
doc is about the *mechanism* that lets the Scrying lane implement it instead of
returning the degenerate matrix.

---

## The problem this fixes

The Hekate doc gives Scrying an explicit **degenerate-observable matrix**: the
system webview is opaque, so the lane returns URL + title for Source/Semantic,
one root rect for Layout/Fragment, an external texture for Paint, and
scrying-granularity Interaction. That honesty is correct, and it is also a
ceiling. While the Scrying lane is degenerate:

- **Apparatus can't inspect** a scryed page (no DOM, no element tree).
- **The graph can't grow** from it. A scryed page contributes one node (its
  address) and a picture; its outbound links, headings, and embedded resources
  are invisible to graph truth. For a graph-of-the-web browser, the
  foreign-engine lane can't participate in the graph beyond its own address.
- **Accessibility is impossible.** The Hekate decision log builds a11y by fusing
  Source/Semantic + Style + Fragment planes in mere/apparatus. Scrying supplies
  none of the structural inputs, so webview content has no a11y at all.
- **Extract bottoms out at E0.** `ExtractCapableLane::extract_structure` (E1)
  needs structure the lane doesn't have.

## The idea (lift the shape, not the code)

`wasm-bindgen-wry` exposes a wry webview's DOM through a `web-sys`-shaped bridge.
It is wry-based and Dioxus-coupled (it "requires a modified Dioxus"), and
scrying's frame producers are bespoke per platform, so **none of its code is
portable to us.** What is portable is the *pattern*: inject a small script shim
into the page that serializes a projection of the DOM and posts it over the
webview's native message channel; reshape it host-side into a `web-sys`-shaped
tree the lane reads.

The reason this works for us where it fails as an app-rendering strategy is a
**two-channel split:**

- **Pixel channel (high frequency, already built).** Rendering never touches the
  bridge. Scrying's producers put frames on the zero-copy GPU path as they do
  today. The marshaling cost that makes `wasm-bindgen-wry` unviable for driving
  a UI does not apply, because we are not driving a UI across the boundary.
- **Introspection/control channel (low frequency, new).** The bridge carries DOM
  structure, selection, and activation requests at human-interaction frequency.
  It is the channel the degenerate matrix is missing, and it is cheap precisely
  because it is not the render path.

So the bridge is the mechanism by which the Scrying lane stops returning the
degenerate matrix and starts implementing the same query traits every other
lane implements.

## Mapping onto the existing trait surface

The bridge feeds the already-defined traits in
`engine-observables-api` almost one-to-one. No new vocabulary is invented; the
Scrying lane just gains a non-degenerate implementation.

| Trait | Method | Fed by the bridge |
|---|---|---|
| `SemanticQuery` | `headings()` / `links()` / `anchors()` / `nodes_by_role()` / `text_range()` | the shim's serialized DOM tree (tag, role, text, href, heading level) |
| `FragmentQuery` | `hit_test(point)` / `box_model()` / `rects_for_selection()` | per-node `getBoundingClientRect` from the shim |
| `FragmentQuery` | `generation_id()` | bumped per snapshot — the invalidation epoch consumers cache against |
| `InteractionQuery` | `selection()` / `affordances_at()` / `activation_target()` | DOM selection + link/button/scrollable detection |

Two of these deserve emphasis:

**The bbox is the channel bridge.** Pairing each DOM node with its on-screen rect
is what correlates the introspection channel with the pixel channel. It turns the
matrix's "one opaque root rect" into real fragments: a click on the texture
hit-tests back to a DOM node, selection maps to screen rects for highlight
painting. Without bboxes the two channels stay disconnected; with them the opaque
texture becomes an inspectable, hit-testable surface.

**`generation_id` already carries the frequency discipline.** SPAs fire DOM
mutations at a high rate. The bridge must be pull-based (host requests a snapshot)
or coarse-debounced, not a `MutationObserver` firehose, or it reintroduces the
marshaling cost it was designed to avoid. `FragmentQuery::generation_id()` is the
natural seam: the bridge takes a snapshot, bumps the epoch, consumers re-query
against the new value. Streaming every mutation is the anti-pattern.

## Trust posture (sharpened by why Scrying exists)

The Scrying lane exists precisely for content the other lanes can't be trusted to
render faithfully: hostile JS, DRM media, enterprise apps that hard-require a real
browser. A page-injected shim runs in that page's JS world. The page can spoof
it, starve it, or block it via CSP, and isolated-world guarantees differ per
platform. So a bridged Scrying observable is **real structure but
page-attestable only** — strictly lower trust than Genet's authoritative DOM,
which we produce ourselves.

The matrix's honesty principle should extend to cover this. Rather than a binary
present/absent cell, Scrying-sourced observables carry a **provenance/confidence
flag** distinguishing "page-attested via bridge" from the authoritative DOM a
first-party engine yields. Apparatus widens what it can show, and it must not let
anything safety-critical (security indicators, permission decisions, identity
surfaces) trust what the bridge reports.

## Where it lives (deferred, with the lane wrapper)

The bridge is part of the Scrying lane wrapper, whose home the Hekate doc leaves
open ("probably mere-side alongside hekate, since it bridges into host
compositing"). The per-platform shim injection + message decode is bespoke per
producer (WebView2 `AddScriptToExecuteOnDocumentCreated` + `postMessage`;
WKWebView `WKUserScript` + `messageHandlers`; WebKitGTK equivalent), matching
scrying's existing per-platform producer split. The host-side shape it normalizes
to is uniform — the `engine-observables-api` traits. That bespoke-producer /
uniform-trait seam is the same one scrying already draws for frames; the bridge
adds a second method alongside `acquire_frame`.

Resolve the wrapper's crate home with the rest of the lane wrapper, not here.

## Relationship to the pixel-path blocker

Mere's separate question — compositing scrying's wgpu texture into a Xilem host
surface — is **orthogonal** to this bridge. The bridge is the low-frequency
control channel; the texture is the pixel channel. Status of the pixel path as of
2026-05-26: masonry has the full external-layer machinery (a widget opts in via
`PaintCtx::set_paint_layer_mode(PaintLayerMode::External)`; the embedder draws
through `AppDriver::composite_external_layers`). The remaining gap is that
Xilem's `MasonryDriver` does not yet forward `composite_external_layers` to
app-controlled state — a small forwarding edit, not a rebuild. Either way, the
DOM bridge does not wait on it: it rides the pixel path that already exists.

## Open questions

- **Snapshot vs delta protocol.** Pull-based full snapshots are simplest and
  safest; coarse deltas reduce payload on large pages. Start pull-based; add
  deltas only if a real page makes snapshots hurt.
- **Shim injection timing + CSP.** Document-start injection vs post-load, and the
  per-platform isolated-world story, affect both reliability and how much the
  page can interfere. Needs a per-producer spike.
- **Where the bbox correlation is computed.** In the shim (ship rects with the
  tree) or host-side (request rects on demand for hit-test). Leaning shim-side
  for the tree, on-demand for fine hit-tests, to keep snapshots small.
- **Provenance flag shape.** A per-plane confidence enum vs a lane-level
  attribute. Likely per-observable, since selection (live, page-reported) and
  link structure (snapshot) differ in freshness.
