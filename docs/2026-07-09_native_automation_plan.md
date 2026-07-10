# Native automation: engine-level observe/actuate core with WebDriver adapters

**Date:** 2026-07-09
**Status:** plan, proposed. Findings verified against source 2026-07-09 (see
Progress). Phase 2's step zero has landed (the leaf hook is wired and the
declared contract is now true); phases 1 and 3-5 have no code yet.

Companion to [2026-07-07_chisel_widget_leaf_design.md](./2026-07-07_chisel_widget_leaf_design.md)
(the leaf contract this plan extends with automation semantics) and
[2026-07-05_w3c_mechanism_adoption_plan.md](./2026-07-05_w3c_mechanism_adoption_plan.md)
(the knockout-then-rebuild doctrine this is an instance of). Checked against
mere's [2026-07-07_headed_automation_plan.md](../../mere/design_docs/mere_docs/testing/2026-07-07_headed_automation_plan.md)
(the scenario vocabulary + self-drive mode this core slots beneath) and
[2026-06-09_accesskit_screen_reader_verification.md](../../mere/design_docs/mere_docs/implementation_strategy/2026-06-09_accesskit_screen_reader_verification.md)
(the ActionRequest route this core generalizes; note that checklist is unrun,
so it documents the intended route rather than proving it, see finding 9) and
[2026-07-03_agent_harness_brief.md](../../mere/design_docs/mere_docs/research/2026-07-03_agent_harness_brief.md)
(the agent loop this core feeds a tool ring into, never a parallel channel).

Code samples are illustrative unless marked implementation-ready.

## Goal

Every xilem-serval app (mere/merecat, strophe, isometry, pelt) becomes drivable
and inspectable by a program: find an element semantically, act on it, wait for
the engine to settle, read state back. One engine-level surface serves test
harnesses, off-the-shelf WebDriver clients, and in-process agents alike.

The framing decision, made before this plan: **the native observation/actuation
core is the product; WebDriver classic and BiDi are thin protocol adapters over
it.** Puppeteer and Selenium have their shape because they drive an engine they
do not own, from outside, through a cross-vendor bottleneck. Serval owns the
engine, so the driver lives inside it. Spec endpoints exist for ecosystem
compatibility (thirtyfour, WPT), never as the core's design driver.

## Findings (what already exists)

The survey that motivated this plan found the skeleton largely standing:

1. **Observation surface, already sketched.**
   `components/shared/engine-observables-api/` defines the `SemanticQuery`
   (headings, links, anchors, roles) and `InteractionQuery` (hovered / active /
   focused / selection, plus `Affordance` with kind + label + source node)
   traits, `LoadingState`/`LoadProgress`, stats, fragments. Generic over
   `NodeId`, so every lane (static, scripted, host UI) can publish it. This is
   the native core's read side.

   It is a **contracts** crate and must stay one. `serval-layout`,
   `serval-scripted-dom` and `serval-render` all depend on it and implement its
   traits (`FragmentQuery`, `InteractionState`, `DomArenaStats`), so it sits
   *below* the layout engine. The core needs `serval-layout` for geometry and the
   semantic tree, so housing the core here would close a cycle
   (`engine-observables-api` -> `serval-layout` -> `engine-observables-api`).
   Grow it for read contracts, including the quiescence contract; never for
   mechanism. See "Where the core lives" below.

2. **Element location.** `script-runtime-api/dom/query_traverse.rs` already
   implements querySelector-style traversal. Selector matching also lives in
   serval-layout via stylo's `selectors`.

3. **A unified semantic tree with an actuation loop, in a11y form.**
   `serval-winit-host/src/a11y.rs` (AccessKit bridge): serval-layout's
   `build_subtree` projects the laid-out DOM into AccessKit nodes (roles from
   ARIA `role=` then tag, names from `aria-label` then direct text,
   `aria-checked` toggled state, absolute bounds from the fragment plane) and
   hands back the actionable nodes; the bridge queues `ActionRequest`s the host
   drains and routes back through its own activation paths. This is exactly the
   automation shape: semantic tree out, actions in. Automation and a11y should
   share this projection, not maintain two.

   **Caveat: leaves are not in that tree yet.** `chisel::Leaf::accessibility()`
   is *declared* (`chisel/src/lib.rs`, doc-commented "a knob still announces as
   a slider") and implemented by no leaf, and the `build_subtree` walk never
   calls it. It is an unwired promise, not standing skeleton. Wiring it is
   phase 2 work and must not be assumed by phase 1.

4. **WebDriver seam types survived the knockout, already retargeted.**
   `shared/embedder/webdriver.rs` keeps `WebDriverCommandMsg`,
   `WebDriverScriptCommand`, prompt types, load-status plumbing, and defines
   `WebDriverJSResult = Result<JSValue, JavaScriptEvaluationError>` in serval's
   own script-engine types (not SpiderMonkey's). The `webdriver` handler crate
   (HTTP server, protocol types) is still a pinned workspace dep
   (`Cargo.toml` \[workspace.dependencies\], v0.53).

5. **Scripting is not a blocker.** Servo's `webdriver_server` was deleted
   2026-05-20 because it was coupled to the SpiderMonkey script thread and
   constellation. Serval has since rebuilt its own stack: `serval-static-dom`,
   `serval-scripted-dom`, `script-engine-nova`/`-boa`/`-piccolo` behind
   `script-engine-api`. ExecuteScript backs onto the nova lane.

6. **Host UI is DOM-backed.** xilem-serval builds real elements
   (`element.rs`, `tags.rs`), so app chrome is selector-locatable with zero
   per-app work. Chisel leaf *interiors* are the opaque case; phase 2 handles
   them. Note what is **not** opaque: mere's orrery gnodes are
   transform-positioned DOM divs, not leaf interiors, and are already published
   semantically from the graph model (finding 9). "Custom-painted" and "opaque
   to the tree" are not the same predicate.

7. **Missing, and load-bearing: a *unified* quiescence signal.** No single
   signal says "loads, layout, and script have all settled." But every
   per-source signal already exists (verified against the tree 2026-07-09), so
   the missing piece is precisely the cross-subsystem join and its vocabulary,
   nothing more:

   - microtasks: `script-engine-api`'s `pump(Budget) -> PumpOutcome`
     (`Quiescent` / `Pending`; boa and piccolo document their drain semantics)
   - timers: the runtime's `next_timer_delay() -> Option<f64>`
   - animation-frame callbacks: the runtime's `has_animation_frame_callbacks()`
   - fetches: the runtime's `pending_fetches() -> usize`
   - declared animations: `IncrementalLayout::has_active_animations()`
   - loads: `LoadingState` is terminal at `Done` / `Failed`

   External drivers poll and sleep precisely because they cannot see these; the
   host can see all of them. This is the single highest-leverage primitive in
   the plan. App-side prior art: mere's scenario vocabulary has a
   `settle [<frames>]` verb, a frame-count approximation of what the host can
   now report exactly.

8. **Mere already has app-level automation; the core slots beneath it.**
   Mere's headed automation plan landed a scenario vocabulary over registry
   command ids (`invoke`, `navigate`, `key`, `capture`, `settle`, `assert`),
   a `MEERKAT_SCENARIO` self-drive mode with the executors shared between the
   headless `agent_harness` and the headed run, and an mk-harness reduced to
   launch + collect. OS synthetic input is already dead there. What that plan
   explicitly leaves outside a scenario is element-targeted interaction
   (pointer gestures, find-by-label-and-click): registry ids name app verbs,
   not elements. That is precisely the layer this core supplies. The two-layer
   doctrine from that plan (headless state assert vs headed pixel truth) is
   durable; the core serves both layers rather than replacing either, and the
   scenario format is a natural adapter over it (a future `find` / `click
   <query>` verb backed by the core).

9. **The a11y actuation route is proven in an app, in-process only.** Meerkat's
   D6 bridge runs `ActionRequest` -> route table -> semantic node selection ->
   `meerkat.agent.action_applied` diagnostic, with a miss emitting
   `meerkat.agent.intent_dropped`; orrery graph links (`Role::Link`, label plus
   URL value) and roster rows are published into the tree. The evidence is
   meerkat's harness tests, which drive `apply_a11y_request` directly. Mere's
   AccessKit verification checklist is **not** evidence: its own First Result
   section reads "Pending manual run," so the OS half (Narrator / VoiceOver /
   Orca traversal and activation) has never been exercised by anyone. Phase 1's
   action routing therefore generalizes a spine that is proven in-process and
   unproven at the OS boundary. Do not let phase 1 inherit that gap silently.

   **Mechanism, and it is the one phase 2 must not copy.** Those graph links
   are projected *from the graph model* (`frame_a11y_panes.rs` walks graph
   nodes), not published by a painted leaf, which is why they carry real titles
   and URLs. This is host model projection, a different mechanism from leaf
   semantic children. See phase 2.

10. **Agents reach the core through the tool-ring seam, not around it.**
    Mere's agent harness brief commits to one tool vocabulary: the model's
    tools are the registry's `enabled_actions`, every action gated, every
    mutation carrying a provenance edge, "never a parallel catalog." An agent
    calling serval-drive directly would be exactly that parallel channel and
    would bypass the gate spine. So for agents the core is the mechanism
    beneath a tool ring, not an API they hold raw: the brief's three rings
    (registry actions, graph reads, outbound MCP) gain a fourth, **page
    content**, the ring registry ids cannot name (find a link by text inside
    a loaded page, fill a form, read a table). Element-level page actions
    surface in the same catalog, gate through the same spine (phase 3's trust
    gate is the engine-side half), and stamp the same provenance. On the
    observe side the core enriches `AgentObservation` assembly with
    engine-level truth (semantic tree, geometry, loading, quiescence). Tests
    and harnesses keep calling the core directly; they are not inside the
    agent loop's consent model.

11. **WPT reaches the core directly, but the core is one link in a six-link
    chain, and its actuate side assumes a host surface `serval-wpt` does not
    have.** (Added 2026-07-09, verified against the tree and against the WPT
    harness-exactness plan's H6 triage.)

    - **The `test_driver` seam is an embedder hook, not a protocol.**
      `tests/wpt/tests/resources/testdriver.js` routes every command through
      `window.test_driver_internal`; the shipped default object's methods throw
      `"… is not implemented by testdriver-vendor.js"`, and WPT's own
      `testdriver-vendor.js` is a one-line blank file by design. So the phase-1
      core is reachable in-process; the phase-4 adapter is not on this path.
    - **`action_sequence` receives the WebDriver Actions tick JSON**, so
      implementing it *is* implementing the Actions tick model. This plan
      currently places the tick model in phase 4 ("the Actions tick model onto
      sequenced input synthesis") while the synthesis primitives sit in phase 1.
      For WPT the **tick interpreter must live in the core**, with phase 4's
      adapter and `serval-wpt`'s `test_driver_internal` as two consumers of it.
      Phase 1's actuate bullet now says so.
    - **The core supplies link 5 of 6.** The harness-exactness plan's H6 traced
      `dom/events/non-cancelable-when-passive` (40 tests, all `no-results`) and
      found the ordered prerequisites to be: window `EventTarget` -> `onX`
      event-handler attributes -> the `load` event -> a harness rAF pump ->
      `test_driver_internal` -> Touch/Pointer event types. serval has none of the
      first four (`var window = globalThis` with `addEventListener` only on
      `Node.prototype`; no `onX` mechanism, so `document.body.onload = fn` is an
      inert expando; no `load` event fired anywhere; `serval-wpt` never calls
      `Runtime::run_animation_frame_callbacks`). On link 6: `passive` listener
      options **are** implemented (`preventDefault` ignored), but `TouchEvent` and
      `WheelEvent` do not exist in `dom/bootstrap.js` at all, and those tests
      assert `event.cancelable` on `touchstart` / `touchmove` / `mousewheel`. The
      core supplies the injection; the DOM must still supply the event types.
    - **Hosted-surface assumption.** Phase 1 actuate synthesizes "into the same
      input path winit events take" and resolves handles "to in-view center via
      serval-layout geometry, scroll-into-view if needed." That presumes a window,
      an input path, a layout session, and hit-testing. The `serval-wpt`
      testharness lane has none: it builds a `Runtime` over a `StaticDocument` and
      **never constructs an `IncrementalLayout`**. There is no geometry to resolve
      a handle against and no input path to inject into. Driving the core from WPT
      therefore requires the harness's driven rendering loop (layout session,
      `load`, rAF pump, virtual clock) plus a headless surface for the core to
      actuate against. That same capability also unblocks the CSS-animations
      event tests and the CSS-transitions T3 WPT slice; see
      `2026-06-24_wpt_harness_exactness_plan.md` H6 and
      `2026-07-09_css_animations_plan.md` A3.
    - **`settled()` is not the WPT primitive.** WPT tests drive their own rAF
      loops and never call it, so the perpetual-source exclusion is irrelevant
      here. What WPT needs from the harness is a virtual clock plus an rAF pump,
      not quiescence.

## Architecture

Five layers, each a phase. Lower layers never depend on higher ones.

```text
  thirtyfour / WPT        BiDi clients        in-process agents (vates)
        |                      |                      |
  [P4] classic adapter   [P5] BiDi adapter      (no adapter: direct calls)
        \______________________|______________________/
                               |
                [P1] automation core (serval-drive)
          observe: semantic tree, state queries, quiescence
          actuate: element-targeted input, action routing
                               |
        engine-observables-api . a11y projection . query_traverse
        serval-layout geometry . input path      . script-engine-api
                               |
                [P2] chisel leaf registration
                [P3] trust gate (wraps every entry)
```

### Phase 1 — the automation core

#### Where the core lives: no new crate

The core is a set of capabilities, not an object, and each one already has a home
that needs **no new dependency**. Founding `serval-drive` up front was the wrong
instinct: it would freeze a public API before the plan's own hardest question
(handle-identity anchoring) is answered, and force a naming and licensing
decision that buys nothing yet.

- **`serval-layout` takes the observe and geometry half.** It already carries the
  semantic tree (`build_subtree`), stylo's selector machinery over the DOM
  (`adapter_stylo.rs` implements `selectors::Element` / `OpaqueElement`), the
  fragment plane for bounds and in-view centers, hit-testing, caret and
  selection. It is already the "queries over a laid-out DOM" crate. Element
  query, the handle registry, and staleness resolution land here as a `query`
  module, adding nothing to the manifest. Note this also means CSS queries do
  **not** need `query_traverse` (finding 2), which would drag `script-engine-api`
  in through `script-runtime-api`.
- **`shared/embedder` takes the actuate seam.** It already holds `input_events.rs`
  and `webdriver.rs` (`WebDriverCommandMsg`, `WebDriverScriptCommand`,
  `WebDriverJSResult`) and already depends on `accesskit`, `keyboard-types` and
  `euclid`. It depends on no window, and `serval-wpt` already depends on it. The
  surface trait and the Actions tick interpreter belong beside the seam types
  that survived the knockout.
- **`engine-observables-api` takes quiescence as a contract**, beside
  `LoadingState` / `LoadProgress`. `script-engine-api` already drains microtasks
  to quiescence and layout knows when it is idle; only the host sees all three,
  so `settled()` is a trait implemented per lane and joined by the host.
- **The WebDriver endpoint is a port**, not a core crate. It belongs beside
  `serval-wpt`.

`serval-layout` and `shared/embedder` are siblings; neither depends on the other.
That is correct, because the thing that composes geometry with input is the
consumer, which has both.

**What a crate would eventually buy** is only the composition façade: resolve a
handle, find its in-view center, scroll it into view, synthesize the tick, await
settled. A few hundred lines, eventually shared by pelt, strophe, `serval-wpt`
and the classic adapter. Extract it when the *second* consumer wants the same
composition; by then its shape is known, and lifting a façade off crates that are
already libraries is mechanical. Naming waits for that moment.

One thing could still force a crate: `shared/embedder` is Servo-derived and MPL,
so new automation logic added there inherits MPL. If this lane should be
MIT/Apache, the seam wants a different house. That is a licensing choice, not a
technical necessity, and it touches only the tick interpreter and the surface
trait, so it blocks nothing.

**Observe:**

- Semantic tree snapshot: reuse the AccessKit projection
  (`serval_layout::build_subtree`) as the one semantic tree. Automation reads
  the same tree screen readers do; divergence between what a user's AT sees and
  what a test asserts becomes impossible. Leaf interiors are absent from that
  tree until phase 2 wires `Leaf::accessibility()` (finding 3); phase 1 targets
  DOM and host-projected nodes only, and must not be specified against leaf
  children it cannot yet see.
- Element query: role / accessible-name / routability queries over the one
  `Projection`, landed as `serval-layout`'s `query` module. CSS selector queries
  ride serval-layout's existing stylo adapters, **not** `query_traverse`, which
  would pull `script-engine-api` in through `script-runtime-api`. `find_one`
  answers `None` on an ambiguous match rather than the first hit.
- Handle registry. **The identity rule inverted once checked against the tree
  (landed 2026-07-09).** The handle *is* the DOM node id, and that is sound:
  xilem-serval's `rebuild` reuses the existing node and diffs attributes and
  children in place, so an update preserving an element preserves its id; and
  `ScriptedDom` allocates from a monotonic counter and never recycles an index on
  removal, so a dead id is never reissued. Resolution answers `Live` or `Stale`,
  never a different element, and it gets that for free from the DOM.

  A `role + label` anchor would have been *worse than the raw id*: a second
  button labelled "Delete" silently satisfies an anchor minted against the first,
  which is precisely the "wrong node" this plan forbids. Names locate an element;
  they do not identify one. A handle captures its role and name for **diagnosis
  only** ("was: button \"Save draft\""); re-finding is the caller's decision to
  make out loud with a fresh query. A test asserts the non-recycling invariant, so
  adding a free list to the arena breaks the build rather than the guarantee.
- Surface axis from day one: every observe/actuate call takes a surface
  handle (window / webview). Mere is one-state-N-windows, the scenario format
  already targets windows with `@<n>`, and WebDriver classic requires window
  handles; retrofitting the axis later would break the whole API.
- State reads: text, attributes, geometry (from serval-layout), interaction
  state (`InteractionQuery`), loading state.
- **Quiescence, scoped by source — the contract landed 2026-07-09**
  (`engine-observables-api::quiescence`): a `PendingWork` snapshot per surface
  (loads, microtasks, dirty layout, next timer, rAF requested, declared
  animations) with `settled()` as the default policy. Level-triggered, not a
  future: the harness loop is "apply step, ask until settled, assert". The host
  assembles it from the per-source signals in finding 7, since only the host
  sees loads, script and layout at once.

  Perpetual sources are excluded from `settled()` by design: declared CSS
  animations and physics fields (the orrery never stops breathing) would make
  a naive settle hang forever, and the first hung test would get "fixed" with a
  timeout, which is a sleep wearing a suit. **Animation-frame callbacks join
  that category** (a correction found while writing the contract): a one-shot
  rAF settles next frame, but a rAF loop — every game loop — re-requests
  forever, and the two are statically indistinguishable, so rAF is reported but
  never blocking. Timers likewise: a page holding `setTimeout(fn, 30_000)` is
  not "busy" for thirty seconds. For the excluded category the tool is
  condition-waits ("element exists," "attribute equals"), still to build. Mere's
  `settle [<frames>]` frame counting is the symptom this design answers, not a
  primitive it merely upgrades.
- Screenshot: element- and viewport-scoped, via the existing paint/rasterize
  path (`SurfaceHost::rasterize`).

**Actuate:**

- Element-targeted input: resolve handle to in-view center via serval-layout
  geometry, scroll-into-view if needed, then synthesize pointer/key events
  into the same input path winit events take (host converts via the existing
  `key_event_from_winit`-adjacent types, so synthetic and real events are
  indistinguishable downstream).
  - **The surface is a seam, not winit.** Actuate must be defined against a
    trait the winit host implements and a headless surface (geometry + event
    sink, no window) also implements, or `serval-wpt` cannot use the core at all
    (finding 11). Do not bake winit into the core's actuate signature.
- **The WebDriver Actions tick model lives here, in the core**, not in the
  phase-4 adapter: ordered input sources, per-tick dispatch, pointer/key/wheel
  source state. WPT's `test_driver.action_sequence` hands over exactly this JSON,
  so the classic adapter and `serval-wpt`'s `test_driver_internal` are two
  consumers of one interpreter (finding 11). Phase 4 translates HTTP onto it; it
  does not own it.
- Action routing: reuse the a11y `ActionRequest` drain for semantic actions
  (focus, activate, set value), so automation actions and screen-reader
  actions share one code path.
- Script evaluation: `script-engine-api` eval on the nova lane, results as
  `WebDriverJSResult` (types already aligned).

**Done when:** a Rust test drives pelt end-to-end (launch, query by role,
click, wait settled, assert text, screenshot) with zero coordinate literals
and zero sleeps.

*Status 2026-07-09: the loop's core is proven headless.*
`semantic_query_drives_a_tab_switch` (pelt's `tile_surface` tests, GPU-free
feature) finds a tab by `role=Tab` + name on the semantic projection, activates
it through the same dispatch path a pointer takes, and reads the switch back as
`is_selected` state — zero coordinates, zero sleeps, and the handle resolves
`Live` across the rebuild. Still open toward the full done-when: a *windowed*
launch, a settled-wait over a document actually loading (this frame DOM has no
async work), text assertion on page content, and the screenshot. Getting there
also surfaced missing ARIA vocabulary: the walk now maps `role="tab"` /
`"tablist"` and `aria-selected` (AccessKit's first-class `Selected` flag), and
`Tab` joined the Click-routable roles — pelt's tab bar is the first annotated
consumer, and meerkat's roster (which encodes selection in a description
string today) should adopt `aria-selected` when it comes out of extraction.

### Phase 2 — leaf interiors join the tree

Leaves are replaced elements; their interior is invisible to the DOM. But two
distinct mechanisms put non-DOM content into the semantic tree, and the plan
needs both. They are not interchangeable, and choosing by who owns the state:

- **Host model projection** (already in use, not chisel's business). Where the
  *host* owns the state, it projects its domain model straight into the tree.
  Meerkat does this for orrery graph links and roster rows
  (`frame_a11y_panes.rs`), which is why its gnodes carry real titles and URLs a
  painter has no access to. Generalize this pattern; do not replace it with leaf
  publication where it already fits.
- **Leaf publication** (the new work). Where the *leaf* owns the state, a `Knob`
  its value, a `Meter` its level, a loop lane its clips, the leaf publishes.
  This is what `Leaf::accessibility()` was declared for and what nothing
  implements today.

**Step zero — landed 2026-07-09.** The declared contract is now true.
serval-layout gained a `LeafA11ySource` trait mirroring `LeafPaintSource` (the
walk knows `<chisel-leaf key="…">` as an element, not chisel's types, so the
host bridges the key to its leaf), and `build_subtree_with_leaves` threads it
through the walk via the existing `construct::chisel_leaf_key_of`. A leaf runs
*after* the DOM-derived semantics, so it may override its role and name itself,
and *before* bounds, so layout keeps sole authority over geometry. `Knob` now
announces as `Role::Slider` with its normalized value and declares
`SetValue`/`Increment`/`Decrement`; `Meter` announces as `Role::Meter` and
declares nothing, because a meter reports rather than actuates.

Actionability generalized with it: the walk hands back any node advertising a
routable action, whether it acquired one from its role (a `<button>` takes
`Click`) or from a leaf declaring its own. So a leaf interior is actuated
through the same routing path a DOM control is, which is the property phase 1
and phase 4 both need. `build_subtree` keeps its old signature and delegates
with a `NoLeafA11y` source, so leaves stay opaque for callers that have none.

Strophe is the first consumer: its output meters are `<chisel-leaf>`s and now
announce their level instead of projecting as unlabeled boxes. Meerkat's chrome
also carries leaves (the toolbar cluster) and is the obvious next adopter; it
compiles unchanged because the old entry point kept its shape.

Then two additions to the leaf contract (extending, not replacing,
`Leaf::accessibility()`):

- **Semantic children:** a leaf may publish named sub-nodes into the semantic
  tree (a loop lane publishes its clips; an isometry map its tiles) with roles,
  labels, and geometry in leaf-local coordinates that the projection offsets
  into page space.
- **Action targets:** those sub-nodes accept the same `ActionRequest` verbs;
  the leaf's existing `event()` path executes them.

Per-leaf cost is one method pair; the convention lands in the chisel leaf
design doc as the normative home. Because this rides the a11y projection,
every leaf that becomes automatable becomes screen-reader-visible in the same
change.

**Done when:** a test finds a *leaf-owned* sub-node by label, acts on it, and
asserts state, through the same API as phase 1: a strophe loop-lane clip, or a
`Knob` reporting its value as a slider. Mere's gnodes are explicitly **not** the
acceptance case. They reach the tree by host model projection (finding 9), so a
gnode test would pass green without exercising one line of leaf publication.

### Phase 3 — trust gate

An engine-native surface that reads all state and synthesizes all input is a
remote-control vulnerability if ambient. The gate wraps every entry point
before any remote adapter ships:

- Off by default; enabled per session by explicit host opt-in (CLI flag or
  host API call), never by an env var alone.
- Local-only transport by default (loopback bind for adapters; in-process
  calls are host-mediated).
- Session-scoped capability token; adapters authenticate with it.
- Anything beyond loopback is out of scope here and routes through personae
  when that lane matures. This plan deliberately does not design it.

**Done when:** with the gate closed, adapter ports refuse to bind and core
calls from non-host code return a capability error; a test proves both.

### Phase 4 — WebDriver classic adapter

An HTTP endpoint implementing the pinned `webdriver` crate's handler trait,
translating spec commands onto the core. Sessions and capability negotiation;
element refs from the handle registry; Find Element onto element query;
Element Click / Send Keys onto actuate (the spec's interactability checks:
in-view center, obscured test, scroll-into-view, implemented once in the core,
phase 1); the Actions endpoint onto the core's tick interpreter (which is phase-1
work, not this phase's — finding 11); Execute Script onto eval plus the spec's
JSON clone serialization; navigation and screenshots onto existing engine paths;
cookies onto the net component; prompts onto the surviving prompt types. Commands
whose substrate does not exist yet return spec-correct `unsupported operation`
rather than fakes.

**Scope note:** this phase is **not** what lets `serval-wpt` run the ordinary
`test_driver` corpus; that rides phase 1 in-process (finding 11). What needs an
adapter is WPT's own `webdriver/` conformance suite, which tests this phase.

**Done when:** thirtyfour, unpatched, runs a session against pelt: navigate,
find by CSS, click, read text. WPT's `webdriver/` conformance suite (vendored at
`tests/wpt/tests/webdriver/`) runs under `ports/serval-wpt` with a recorded
pass/fail baseline (a baseline, not a target; gaps become follow-on items with
the spec section named).

### Phase 5 — BiDi adapter

The event-driven lane, and the one agents want most after the core itself:
subscription streams (load, DOM mutation via the capture/replay lane, console,
network), realm-scoped script, network interception. Same core, session
adapter instead of request/response. Scoped last because classic covers app
testing and the core covers in-process agents; BiDi pays when external agents
need event streams. Sequenced after phase 4, not skipped: the ecosystem
framing (agents driving a web-compatible GUI natively) lands fully only with
this phase.

**Done when:** an external client subscribes to load + mutation events on a
live session and drives an interaction loop with no polling.

## What this buys, per consumer

- **Mere:** the scenario vocabulary gains the element layer it deliberately
  left out (pointer gestures, find-by-query) as verbs backed by the core, and
  `settle [<frames>]` is retired rather than upgraded: frame counting is
  replaced by the engine's scoped `settled()` where the sources are finite, and
  by condition-waits where they are perpetual (the orrery never stops
  breathing). The scenario runner and the two-layer doctrine stay as they are.
- **Apps without a scenario runner (strophe, isometry, pelt):** element-level
  driving arrives without each app building meerkat's harness stack first;
  screenshots remain for appearance only.
- **Agents (vates lane):** the agent loop gains the page-content tool ring
  (finding 10): element-level page actions in the same gated, provenanced
  vocabulary as registry actions, plus engine-truth observations (semantic
  tree, quiescence) in context assembly. No screenshot-and-guess loop, and no
  parallel ungated channel either.
- **A11y:** phases 1 and 2 force the AccessKit projection to be complete and
  truthful, because tests now depend on it.
- **WPT: two distinct uses, and only one of them is the classic adapter**
  (corrected 2026-07-09; see finding 11).
  - *Running ordinary WPT tests that need synthetic input* (the `test_driver`
    corpus) needs **phase 1 only**, on the same "no adapter: direct calls" lane
    as in-process agents. WPT's `testdriver.js` resolves every command through
    `window.test_driver_internal`, whose default methods merely throw
    `"… is not implemented by testdriver-vendor.js"`. The embedder supplies that
    object. `serval-wpt` binds it straight to host functions, exactly as the
    runtime already exposes `__matchMedia` / `__dispatchSynthetic`. No HTTP, no
    session negotiation, no `webdriver` crate. **This payoff lands three phases
    earlier than this plan previously implied.**
  - *Running WPT's own `webdriver/` conformance suite* (present in the checkout at
    `tests/wpt/tests/webdriver/`) exercises **our WebDriver implementation** and
    genuinely needs phase 4. That proves the adapter rather than using it.
  - Necessary, not sufficient: the core supplies one link in a six-link chain.
    See finding 11.

## Non-goals

- Designing remote/P2P automation transport (noted as personae follow-on).
- CDP compatibility. BiDi is the standards lane; CDP emulation is a different,
  larger bet, revisit only if a concrete consumer appears.
- Driving third-party engines (scry/graft/weld multiplexer lanes). The core is
  serval-native; a multiplexer story would be a separate plan.
- The agent loop itself (context assembly, inference, gating policy, run
  provenance). Owned by mere's agent harness brief; this plan only supplies
  the page-content ring and the observation feed (finding 10).

## Open questions

- ~~**Handle identity anchoring.**~~ **Settled 2026-07-09, and it was not hard;
  it was mis-stated.** Both premises were false. xilem-serval's `rebuild` reuses
  the node rather than recreating it, and `ScriptedDom` never recycles an arena
  index. So the raw DOM id already survives rebuilds and can never alias a live
  element, and the proposed `role + label` anchor would have introduced the exact
  rebinding hazard the requirement forbade. The handle is the node id; the
  captured role and name are diagnostics. See phase 1 and the non-recycling test.

  What remains open is narrower: a **host-assigned automation id attribute** is
  still worth having, not for identity but for *addressability* — so a caller can
  name a control that has no distinctive accessible name, instead of relying on
  ambiguity-rejecting queries. That is a convenience layer above handles, not a
  replacement for them.
- **Quiescence scope boundaries.** Where the settled/perpetual line sits for
  sources that are neither (long transitions, media playback, streaming
  loads); whether a scope descriptor is per-call or per-session. Empirical;
  start with the loads+layout+script default and grow from hung-test
  evidence.
- **Scenario-verb integration home.** Whether the `find` / `click <query>`
  scenario verbs live mere-side (scenario runner calls the core) or
  serval-side (a shared verb layer other apps' runners reuse). Decide when
  the first element verb is actually needed by a scenario; the core's API is
  the same either way.
- **Semantic-children granularity for dense leaves.** A dense leaf should not
  publish thousands of tree nodes per frame. Viewport-culled publication,
  on-demand query, or both. Mere's orrery is *not* the stress case: its gnodes
  are DOM divs projected from the graph model, and the chrome walk already skips
  the `.orrery` subtree so the frame tree can project it richly. Pick a
  genuinely dense leaf (isometry's map) instead.

- **Shape of the surface seam.** Actuate needs geometry and an event sink. The
  winit host has both; `serval-wpt`'s testharness lane has neither today (no
  layout session at all). Does the core take a `Surface` trait (geometry +
  event sink + rasterize), or does it require callers to hand it an
  `IncrementalLayout` plus an input sink? Settle before phase 1's actuate API
  freezes, because `serval-wpt` is the second consumer and it is headless
  (finding 11).

- ~~**Does the core need its own crate?**~~ **Settled 2026-07-09: no.** Every
  capability has an existing home needing no new dependency; see "Where the core
  lives". The open remainder is narrower: *when* the composition façade earns its
  own crate, and under which license, given `shared/embedder` is MPL. Revisit at
  the second consumer, not before.

## Progress

- 2026-07-09: **quiescence contract landed** in
  `engine-observables-api::quiescence`: `PendingWork` (per-source snapshot) +
  `settled()` / `fully_idle()` + the `QuiescenceQuery` trait. Verified first
  that every per-source signal exists (finding 7 now lists them by name), so
  the contract adds vocabulary only, no mechanism. One design correction fell
  out: rAF callbacks are perpetual-capable (a loop is indistinguishable from a
  one-shot), so they are reported but never block `settled()`, alongside timers
  and declared animations. Tests pin the policy from both directions: a surface
  that is only animating is settled (or every game loop hangs the harness), and
  each self-finishing source blocks it. Remaining quiescence work is host
  assembly per surface and the condition-wait built on top.

- 2026-07-09: **phase 1 begun; handle identity settled by inverting it.** The
  `query` module landed in `serval-layout` (`ElementQuery` by role / accessible
  name / routability; `find_one` refuses an ambiguous match). The walk now retains
  each node's DOM origin and returns a `Projection`, the single semantic
  projection that `accesskit_tree`, `build_subtree` and queries are all views
  onto; `actionable` is derived from the finished nodes, so a leaf declaring
  `SetValue` counts exactly like a `<button>` that got `Click` from its role.

  The handle question turned out mis-stated rather than hard. Checked against the
  tree: `rebuild` reuses the node (xilem-serval `element.rs`), and `push` /
  `drop_subtree` never recycle an arena index (`serval-scripted-dom`). So a raw
  DOM id survives rebuilds and cannot alias a live element, while the proposed
  `role + label` anchor would have rebound to a same-named sibling. Handles are
  node ids; role and name are captured for diagnosis. A test asserts the
  non-recycling invariant directly, so a future free list fails the build.

  Chisel leaf interiors are queryable through the same path, which is phase 2's
  step zero paying off inside phase 1: a `Knob` is found by `role = Slider`, and
  the same DOM yields nothing without a leaf source.

- 2026-07-09: **placement settled: no new crate.** Verified against the manifests
  rather than the layer diagram. `serval-layout` depends on
  `engine-observables-api`, so the core cannot live there (cycle). No existing
  crate occupies the slot above `serval-layout` and below both the windowed hosts
  and the headless `serval-wpt` (which deps `script-engine-api`/`-boa`/`-nova`
  and neither `winit` nor `xilem-serval`) — but none is needed, because the core
  decomposes into capabilities that already have homes: observe/geometry/handles
  into `serval-layout` (whose stylo selector adapters make `query_traverse`
  unnecessary), the surface trait and Actions tick interpreter into
  `shared/embedder` beside `webdriver.rs` and `input_events.rs`, and `settled()`
  as a contract into `engine-observables-api`. Only the composition façade would
  ever want a crate; defer it to the second consumer. Phase 1 is therefore
  unblocked: it needs no naming pass and no licensing decision to begin.

- 2026-07-09: plan drafted from seam survey (observables API, a11y bridge,
  query_traverse, webdriver seam types, script engines, chisel leaf contract).
  No code yet.
- 2026-07-09: reconciled against mere's headed automation plan and AccessKit
  verification checklist (findings 7-9; per-consumer section corrected: mere's
  scenario self-drive already killed OS synthetic input, the core supplies the
  element layer that plan left open).
- 2026-07-09: reconciled against mere's agent harness brief (finding 10): for
  agents the core is the mechanism beneath a gated page-content tool ring in
  the one registry vocabulary, never a parallel ungated channel; the vates
  bullet and non-goals corrected accordingly.
- 2026-07-09: self-review pass. Quiescence rescoped (perpetual sources
  excluded by design; physics/animations would hang a naive `settled()`),
  handle identity named as the hardest open question (xilem rebuilds vs
  stable handles), surface axis added to phase 1 (one-state-N-windows),
  Open questions section added.
- 2026-07-09: verification pass against the tree (every "already exists"
  finding checked against source, not against other docs).
  - **Finding 3 corrected, and it was load-bearing.** `Leaf::accessibility()`
    occurs exactly once in all of chisel, as an empty default body; no leaf
    implements it and `build_subtree` never calls it. Leaves do not join the
    tree today. Phase 1's semantic-tree bullet no longer assumes them; phase 2
    gains a "step zero" that wires the hook.
  - **Phase 2 restructured.** It conflated two mechanisms. Host model
    projection (meerkat's gnodes and roster rows, from `frame_a11y_panes.rs`)
    is distinct from leaf publication. Its acceptance test, "find a gnode on
    mere's graph canvas," would have passed without exercising a single line of
    leaf publication, because gnodes are DOM divs, not leaf interiors. The
    done-when now targets a leaf-owned sub-node.
  - **Finding 9 re-cited.** The AccessKit verification checklist proves nothing:
    its First Result reads "Pending manual run." The real evidence is meerkat's
    harness tests exercising `apply_a11y_request`. The route is proven
    in-process and unproven at the OS boundary; that gap is now stated.
  - **Finding 7 credits existing work.** `script-engine-api` already drains
    microtasks to quiescence; the missing piece is the cross-subsystem join,
    not the script term.
  - **Finding 6 narrowed** ("custom-painted" is not "opaque to the tree"), and
    the dense-leaf open question moved off the orrery, which is not a leaf.
  - Founding questions added (new crate versus growing `engine-observables-api`;
    license for a non-Servo-derived component).
  - Findings 1, 2, 4, 5, 8, 10 verified accurate as written. `query_traverse.rs`
    is at the cited path; `WebDriverJSResult` is at `webdriver.rs:273` over
    serval's own `JSValue`; the four script-engine crates exist; `webdriver`
    v0.53 is still a pinned workspace dep.
- 2026-07-09: **phase 2 step zero implemented.** `LeafA11ySource` +
  `NoLeafA11y` + `build_subtree_with_leaves` in serval-layout, wired into the
  walk through `construct::chisel_leaf_key_of`; `Knob` announces as a slider
  (value + `SetValue`/`Increment`/`Decrement`), `Meter` as a read-only meter.
  The actionable handback now records any node advertising a routable action,
  so leaf interiors route exactly like DOM controls. `build_subtree` unchanged,
  delegating with `NoLeafA11y`. Test `chisel_leaf_interior_reaches_the_tree_
  through_its_source` asserts both directions (opaque without a source; role,
  name, value, routability, and layout-owned bounds with one). Strophe wired as
  the first consumer (its output meters). Finding 3's caveat is now historical
  rather than current; it stays recorded because the plan was written against
  the false version.
- 2026-07-09: **reconciled against the WPT harness-exactness plan's H6 triage**
  (finding 11 added; the per-consumer WPT bullet, phase 1 actuate, and phase 4
  scope corrected). Three changes, in decreasing order of consequence:
  - The WPT payoff was mis-phased. `test_driver` is an embedder hook
    (`window.test_driver_internal`, whose shipped defaults throw and whose
    `testdriver-vendor.js` is deliberately blank), so `serval-wpt` reaches the
    core **in-process at phase 1**. Phase 4 is needed only to run WPT's own
    `webdriver/` conformance suite. The plan previously pointed all of WPT at
    phase 4.
  - The Actions tick model was mis-layered. `test_driver.action_sequence` receives
    WebDriver Actions tick JSON, so the interpreter belongs in the phase-1 core
    with phase 4 and `test_driver_internal` as two consumers, not in the adapter.
  - Phase 1 actuate silently assumed a hosted (winit) surface. `serval-wpt` has no
    window, no input path, and no `IncrementalLayout` at all, so the actuate API
    must be defined against a surface seam. Added as an open question, since it
    constrains the API before it freezes.

  Also recorded: the core supplies one of six prerequisites for the 85
  `test_driver`-blocked `dom` tests. `passive` listener options already exist;
  `TouchEvent` / `WheelEvent` do not exist at all, and those tests assert
  `event.cancelable` on them. The core supplies injection, not event types.
