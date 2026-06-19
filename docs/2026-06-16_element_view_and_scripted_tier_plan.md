# Element view + scripted tier plan

**Date:** 2026-06-16. **Parent:** `2026-06-16_serval_layout_roadmap.md` (Thread
2). **Scope:** the external-texture element view and the scripted tier (JS
engines + scripted DOM). This is a secondary plan: both threads are far more
*built* than older docs imply, so the job here is to record true state with
file:line and name the residual work, not to re-derive a design. It is spun out
so the roadmap stays a map.

The headline for both: the serval-layout / serval engine primitives are
**done**. The remaining work is consumer wiring (meerkat) and scripted-DOM
breadth, not engine primitives.

---

## Part A — external-texture element view

### State: done end to end (serval side)

The chain is built and tested across four crates. This is the correction to any
"still needs the element view" framing.

1. **The view** (`components/xilem-serval/src/tags.rs:74-82`).
   `external_texture<State, Action>(key, width, height)` builds an
   `<external-texture>` element carrying the key and a
   `display:block;width…;height…` style. Tested:
   `external_texture_builds_keyed_element` (`tags.rs:115`). The doc-comment
   names all three downstream lanes (constellation actor scene, scrying
   WebView, pelt tile external-content lane).
2. **Box tree** (`box_tree.rs:150` field `external_texture_key: Option<u64>`,
   populated at `box_tree.rs:346` via `construct::external_texture_key_of`). It
   participates in layout as a replaced box (300x150, CSS-overridable).
3. **Paint** (`paint_emit.rs:769-780`): when `external_texture_key` is `Some`,
   push `PaintCmd::DrawExternalTexture(ExternalTextureItem { placement,
   texture_key, opacity })` instead of serval content. Tested:
   `external_texture_element_emits_a_compositor_pass` (`paint_emit.rs:3048`)
   asserts exactly one `DrawExternalTexture` carrying the key and no
   `DrawImage`.
4. **Host composite** (`serval-winit-host/src/lib.rs`): `compose_external_texture`
   on the netrender `Renderer` blits the producer's registered `wgpu::Texture`
   onto the backbuffer.

Corroborated by `archive/2026-06-12_pelt_development_plan.md:275-291`: the element
landed (`a8832e2762a`), pelt-core's `ContentSource` names the
`ExternalTexture(key)` lane, and the surface exposes
`TileFrame::external_tiles = (tile, rect, key)` (`fdfd0b89850`).

`serval-scripted-dom` has no external-texture references, correct by design:
the element is a layout/paint/host concern that rides the normal
`<external-texture key>` element through the standard DOM, not a scripted-DOM
concern.

### Residual: meerkat's render-loop swap (cross-repo, not an engine primitive)

Per `archive/2026-06-12_pelt_development_plan.md:266-301`, V6 (pelt-as-meerkat-pane) is
gated on the meerkat side only. Everything up to the live render swap is done:
`tree_projection -> TileTree` mapping in platen (mere `f0440f1`), the GPU-free
`tile-surface` feature (`0705a366bcb`) consumed by meerkat (mere `e415cfc`),
and `Workbench::to_tile_tree` (mere `6daf2f9`).

The one remaining piece is the meerkat-internal render-loop swap: build the
`TileTree` from the `Workbench`, render the `TileSurface` frame in place of
`WorkbenchScene`, composite each member's actor texture into the
`external_tiles` rects (key maps back to member by low 64 bits), translate the
surface's `TileEvent`s into `Workbench` mutations, re-project. This is a
mere-repo live-render rewrite, verifiable by running meerkat, and is also gated
on mere's window-composition P2+. It belongs to the mere agent, not to
serval-layout.

---

## Part B — scripted tier

### State: the layering is realized, all three engines real

Eight live crates under `components/`: `script-engine-api` (the Layer-0 VM
trait), `script-engine-nova` / `-boa` / `-piccolo` (per-backend impls),
`script-runtime-api` (the browser host surface: `dom.rs` 3259 LOC, `fetch.rs`,
`selector.rs`, `webgl.rs`), `serval-scripted-dom` (the scripted DOM provider),
`serval-scripted` (host-coupled glue).

**Engines:**

- **Nova (native, primary), working.** serval runs a fork ahead of upstream
  (`Code/crates/nova`), not upstream trynova
  (`2026-06-10_nova_conformance_campaign_plan.md:4-6`). Native-only by a hard
  constraint, not a choice: `nova_vm`'s `Value` asserts word-size equality
  (`2026-05-25_js_execution_strategy.md:40-44`), so wasm32 fails to compile.
- **Boa (wasm + conformance oracle), working.** Default backend in tests, the
  gc-reflector proving ground, carries the full ICU4X/Temporal stack.
- **Piccolo (Lua), working as an option module.** Implements the same
  `ScriptEngine` surface plus a coroutine-yield host-promise bridge
  (`2026-06-11_gc_arena_dom_plan.md` G4). Explicitly a modding-Lua option, not
  a third first-party JS substrate.

**Scripted DOM surface** (the key reference-not-duplicate note): the
authoritative inventory is the `script-runtime-api/dom.rs:5-41` module
doc-comment, not the older `2026-05-25_web_platform_api_shared_middle_plan.md`
"single `setText` probe" framing, which the code has overtaken. Real today:
construction/mutation (`createElement(NS)`, `appendChild`, `setAttribute`,
`textContent`, the `ChildNode` mixin), query (`querySelector(All)`, `matches`,
`getElementById`, `getElementsByTagName`, traversal, `TreeWalker`/`NodeIterator`),
the reflected-IDL-attribute layer, `EventTarget` with real
capture/target/bubble, `document.body`/`documentElement`/`head`, document
cloning. Adjacent host surface real: `fetch.rs` (brokered fetch/XHR),
`webgl.rs`, `selector.rs`.

**Event model:** converged and done
(`2026-06-01_event_model_convergence_plan.md`): JS + native
`stopPropagation`/`stopImmediatePropagation` via a shared `Propagation` cell,
cross-path conformance asserted on Boa + Nova; the `dom/events` WPT push took
the suite 66 -> 142 subtests.

**GC / reflector liveness:** done across all three backends
(`2026-06-11_gc_arena_dom_plan.md`, G0-G4): real death-reporting (vendored Boa
`downgrade` + Nova `into_weak_ref` patches, piccolo native), a custom
mark-sweep collector over document roots ∪ host reflector pins, wired into
`Runtime::collect_garbage` with pin-on-mint complete.

**The lynchpin gap is closed.** The older docs named "nothing drives a full-page
`<script>` end to end." pelt V4 closes it (done, 2026-06-12,
`archive/2026-06-12_pelt_development_plan.md:202-232`): `pelt --engine scripted` parses
a loaded page's HTML into a live `ScriptedDom`, runs its inline `<script>`s,
the mutation relayouts and renders, `--js boa|nova` selects the engine, the GC
tick auto-fires at frame cadence, and `gc_soak_bounds_memory` (120 frames x
50-node churn) holds memory bounded. The layout coupling is the live
`IncrementalLayout` path (`serval-scripted/lib.rs:35,44`), with
`relayout_if_dirty` retained as the diff-tested oracle.

### Residual scripted-tier work (the real backlog)

Ranked roughly by leverage toward real scripted pages:

1. **External `<script src>`** loading — **DONE (2026-06-18).** Was the most
   common reason a real scripted page did nothing. `ScriptedDocument` now
   collects every `<script>` in document order as inline-or-external
   (`scripted.rs` `collect_scripts` / `ScriptSource`); `build()` runs them in
   that order, fetching each `src` through the same `ResourceFetcher` the page
   loaded over and resolving relative URLs against the document URL
   (`document::resolve_href`). `parse()` (the fetch-free path) skips externals;
   `load()` fetches them. Verified on Boa (+ Nova under `scripted-nova`):
   `external_script_runs`, `scripts_run_in_document_order` (inline A / external
   B / inline C → console A,B,C), `relative_src_resolves_against_page_url`,
   `missing_external_script_is_skipped`.

   **Script-element semantics — also DONE (2026-06-18):**
   - **`async`/`defer` timing.** Two-phase: parser-blocking scripts (inline;
     external with neither attribute) run in document order; `defer`/`async`
     externals run after that pass (`defer` in document order — the guarantee;
     `async` unordered, document order a faithful realization of the synchronous
     fetch). `async`/`defer` ignored on inline scripts; `async` wins over `defer`.
   - **`<script type>` classification** (`classify_script_type`). Empty / a JS
     MIME essence → classic; `module` → an ECMAScript module; anything else
     (`application/json`, `text/plain`, import maps) → a data block, not executed.
   - **`type=module` execution + cross-module `import`.** A new defaulted
     `ScriptEngine::eval_module(source, base_url, resolve)` (`Ok(None)` = backend
     unsupported, so Nova/piccolo degrade gracefully) overridden on Boa. Module
     scripts (inline or `src`) are **deferred** and run with module scope.
     `import` works: Boa's `Context` carries a `HostModuleLoader` whose
     `load_imported_module` resolves each specifier against the importing module's
     URL and pulls source through the host `resolve` callback (pelt's fetcher,
     WHATWG-`url`-joined), caching by URL so a diamond / cycle loads once. The
     resolver borrows host state for one call, injected as a scoped raw pointer
     (the loader outlives the call). An unresolvable / throwing import rejects the
     module, which is reported and skipped. **Nova** supports modules + imports too
     (its `eval_module` parses with `parse_module`, drives `Agent::run_module`, and
     overrides `HostHooks::load_imported_module` with the same scoped-resolver +
     URL-keyed `Global` cache); verified on both engines. (Two Nova GC soak /
     orphan-reaping failures that passed on Boa were root-caused and **fixed**
     2026-06-19: a per-call `Global` leak in the Nova adapter, since `Global` has
     no `Drop`; `Self::Value` is now a deferred-release `NovaValue` wrapper. See
     `2026-06-19_nova_reflector_global_leak.md`.)
   - **`charset`** — fetched bytes decoded via `encoding_rs` per the attribute
     (default UTF-8). **`integrity`** — Subresource-Integrity: strongest-algorithm
     sha256/384/512 digest checked against the metadata (raw-bytes compare via
     `sha2` + `base64`); a mismatch blocks just that script.

   Verified: `defer_runs_after_parser_blocking`,
   `defer_scripts_run_in_document_order`, `async_runs_after_parser_blocking`,
   `script_type_data_block_is_not_executed`, `module_keeps_classic_siblings_running`,
   and (on **both** Boa and Nova) `module_executes_with_module_scope`,
   `module_runs_after_parser_blocking`,
   `module_imports_dependency`, `module_import_diamond_loads_shared_once`,
   `module_import_fails_gracefully`, `external_module_runs`,
   `external_script_charset_decodes` (ISO-8859-1 → café),
   `integrity_match_runs`, `integrity_mismatch_blocks`. Nova module support (its
   `eval_module` override over `nova_vm`'s module records) is **done** (2026-06-19),
   as is the Nova GC fix that unblocked its soak/orphan tests.
2. **DOM node-type breadth — DONE (verified 2026-06-18; the `dom.rs:39-40`
   "Not yet" note was stale).** `Comment` / `DocumentFragment` (`createComment`
   / `createDocumentFragment`, nodeType 8 / 11), `cloneNode` (shallow + deep),
   and **live** `HTMLCollection`s (`getElementsByTagName` /
   `getElementsByClassName` / `children` — Proxy-backed, re-walked per access, so
   they reflect mutations) are all implemented + tested on Boa and Nova
   (`dom_fragment_clone`, `dom_collections_works` — which asserts the length grows
   after an `appendChild` — and `dom_tokenlist_dataset_works` for
   `classList`/`relList`/`dataset`). The reflected-attribute table carries the
   **tokenlist** kind (`relList`) and, as of 2026-06-18, the **URL** kind
   (`href`/`src`/`action`/`cite`/`poster`/`formAction`, resolved against the
   document base URL via `__resolve_url`; `ScriptedDocument` sets that base from
   the page URL). Verified by `dom_url_reflection_works` +
   `url_attributes_resolve_against_page_url`. Only the `double` reflected kind
   remains (niche).
3. **CSSOM + platform services.** `getComputedStyle`,
   `CSSStyleDeclaration`, `localStorage`, `history`/`location`: catalogued in
   `web_platform_api_shared_middle_plan.md:245-258`, unbuilt.
4. **`web-api-bindgen` codegen.** Planned-only; today's surface is hand-written
   native fns + JS bootstrap. Promote when the hand-written surface gets large
   enough that codegen pays for itself, not before.
5. **Intl/402 on Nova.** Nova binds no ICU4X; the one genuinely non-redundant
   fullweb conformance lift (Temporal failures ride upstream trynova, do not
   duplicate). `2026-06-10_nova_conformance_campaign_plan.md:82-84`.
6. **Event-model long tail.** Per-interface event subclasses (`MouseEvent`
   etc.; `createEvent` returns base `Event`), shadow DOM / `composedPath`
   retargeting, passive scroll-blocking.
7. **weval / wasm-speed path.** Research-note only
   (`2026-05-25_js_execution_strategy.md`), probes removed 2026-06-10. Reopen
   only if Boa-in-wasm speed becomes a measured blocker.

**One hard ceiling, named not solved** (`2026-06-11_gc_arena_dom_plan.md:70-81`):
monotonic `NodeId` minting is an unbounded vector on wasm32 (32-bit `usize`
packed into Stylo's `OpaqueElement`); the mark-sweep frees node memory but
never id space. Long-lived wasm sessions with high node churn eventually
exhaust the id range. Recorded as a constraint, not on the near horizon.

---

## Where these threads touch serval-layout

For the layout engine specifically, both threads are *consumed*, not *built*:

- The element view's only serval-layout surface is the replaced-box field +
  paint pass (Part A items 2-3), both done.
- The scripted tier's only serval-layout surface is `IncrementalLayout`
  (re-exported through `serval-scripted`), the relayout-on-mutation engine,
  also done.

So from the roadmap's seat, neither thread carries open *engine* work. Part A's
open work is meerkat wiring; Part B's is scripted-DOM breadth and external
script loading. Both live outside serval-layout, which is why they are here and
not in the layout roadmap proper.
