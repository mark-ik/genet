# Native automation: engine-level observe/actuate core with WebDriver adapters

**Date:** 2026-07-09
**Status:** plan, proposed. Findings verified against source 2026-07-09 (see
Progress); no code yet.

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
   the native core's read side; the plan grows it rather than founding a new
   crate.

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
   signal says "loads, layout, and script have all settled." Partial prior art
   exists engine-side, and the plan should build on it rather than start cold:
   `script-engine-api` already drains pending microtasks *to quiescence*
   (`pump` with `Budget::Unbounded`), and the boa / piccolo engines document
   their drain semantics against it. What is missing is the cross-subsystem
   join, not the script term. External drivers poll and sleep precisely because
   they cannot know this; the engine can. This is the single highest-leverage
   primitive in the plan. App-side prior art: mere's scenario vocabulary has a
   `settle [<frames>]` verb, a frame-count approximation of what the engine can
   report exactly.

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

A new component (working name `serval-drive`; naming pass with Mark before
founding) exposing a direct Rust API. Everything else is a view onto this.

**Observe:**

- Semantic tree snapshot: reuse the AccessKit projection
  (`serval_layout::build_subtree`) as the one semantic tree. Automation reads
  the same tree screen readers do; divergence between what a user's AT sees and
  what a test asserts becomes impossible. Leaf interiors are absent from that
  tree until phase 2 wires `Leaf::accessibility()` (finding 3); phase 1 targets
  DOM and host-projected nodes only, and must not be specified against leaf
  children it cannot yet see.
- Element query: selector and role/label/text queries via `query_traverse` and
  `SemanticQuery`. Returns element handles.
- Handle registry, with an identity rule: xilem-serval rebuilds and diffs the
  view tree every update cycle, so a handle pinned to a raw NodeId goes stale
  on any rebuild — Selenium's `StaleElementReference` wart, re-imported into
  an engine we own. Handles anchor to semantic identity (keyed-view path,
  role + label) so they survive rebuilds that preserve meaning; a handle
  whose anchor is gone answers "stale," never a wrong node. The exact anchor
  scheme is the plan's hardest open design question (see Open questions).
- Surface axis from day one: every observe/actuate call takes a surface
  handle (window / webview). Mere is one-state-N-windows, the scenario format
  already targets windows with `@<n>`, and WebDriver classic requires window
  handles; retrofitting the axis later would break the whole API.
- State reads: text, attributes, geometry (from serval-layout), interaction
  state (`InteractionQuery`), loading state.
- **Quiescence, scoped by source:** a `settled()` future the engine resolves
  when loads, layout, and pending script microtasks are idle. Perpetual
  sources are excluded from the default contract by design: declared CSS
  animations and physics fields (the orrery never stops breathing) would make
  a naive `settled()` hang forever, and the first hung test would get "fixed"
  with a timeout, which is a sleep wearing a suit. For the excluded category
  the tool is condition-waits ("element exists," "attribute equals"), also
  built here. Mere's `settle [<frames>]` frame counting is the symptom this
  design answers, not a primitive it merely upgrades.
- Screenshot: element- and viewport-scoped, via the existing paint/rasterize
  path (`SurfaceHost::rasterize`).

**Actuate:**

- Element-targeted input: resolve handle to in-view center via serval-layout
  geometry, scroll-into-view if needed, then synthesize pointer/key events
  into the same input path winit events take (host converts via the existing
  `key_event_from_winit`-adjacent types, so synthetic and real events are
  indistinguishable downstream).
- Action routing: reuse the a11y `ActionRequest` drain for semantic actions
  (focus, activate, set value), so automation actions and screen-reader
  actions share one code path.
- Script evaluation: `script-engine-api` eval on the nova lane, results as
  `WebDriverJSResult` (types already aligned).

**Done when:** a Rust test drives pelt end-to-end (launch, query by role,
click, wait settled, assert text, screenshot) with zero coordinate literals
and zero sleeps.

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

**Step zero: make the declared contract true.** `build_subtree` must call
`Leaf::accessibility()` when it walks a `<chisel-leaf>` (the leaf key is already
an attribute on the element, and the walk already computes the node's absolute
bounds), and `Knob` / `Meter` must implement it. That is the smallest change
that turns finding 3's unwired promise into working code, and it delivers the
doc's own example: a knob announcing as a slider with its value.

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
phase 1); the Actions tick model onto sequenced input synthesis; Execute
Script onto eval plus the spec's JSON clone serialization; navigation and
screenshots onto existing engine paths; cookies onto the net component;
prompts onto the surviving prompt types. Commands whose substrate does not
exist yet return spec-correct `unsupported operation` rather than fakes.

**Done when:** thirtyfour, unpatched, runs a session against pelt: navigate,
find by CSS, click, read text. WPT's webdriver conformance suite runs under
`ports/serval-wpt` with a recorded pass/fail baseline (a baseline, not a
target; gaps become follow-on items with the spec section named).

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
- **WPT:** the classic adapter is the substrate `serval-wpt` needs for
  harness-driven conformance runs.

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

- **Handle identity anchoring.** What exactly a handle pins to across view
  rebuilds: keyed-view path, role + label, a host-assigned automation id
  attribute, or a layered fallback of these. The hardest design question in
  the plan; settle it in phase 1 before the registry API freezes, with a
  test that rebuilds the view and proves the handle survives or reports
  stale (never resolves to a different node).
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

- **Does the core need its own crate?** Finding 1 says the read side grows
  `engine-observables-api` rather than founding a crate; phase 1 then founds
  `serval-drive` anyway. State what the core holds that observables plus an
  actuation module could not, before founding it. Workspace convention is to
  check existing crates and extend rather than duplicate.

- **License for a new component.** `serval-drive` would carry no Servo lineage,
  while every file it would sit beside inherits MPL. The founding convention for
  new, non-Servo-derived crates is MIT/Apache plus edition 2024. Decide before
  founding, not after; relicensing a crate with consumers is the expensive
  order.

## Progress

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
