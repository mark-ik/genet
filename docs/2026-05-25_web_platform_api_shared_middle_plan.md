# Web Platform API plan — a shared engine-neutral middle, not a WebIDL→engine monolith

**Status (2026-05-25): proposed; for review.** This is **bucket #2** of the
three-bucket networking/web-API/media triage (bucket #1 = `netfetcher`, planned
in mere; bucket #3 = `net-media`, later). It designs the **web-platform-API
layer** — the "frustratingly deep lake" of DOM / CSSOM / HTML / URL / Encoding /
Storage / Streams interfaces a browser exposes to script — and answers the load-
bearing architectural question directly:

> *Are we recreating Servo's specialized WebIDL→SpiderMonkey approach, or
> building a shared middle that different engines could use before building out
> to the edges of our specific implementation?*

**Answer: a shared middle.** The API *behavior* is implemented **once, in pure
Rust, against genet's engine-neutral DOM** (`layout-dom-api`'s
`LayoutDom`/`LayoutDomMut`); only a thin **binding edge** is per-JS-engine. We
keep WebIDL — but as a **neutral schema** that generates *both* the neutral
surface *and* each engine's edge, not as Servo's codegen that targets one
engine's JSAPI. That inversion is the whole plan.

## Relationship to the other scripting docs (read first)

- [2026-05-20 script-engine plan](./2026-05-20_genet_script_engine_plan.md) —
  owns the **engine axis** (which JS VM: Nova primary native, Boa oracle/wasm)
  and the **reflector mechanism** (`ScriptEngine` / `ScriptEngineLive`,
  `make_reflector` / `reflector_data`, the GC marriage). It explicitly names a
  `script-runtime-api` "browser host surface" layer but leaves its **interior**
  (the interface catalogue + how it's structured) as a stub. **This plan is that
  interior.** It owns the *interface-catalogue axis*, orthogonal to the engine
  axis.
- [2026-05-16 layout_dom_api design](./2026-05-16_layout_dom_api_design.md) — the
  neutral DOM contract the shared middle operates on.
- `netfetcher` plan (mere `mere_docs/implementation_strategy/2026-05-25_netfetcher_plan.md`)
  — the network organ the `fetch()`/`XMLHttpRequest`/`URL`-with-loading
  interfaces bind to. Web-API layer calls netfetcher (via the host) for the
  loading-flavored interfaces; it never links it.

## Where it stands today

The scripted tier has **one** web-API binding: `setText(reflector, text)` in
[genet-scripted/lib.rs](../components/genet-scripted/lib.rs), probe-grade
(thread-local host DOM, single hand-written native fn). That is the seed crystal,
not the structure. The entire catalogue — `document`, `Element`, `Node`,
`querySelector`, `classList`, `addEventListener`, CSSOM, `URL`, `TextEncoder` —
is unbuilt. Building it the way Servo did (772 files, WebIDL→SpiderMonkey
codegen) would re-marry genet to one engine and re-incur the cost the
2026-05-15 SpiderMonkey excision just paid down. This plan builds it so it stays
engine-neutral *by construction*.

---

## 1. The thesis — separate behavior from binding

Servo's `components/script` conflates three things in one codegen pipeline:

1. **Interface schema** — what `Element` *is* (attributes, methods, inheritance).
2. **API behavior** — what `element.classList.add('x')` actually *does* to the
   tree (the algorithm).
3. **Engine binding** — how a SpiderMonkey `JSObject` for that `Element`
   marshals JS args into the behavior and roots the result.

Servo's WebIDL codegen emits (2) and (3) *fused and SpiderMonkey-shaped*. That
fusion is the 772-file tax and the reason the DOM *was* the bindings. Our move:

```text
        ┌──────────────────────────────────────────────────┐
        │  1. SCHEMA   — WebIDL (neutral; one source)        │
        └───────────────────────┬──────────────────────────┘
                                 │  web-api-bindgen (codegen)
              ┌──────────────────┴───────────────────┐
              ▼                                       ▼
  ┌───────────────────────────┐         ┌──────────────────────────────┐
  │ 2. BEHAVIOR (shared middle)│         │ 3. BINDING EDGES (per engine) │
  │  crate: web-api            │         │  web-api-nova / web-api-boa   │
  │  pure Rust over            │         │  thin marshal: JS args →      │
  │  LayoutDom/LayoutDomMut +  │◄────────│  neutral op → JS result;      │
  │  neutral Value abstraction │  calls  │  reflector class per interface│
  │  NO engine dep, NO JS types│         │  engine types confined here   │
  └───────────────────────────┘         └──────────────────────────────┘
```

- **Behavior is written once**, in `web-api`, against the neutral DOM. It never
  names a JS type. `element_class_list_add(dom, node, token)` is the same code
  whether Nova or Boa called it.
- **Binding edges are thin and generated.** Per engine, per interface, codegen
  emits a reflector class whose method bodies are *marshal → call neutral op →
  marshal back*. The hand-written part per engine is only the **marshaling
  primitives** (how this engine turns a JS string into `&str`, how it builds a
  reflector) — proven small already (`ScriptEngine` is 57 lines).
- **A second JS engine costs one adapter crate**, not a re-fork of the DOM. A
  *third* render engine wanting these APIs reuses `web-api` wholesale if it
  speaks `LayoutDomMut`. That is the "shared middle different engines could use."

This is the [script-engine plan's house pattern](./2026-05-20_genet_script_engine_plan.md#part-5--the-modular-pattern-generalized-to-genets-other-big-parts)
applied to the API-surface boundary: `*-api`/behavior crate + per-impl edge
crates + witness-by-package.

---

## 2. Do we keep WebIDL? Yes — as neutral schema, with neutral codegen

Three options were weighed:

| Option | What | Verdict |
|---|---|---|
| **(a) Hand-write everything** | No IDL; hand Rust traits + hand each engine binding | Fine for the first ~dozen interfaces; does **not** scale to the lake (hundreds of interfaces, thousands of members). Rejected as the *whole* strategy; kept as the v0 on-ramp (§5). |
| **(b) IDL-as-neutral-schema + neutral codegen** | WebIDL is the single source; `web-api-bindgen` emits the neutral op trait/dispatch **and** each engine's reflector edge | **Chosen.** Same source of truth Servo uses, but codegen target is *neutral surface + N thin edges*, not one engine's JSAPI. |
| **(c) Borrow Servo's codegen** | Reuse `components/script`'s Python codegen | Rejected — it's SpiderMonkey-fused (emits behavior+binding together, JSAPI-shaped). Borrowing it *is* recreating the monolith. |

So the honest answer to "are we recreating webIDL→spidermonkey?": **we keep
WebIDL, we do not keep the SpiderMonkey fusion.** WebIDL is a good *interface
description language* — it's the schema browsers actually publish. What we reject
is codegen that bakes one engine into the output.

### The bindgen pipeline (illustrative)

- **Parse:** use an existing Rust WebIDL parser (`weedle` / `weedle2` — the
  wasm-bindgen ecosystem's parser; mature, MIT). We do **not** write an IDL
  parser. (OQ §8.1: `weedle` vs a thinner subset parser.)
- **Emit (2) neutral surface:** for each interface, a Rust trait or dispatch
  table of neutral ops keyed by the neutral DOM `NodeId` + a neutral `Value`
  abstraction:

  ```rust
  // GENERATED into web-api — ILLUSTRATIVE, signature-only.
  // From: interface Element { attribute DOMString className; DOMString getAttribute(DOMString); }
  pub trait ElementOps<D: LayoutDomMut> {
      fn get_class_name(dom: &D, node: D::NodeId) -> String;
      fn set_class_name(dom: &mut D, node: D::NodeId, value: &str);
      fn get_attribute(dom: &D, node: D::NodeId, name: &str) -> Option<String>;
  }
  ```

  The trait *signatures* are generated; the *bodies* are hand-written behavior
  (or delegate to `LayoutDomMut` / Stylo / `selectors`). Codegen never invents
  algorithm logic — it gives behavior a typed home.
- **Emit (3) per-engine edge:** for each interface × engine, a reflector class +
  marshaling method bodies:

  ```rust
  // GENERATED into web-api-nova — ILLUSTRATIVE, signature-only.
  fn element_getAttribute<'gc>(agent: &mut Agent, this: Value, args: ArgumentsList, gc: GcScope<'gc,'_>)
      -> JsResult<'gc, Value<'gc>> {
      let node = reflector_node(agent, this)?;            // edge: JS this → NodeId
      let name = arg_to_string(agent, &args, 0, gc)?;     // edge: JS arg → &str
      let out = with_host_dom(|dom| Element::get_attribute(dom, node, &name)); // SHARED MIDDLE
      Ok(out.map(|s| js_string(agent, &s)).unwrap_or(Value::Null))             // edge: result → JS
  }
  ```

  Everything `reflector_node` / `arg_to_string` / `js_string` is the small
  hand-written **marshaling primitive set** per engine (~one module, reused
  across all interfaces). The per-interface edge is mechanical and generated.

### What codegen does *not* do

- It does not generate behavior. Behavior is hand-written Rust in `web-api`,
  reviewed, tested, and the same across engines.
- It does not own the event loop, timers, or the host frame integration — those
  are `script-runtime-api`/`genet-scripted` (the host layer), per the
  script-engine plan §OQ2.
- It does not generate the marshaling primitives — those are the hand-written
  per-engine seam (small, already proven by `ScriptEngine`).

---

## 3. Crate layout (witness-by-package)

```text
# Schema + tool
web-idl/                 # vendored WebIDL fragments (curated subset; see §5), pinned
web-api-bindgen          # weedle-based codegen: IDL → neutral surface + per-engine edges (build-time/dev tool)

# Shared middle — the behavior, engine-neutral
web-api                  # neutral op impls over LayoutDom/LayoutDomMut + neutral Value.
                         # NO engine dep, NO browser host-loop. Wraps reusable leaf crates (§4).

# Edges — per JS engine, thin + mostly generated
web-api-nova             # reflector classes + marshaling primitives over nova_vm (native primary)
web-api-boa              # ditto over boa (oracle/wasm) — NOT a workspace member (icu pin, per script-engine plan)

# Host layer (the script-engine plan's script-runtime-api interior consumes this)
#   document/window globals, event loop, timers — assembled in genet-scripted from web-api + an engine edge.
```

Witness gates extend the script-engine plan's: `genet-static-html` /
`-interactive-html` pull **no** `web-api*` crate; a build that links `web-api`
but no `web-api-<engine>` is a valid **headless DOM-ops** target (the
DOM-as-a-library case — `querySelector`/serialize without any JS engine). That
last point matters: because behavior is engine-free, **the web-API algorithms
are usable with zero JS engine present** — reader-mode, extraction, and tests
can call `Element::get_attribute` directly.

---

## 4. Reuse the lake's existing tributaries (don't reimplement algorithms)

The "directly reusable dependencies" — many web-API behaviors are *already*
crates, and genet already pins most of them. The shared middle **wraps**, it
doesn't reimplement:

| Web API | Reuse | Already pinned? |
|---|---|---|
| `URL` / `URLSearchParams` | `url` 2.5 (+ `form_urlencoded`) | ✓ |
| `TextEncoder` / `TextDecoder` | `encoding_rs` | ✓ |
| `fetch` / `XMLHttpRequest` (loading) | **netfetcher** (via host) | netfetcher (planned) |
| CSSOM (`getComputedStyle`, `style`) | **Stylo** `ComputedValues` (read) + cssparser (parse) | ✓ (StylePlane) |
| Selectors (`querySelector(All)`, `matches`) | `selectors` crate (genet-layout already uses it) | ✓ |
| `DOMParser` / `innerHTML` | `html5ever` (genet-static-dom's sink) | ✓ |
| `structuredClone` / JSON | engine-native + `serde_json` where neutral | partial |
| `Blob` / `File` / `FileReader` | `mime` + bytes; minimal | partial |
| `crypto.subtle` (subset) | `aes` / `sha2` / ring (already in net stack) | partial |
| Streams (`ReadableStream`) | hand-written over neutral async; defer | — |

This is why the lake is *deep* but not *bottomless*: the hard algorithm work
(URL parsing, encoding, selector matching, HTML parsing, CSS cascade) is done by
crates genet already depends on. The web-API layer is mostly **interface
shape + glue to existing behavior**, plus genuinely-new behavior for the
tree-mutation and event-dispatch interfaces.

---

## 5. The catalogue, tiered by what frameworks actually touch

Breadth is triaged, not boiled-ocean. The **inventory is empirical**: rakers'
`bootstrap.js` (758 lines, cited in the script-engine plan) is a ready-made
checklist of *what real frameworks reach for* (React's `node.ownerDocument`
delegation, Vue/Angular/Elm's `classList`/`attributes`, `process.env.NODE_ENV`,
sloppy-mode globals). Use it to order the catalogue.

**Tier W0 — core DOM (the 80/20 of framework hydration).** `Node`
(`childNodes`, `parentNode`, `appendChild`, `removeChild`, `textContent`),
`Element` (`getAttribute`/`setAttribute`, `className`/`classList`, `id`,
`tagName`, `innerHTML`, `querySelector`/`querySelectorAll`, `matches`),
`Document` (`getElementById`, `querySelector`, `createElement`, `createTextNode`,
`body`/`documentElement`), `CharacterData`/`Text`. This is the set that turns
prerender + initial hydration from "one `setText`" into "real React/Vue SSR
mount." Maps to WPT `dom/nodes/`.

**Tier W1 — events.** `EventTarget` (`addEventListener`/`removeEventListener`/
`dispatchEvent`), `Event`/`CustomEvent`, capture/bubble/`stopPropagation`/
`preventDefault`. Needs the event-loop driver (host layer, script-engine plan
§OQ2). The first genuinely-interactive capability. Maps to WPT `dom/events/`.

**Tier W2 — CSSOM + reflection breadth.** `getComputedStyle`,
`HTMLElement.style`, `CSSStyleDeclaration`, attribute-reflection IDL attributes
(`href`, `src`, `value`, …), `DOMTokenList` full, `NamedNodeMap`. Reads from
Stylo's `ComputedValues`. Maps to WPT `cssom/`, `html/dom/` reflection.

**Tier W3 — platform services.** `URL`/`URLSearchParams`, `TextEncoder`/
`Decoder`, `fetch`/`XHR` (→ netfetcher), `localStorage`/`sessionStorage`,
`history`/`location` (navigation-coupled), timers (`setTimeout`/rAF — host loop).
Maps to WPT `url/`, `encoding/`, `fetch/`, `webstorage/`.

**Tier W4+ — the deep water (deferred, fullweb).** Streams, Workers,
`structuredClone` graph semantics, IndexedDB, Canvas2D/WebGL/WebGPU contexts,
WebRTC (→ net-media), Service Workers. Each gated behind a real consumer per the
"don't pre-build contract crates" discipline.

The tiers map onto the [profile ladder](./2026-05-12_genet_profile_ladder_plan.md):
W0 makes prerender/scripted-core real; W1–W2 are `genet-scripted` breadth;
W3 straddles scripted/fullweb; W4 is fullweb.

---

## 6. Conformance — the binding axis, made legible

This layer *is* the script-engine plan's **binding-conformance axis** (vs the
engine/test262 axis). The same WPT runner measures it:

- **`dom/nodes/`, `dom/events/`** ← W0/W1. The core signal.
- **`html/dom/` (reflection), `cssom/`** ← W2.
- **`url/`, `encoding/`, `fetch/`, `webstorage/`** ← W3.

Because behavior is engine-neutral, run each suite against **both** Nova and Boa
edges: a test that passes on Boa but fails on Nova is an *engine* gap; a test
that fails on both is a *behavior/binding* bug in `web-api` (our work). That's
the script-engine plan's cross-backend-delta triage, now with the binding layer
that makes it produce numbers. The neutral behavior can *also* be unit-tested
with **no engine at all** (call `Element::get_attribute` directly) — fast,
deterministic, the bulk of `web-api`'s own test suite.

---

## 7. Increment ladder

1. **W0 hand-written, Nova edge, no codegen yet.** Implement core-DOM behavior in
   `web-api` (over `LayoutDomMut`) + hand-write the Nova reflector edge for it.
   Replaces the `setText` probe with real `document`/`Element`/`Node`. Proves the
   behavior/edge split end-to-end and the marshaling-primitive set. *Gate:*
   prerender hydrates a React/Vue SSR fixture to non-empty body.
2. **Stand up `web-api-bindgen`; regenerate W0's edge from IDL.** Validate codegen
   by reproducing the hand-written W0 Nova edge from WebIDL + the hand-written
   behavior bodies. Diff generated-vs-hand-written to prove the codegen is
   faithful. *Gate:* generated W0 edge passes the same fixtures.
3. **W0 Boa edge via codegen.** Second engine for ~free — proves "one adapter
   crate, not a re-fork." Seeds cross-backend WPT triage. *Gate:* `dom/nodes/`
   runs on both backends; delta attributed.
4. **W1 events + the event-loop driver** (host layer). First interactivity.
5. **W2 CSSOM + reflection breadth** (Stylo-read).
6. **W3 platform services** (URL/encoding/storage/fetch→netfetcher).
7. **W4+ on consumer-demand**, each its own plan.

Ordering rule: **W0 by hand first, codegen second.** Don't build the codegen
before there's a hand-written reference for it to reproduce — otherwise the
codegen has no oracle and bakes in guesses. (Mirrors the "ship the impl with the
trait" rule from layout_dom_api.)

---

## 8. Open questions

1. **WebIDL parser — `weedle`/`weedle2` vs a curated subset parser.** weedle is
   mature but parses full WebIDL grammar (incl. features we'll never emit). A
   thinner parser over a curated `.idl` subset might be simpler to target. Lean:
   start with `weedle`, emit only the constructs W0–W3 need.
2. **Neutral `Value` abstraction — how thin.** The edges marshal JS↔neutral; the
   shared middle needs *some* neutral value vocabulary for non-DOM returns
   (numbers, bools, sequences, dictionaries). Is that the `http`-crate-style
   minimal enum, or do most W0 ops return plain Rust types (`String`,
   `Option<NodeId>`, `Vec<NodeId>`) and dodge a neutral `Value` entirely? Lean:
   plain Rust types for W0–W2; introduce a neutral `Value` only where an IDL
   member genuinely returns `any`/union.
3. **Reflector identity / caching.** `document.body === document.body` must hold
   (same JS object for the same node). The edge needs a per-realm
   `NodeId → reflector` cache with correct GC interaction. Where does it live —
   the edge crate, or a shared `web-api-reflector-cache` helper? (Interacts with
   the script-engine plan's GC-marriage analysis.)
4. **Codegen output: trait-per-interface vs free-functions + dispatch table.**
   IDL inheritance (`HTMLElement : Element : Node : EventTarget`) maps awkwardly
   to Rust traits. A flat free-function namespace per interface with explicit
   up-cast helpers may codegen cleaner. Decide at increment 2.
5. **Live binding vs prerender host surface — one catalogue or two.** Prerender
   needs a *faked* DOM host surface (script-engine plan Part 2); live scripting
   needs the *real* one. Are they the same `web-api` behavior over different DOM
   providers (`genet-static-dom`-backed faux vs `genet-scripted-dom`), or two
   surfaces? Lean: one catalogue, two providers — that's the whole point of
   `LayoutDom`/`LayoutDomMut` neutrality.
6. **Attribute reflection generation.** Reflected IDL attributes (`a.href`,
   `img.src`) are a huge, mechanical slice — codegen should emit them from IDL
   `[Reflect]`-style extended attributes automatically. Confirm the curated IDL
   carries enough reflection metadata, or annotate.

---

## 9. Pitfalls

- **Re-fusing behavior and binding.** The instant a `web-api` function names a
  `nova_vm`/`boa` type, the middle stops being shared and we're back to Servo's
  monolith. Engine types live **only** in `web-api-*` edge crates. (Same red line
  as the script-engine plan's "don't put engine types in `layout-dom-api`.")
- **Codegen before an oracle.** Building `web-api-bindgen` before W0 exists by
  hand means the generator has nothing to be checked against. Hand-write W0,
  then make codegen reproduce it.
- **Boiling the lake.** The catalogue is hundreds of interfaces; W0 is dozens of
  members. Ship W0 framework-hydration breadth before chasing `cssom/` long-tail.
  Tier by rakers' empirical "what frameworks touch," not by spec completeness.
- **Reimplementing tributaries.** Don't hand-roll URL parsing / encoding /
  selector matching — wrap `url` / `encoding_rs` / `selectors`. New behavior is
  only the tree-mutation + event-dispatch + reflection glue.
- **Letting the event loop leak into `web-api`.** Timers, microtask pumping, rAF,
  and frame integration are the host layer's (`genet-scripted` /
  `script-runtime-api`), built on the engine's `pump_microtasks`. `web-api`
  behavior is synchronous tree/style ops; it must not own the loop.
- **Forgetting the no-engine consumer.** `web-api` algorithms must stay callable
  with zero JS engine (reader-mode, extraction, unit tests). Any hidden
  dependence on an engine being present breaks the DOM-as-a-library witness.

---

## Findings

- The scripted tier today is a single `setText` probe; the catalogue is
  greenfield — so the shared-middle architecture can be adopted from the first
  real interface, with no monolith to dismantle.
- Most hard web-API *algorithms* are already crates genet pins (`url`,
  `encoding_rs`, `selectors`, `html5ever`, Stylo) — the lake is deep but its
  hardest currents are already bridged; the layer is mostly interface-shape +
  glue.
- The existing `ScriptEngine` (57 lines) + reflector mechanism proves the
  marshaling-primitive seam is small — confirming "one adapter crate per engine"
  is realistic, not aspirational.

## Progress

- **2026-05-25** — plan created. Shared-middle-vs-WebIDL-monolith question
  resolved (keep IDL as neutral schema; codegen emits neutral surface + thin
  per-engine edges). Crate layout, leaf-crate reuse map, empirically-tiered
  catalogue (W0–W4), conformance binding-axis framing, increment ladder (W0 by
  hand → codegen → second engine), open questions, pitfalls. No code yet
  (plan-only). Sequenced *after* W0 demand from prerender/scripted-core; the
  script-engine plan's reflector mechanism is the prerequisite (largely landed).
