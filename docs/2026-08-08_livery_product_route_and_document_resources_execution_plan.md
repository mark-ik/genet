# Livery product route and shared document resources execution plan

**Date:** 2026-08-08

**Status:** scoped. R0 is the first executable gate. This plan stops with an
explicit Pelt Livery route; it does not flip the static or fullweb default.

**Parent:** [Livery fullweb cutover and the servo-* retirement](2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md)

**Product receipt source:** [Pelt presentability execution plan](2026-08-07_pelt_presentability_execution_plan.md)

**Layout authority:** [Buckram CSS layout engine plan](2026-07-26_buckram_css_layout_engine_plan.md)

## Ruling

Livery reaches Pelt before F4 as an explicit, user-selectable engine. The
projection must use the same host-owned document resources as the incumbent
route. A stylesheet or image does not belong to Stylo or Livery merely because
one engine consumes it first.

The shared boundary is a resolved document resource set:

- document base identity;
- ordered inline and linked author stylesheets;
- each stylesheet's own source URL and media condition;
- host-fetched image and font bytes keyed by authored and resolved URL; and
- explicit diagnostics for dependencies the selected engine cannot yet load.

Pelt owns network policy and engine choice. A neutral Genet resource component
owns HTML discovery, URL resolution, ordering, and byte attribution. Livery
owns CSS parsing and cascade. Buckram receives only computed styles and layout
inputs.

## Why this gate moved ahead of F4

The cutover plan originally placed product reachability, runtime selection,
and the default flip together. That allowed all Livery evidence to come from
`genet-wpt` while the shipping port could not instantiate the engine.

The live product seam is narrower and more revealing:

- `components/genet-documents/src/engines.rs` already contains
  `LiverySessionEngine<Fetch>` under the `livery` feature.
- `inker::SessionRegistry` and `ENGINE_GENET_LIVERY` already provide runtime
  identity and an explicit pin.
- no port enables `genet-documents/livery`;
- Pelt's static surface still calls the incumbent convenience path directly;
  and
- the Livery session currently combines host CSS with
  `genet_layout::inline_stylesheets`, so a normal
  `<link rel="stylesheet">` page loses its authored presentation.

The Merely headed smoke from the Pelt plan depends on linked CSS. It is the
first product falsifier for this route, not a decorative screenshot.

## Standards boundary

HTML treats each stylesheet link as its own external resource and evaluates
its `media` condition against the environment. Its URL resolves against the
link processing base. CSS cascade order treats independently linked sheets in
document linking order and imported sheets at the location of their
`@import` rule.

Normative anchors:

- [HTML link processing](https://html.spec.whatwg.org/multipage/semantics.html#the-link-element)
- [HTML stylesheet links](https://html.spec.whatwg.org/multipage/links.html#link-type-stylesheet)
- [CSS cascade order](https://drafts.csswg.org/css-cascade-5/#cascade-order)

The current byte-only `ResourceFetcher` cannot verify HTTP content type,
redirect-final URL, CORS mode, integrity, or response caching metadata. R0-R4
must not claim those behaviors. R5 either enriches the host response contract
or retains each as a named cutover blocker.

## Live ownership defects

1. `components/genet-layout/host_loader.rs` owns generic HTML resource
   discovery even though `genet-layout` is the Stylo retirement target.
2. `LoadedDocument` has the best product resource assembly, but its cache and
   loader are private to the incumbent static session.
3. `LiverySessionEngine` scans already-known CSS for `url(...)` and the DOM
   for `<img>`, but discovers only inline stylesheets.
4. `ports/genet-wpt/src/render.rs` has a separate Livery resource path,
   including fonts. Its success is harness evidence, not product evidence.
5. `genet-scripted` also consumes the generic stylesheet helpers, so moving
   them into `genet-documents` would create the wrong dependency direction.

These are three real consumers. The common resource component is justified;
another copy is not.

## Target contracts

The exact Rust spelling can change in R0. These distinctions may not:

```rust
pub struct ResolvedDocumentResources {
    pub document_url: Option<String>,
    pub stylesheets: Vec<ResolvedStylesheet>,
    pub resources: Vec<ResolvedResource>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

pub struct ResolvedStylesheet {
    pub owner: StylesheetOwner,
    pub source_url: Option<String>,
    pub media: Option<String>,
    pub text: String,
    pub document_order: u64,
}

pub struct ResolvedResource {
    pub kind: ResourceKind,
    pub authored_url: String,
    pub resolved_url: String,
    pub bytes: Vec<u8>,
}
```

Required invariants:

1. Inline and linked sheets share one document-order walk.
2. An inline sheet resolves relative URLs against the document URL; a linked
   sheet resolves them against its own URL.
3. Media remains metadata until the style engine evaluates it. Wrapping
   fetched text in an invented `@media` string is a migration behavior, not
   the target model.
4. Fetch policy, redirects, caches, and byte limits remain host authority.
5. A missing or unsupported dependency is observable. It never becomes an
   empty successful resource.
6. Engine consumers cannot mutate the shared set or fetch ambient URLs behind
   the host's back.

## Execution gates

| Gate | Outcome |
|---|---|
| R0 | neutral resource model and frozen incumbent behavior |
| R1 | shared HTML stylesheet discovery and URL attribution |
| R2 | Livery consumes the shared resource set |
| R3 | Pelt exposes an explicit Livery engine pin |
| R4 | local and headed real-page receipts |
| R5 | cutover-grade imports, response metadata, and dynamic resource updates |

### R0. Model and baseline

Create `components/genet-document-resources` as the engine-neutral owner of
the contracts above. It may depend on `layout-dom-api` and the
`genet-host-api::ResourceFetcher` contract. It must not depend on
`genet-layout`, `genet-livery`, `genet-scripted`, a port, or a concrete
network engine.

Before moving behavior, freeze fixtures for:

- interleaved `<style>` and `<link rel=stylesheet>` order;
- a relative link under a document base;
- one screen and one print media condition;
- a linked sheet with a relative image and font URL; and
- unavailable, invalid UTF-8, and unsupported-scheme resources.

**Receipt:** the new crate has model tests, while the incumbent static Pelt
scene and the Merely headed smoke are byte- or pixel-equivalent to their
accepted G7 receipts.

**Stop:** do not add a second fetch trait. Adapt the existing host byte
contract until R5 proves response metadata is required.

### R1. Shared discovery

Move the DOM walk, `rel` token handling, media capture, and URL attribution
out of `genet-layout/host_loader.rs`. Migrate `LoadedDocument` first. Keep a
temporary `genet-layout` re-export only for consumers that have not moved in
this gate.

The accepted path must preserve document order across inline and linked
sheets and retain each linked sheet's base identity. Fetch-free parses retain
inline sheets and explicitly diagnose that linked resources have no byte
authority.

**Removal receipt:** the incumbent static session no longer obtains generic
stylesheet discovery from `genet-layout` internals.

### R2. Livery consumption

Make `LiverySessionEngine` consume the same `ResolvedDocumentResources`.
Delete its private `url(...)`/`<img>` discovery once equivalent typed resource
inputs reach `LiveryDocument::set_image_resource` and `set_font_resource`.

Livery receives ordered stylesheet records, not concatenated text. `StyleSet`
must preserve owner identity so CSSOM and future invalidation can attribute a
rule to the correct sheet.

`@import` is not currently represented by Livery's `CssRule`. R2 reports
`ImportRulePendingR5` when a leading import is encountered. It does not strip
the rule and present the remaining sheet as complete support.

**Receipt:** the same local document produces the expected authored color,
linked image, linked font, and screen-media result through both static engine
pins. The Livery ledger names every unsupported dependency.

### R3. Pelt engine pin

Add a `livery` Pelt feature that enables `genet-documents/livery` without
removing the incumbent. Register `StaticSessionEngine` and
`LiverySessionEngine` in the product session registry. Extend Pelt's engine
selection with the existing `genet.livery` identity; do not create a second
Livery engine name.

The choice must remain user-configurable at runtime when both engines are in
the binary. File type, URL scheme, or compile-time feature order must not
silently select Livery.

**Receipt:** `pelt --engine livery <fixture>` reaches a
`LiveryDocumentSession`; `--engine static` reaches the incumbent; an
unavailable pin fails with the missing engine id rather than falling back.

### R4. Product projection

Run the explicit Livery route against:

1. a local interleaved inline/linked stylesheet fixture;
2. `https://merelyllc.com` with its accepted parchment and oxblood colors;
3. a table fixture covering separate borders, captions, and one named K4g
   collapsed-border deferral;
4. a page with a linked image and local webfont; and
5. a viewport resize followed by link hit testing and scroll.

Preserve scene snapshots and headed screenshots outside Git, with a small
checked-in receipt recording engine id, viewport, resource identities, frame
count, and diagnostics.

**Stop after R4.** Livery remains opt-in. F0, F3/F4, Buckram, contextual
color, and presentational-hint gaps remain independent gates.

### R5. Cutover-grade resource graph

Before F4, close or explicitly knock out:

- `@import` ordering, media, layer, supports, cycles, and sheet-relative URLs;
- stylesheet response type and redirect-final identity;
- dynamic linked-sheet insertion/removal and media mutation;
- CSSOM owner-sheet identity;
- cache invalidation and resource replacement; and
- host limits for bytes, nesting depth, redirects, and concurrent fetches.

R5 must decide whether `ResourceFetcher` grows a response type. That decision
is made from these consumers and receipts, not from the full Fetch standard in
the abstract.

## Verification ladder

Every behavior-changing gate runs:

```powershell
cargo test -p genet-document-resources --offline
cargo test -p genet-documents --all-features --offline
cargo test -p genet-livery --all-targets --offline
cargo test -p pelt-desktop --all-targets --features livery --offline
cargo clippy -p genet-document-resources -p genet-documents -p genet-livery --no-deps --offline -- -D warnings
cargo fmt --check
git diff --check
```

Headed evidence uses the fixed viewport and bounded frame controls from the
Pelt presentability plan. A Stylo route result is regression evidence only; a
Livery-headed frame is the product proof.

## Stop rules

- Stop if Livery imports `genet-layout` to obtain resource discovery.
- Stop if Pelt grows another HTML/CSS resource walker.
- Stop if stylesheet order is reconstructed after separate inline and linked
  passes.
- Stop if every stylesheet uses the document URL as its resource base.
- Stop if a missing dependency is represented as successful empty bytes.
- Stop if the Pelt feature changes the default engine before F4.
- Stop if WPT-only output is credited as the R4 product receipt.

## Done condition

This plan's first execution slice is done at R4 when Pelt can explicitly run
both static engines from one build, both consume one host-owned resource set,
and the Livery pin presents the real-page fixture with linked CSS and
resources. R5 remains a named F4 prerequisite until every item is built or
accepted as a recorded knockout.
