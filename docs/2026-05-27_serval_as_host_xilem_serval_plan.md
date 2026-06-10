# serval as host: the `xilem_serval` reactive backend

Status: **strong through Stages 0-7; the previously named host-backend blockers
are now landed.** Scopes using
serval as the application host (chrome and content rendered by one engine),
and the reactive authoring layer that requires. The finding held: that
layer is mostly *reuse* of `xilem_core` (a third backend beside Masonry and
`xilem_web`), not a from-scratch Dioxus-style framework. The full loop —
`xilem_core` diff → serval DOM → layout → paint → netrender → present, with
input routed back through serval's hit-test + faithful xilem message
dispatch — is validated on screen (`pelt-live-counter`), serval the sole
engine. Stages 0 through 7 have since added component composition,
keyboard/focus, capture-phase events, overlays + inline style, erased views,
scrolling, z-index Tier 1, `DOM → AccessKit` emission, a real caret-aware text
field with selection + clipboard, select, radio, textarea, and complete IME.
Pointer-drag (`pointerdown`/`move`/`up` + capture), slider, Tab/Shift+Tab focus
traversal, and clip-aware scroll hit-testing are also present in the current
tree and covered by focused tests.
Sibling to the
[scripted render loop](#relationship-to-existing-docs): both share that
native dispatch substrate, which wires serval's *existing* hit-test query
into event routing rather than building a new one. Per-stage status and
commits are in [Staging](#staging).

## The three serval-GUI architectures

"Use serval for the GUI" is three different architectures, and most
confusion comes from blurring them.

1. **serval-as-texture-in-a-host.** serval renders a document to a wgpu
   texture; a host framework (Xilem today) composites it. This is what
   [pelt-viewer](../ports/pelt-viewer/render.rs) does for content now.
   The host owns the chrome; serval owns the content. This is the right
   division of labor while Xilem works, and nothing here proposes
   changing it.
2. **host framework + serval-rendered chrome (excluded).** Render the
   chrome *through* serval while keeping the host framework's widgets.
   This runs two layout engines, two event models, two focus/hit-test
   systems, two accessibility trees. The host collapses to a compositor
   and you pay for two of everything to gain only web-authoring
   ergonomics for the chrome. This is the worst seat and is deliberately
   excluded. Whenever the GUI stack wobbles, the temptation is to reach
   here; the answer is "go to 3 instead."
3. **serval-as-the-host.** serval owns the window, layout, paint, input,
   script, and accessibility. Chrome and content are the same kind of
   object: documents. This is the Blitz/Dioxus model, and serval already
   shares its shape (Stylo cascade + taffy + vello/wgpu via netrender).
   This is the only architecture where "chrome as CSS" is coherent,
   because there is a single engine.

The rest of this doc is about making (3) cheap, and the key is that its
biggest apparent cost (a reactive layer) is reuse.

## Why the reactive layer is reuse, not invention

A reactive layer's only job is **app state → a view tree → diff →
mutation calls** (`createElement`, `setAttribute`, `insertBefore`,
`removeChild`, attach listener). It does no layout, paint, or
hit-testing. So taking *only* that piece and pointing it at serval's
[`LayoutDomMut`](../components/shared/layout-dom/lib.rs) keeps serval the
sole engine. This is not architecture (2): there is no second layout
engine, only a state-to-mutations differ. Dropping the host framework's
*renderer* while keeping its *reactive core* is exactly what sidesteps
the double-engine trap.

`xilem_core` is built for this. It is `#![no_std]` and references no
`web_sys`; it already drives two backends in the fork at
`crates/xilem`: `xilem` (native, → Masonry) and `xilem_web` (→ the
browser DOM via `web_sys`). `xilem_web` is the proof that a DOM-shaped
backend is a supported target. `xilem_serval` is a third backend pointed
at serval's DOM, with `xilem_web` as the line-for-line template.

The mental model: **`xilem_serval` is `xilem_web`, but native, with
serval as the engine you own.** Xilem's Rust reactive authoring, serval's
web-fidelity rendering, one stack end to end (Xilem authoring → serval
engine → netrender/vello → wgpu).

Alternatives considered: fine-grained signals (Leptos `tachys`, or
Floem's reactive runtime) would update targeted nodes with no diff pass,
which is a slightly better impedance match to serval's mutation-recording
`IncrementalLayout` (fewer mutations per update). They remain a viable
route, but neither already ships a DOM backend to copy, and `xilem_core`
keeps us on the committed Linebender stack. gpui is a poor fit (bespoke
renderer, reactivity fused to its own layout) and is excluded for Mere
regardless.

## The `xilem_core` backend contract

A backend supplies (signatures from `crates/xilem/xilem_core`):

- A `Context: ViewPathTracker` holding the `id_path` (message routing),
  an `Environment`, plus backend state.
- An element type `E: ViewElement` with a `Mut<'a>` handle.
- A node + props pair with deferred `apply_props` (the `DomNode` shape in
  `xilem_web`).
- `ElementSplice<E>` (`insert` / `mutate` / `skip` / `delete` /
  `with_scratch`), which translates a view-sequence diff into ordered
  child mutations.
- Event listeners that capture a `MessageThunk(id_path)` and, on fire,
  `push_message(event)` into the `AppRunner`, which routes down the path
  and triggers a `rebuild`.

The message/rebuild cycle: a fired event becomes an `AppMessage { id_path,
body }`; the runner routes it down the view tree by `id_path`, the target
`View::message` mutates app state, then `app_logic(&mut state)` produces a
new view tree and `seq_rebuild` diffs it, emitting `ElementSplice`
operations the backend turns into DOM mutations.

## Mapping the contract onto serval

| `xilem_web` (browser) | `xilem_serval` (serval) | status |
| --- | --- | --- |
| `create_element_ns(ns, name)` | `LayoutDomMut::create_element(QualName)` | ready |
| `Text::new_with_data` | `LayoutDomMut::create_text(&str)` | ready |
| `set_attribute` / `remove_attribute` | `set_attribute(node, QualName, &str)` / **none** | gap (small) |
| `parent.insert_before(node, next)` | **none** (only `append_child`) | **gap (crux)** |
| `parent.remove_child(node)` | `LayoutDomMut::remove(node)` | ready |
| browser hit-test + `addEventListener` | `ServalLaneView::hit_test` exists; **native dispatch + listener registry** not wired | gap (wiring, shared) |
| browser relayout + repaint | `drain_mutations` → `IncrementalLayout` → `emit_paint_list` → netrender → present | ready |

The mutation + render spine already exists; it is the same pipeline the
scripted render loop uses. Two small mutation-API additions and one
larger shared capability (input) remain.

## Gap 1: ordered insertion + attribute removal (close now)

`ElementSplice::insert` inserts *before the cursor's next sibling*, not at
the end (`xilem_web` calls `parent.insert_before(node, next)`). With only
`append_child`, mid-list insertion forces O(n) churn and records the wrong
mutations. So [`LayoutDomMut`](../components/shared/layout-dom/lib.rs) and
[`ScriptedDom`](../components/serval-scripted-dom/lib.rs) need:

```rust
/// Insert `child` immediately before `reference` under `parent`
/// (append if `reference` is None), detaching it from any prior parent.
fn insert_before(&mut self, parent: Self::NodeId, child: Self::NodeId,
                 reference: Option<Self::NodeId>);

/// Remove an attribute (the xilem `AttributeModifier::Remove` case).
fn remove_attribute(&mut self, node: Self::NodeId, name: QualName);
```

`insert_before` is a small arena edit in `ScriptedDom` (insert into the
existing child `Vec` at the found index; record
`DomMutation::Inserted{node, parent}`, no position needed because
`IncrementalLayout` re-reads sibling order from the DOM). Both are
producer-side (the scripted-DOM crate) and both are real DOM methods the
JS DOM surface will want anyway (`insertBefore`, `removeAttribute`), so
this is shared groundwork, not a detour.

## Gap 2: native event dispatch (wiring, shared capability)

`xilem_web` gets events for free: the browser hit-tests and dispatches.
`xilem_serval` has no browser, but serval is further along than "no
hit-test." [`ServalLaneView`](../components/serval-layout/serval_lane.rs)
already implements `FragmentQuery::hit_test` (part of
`engine_observables_api`): it walks fragments in paint order and returns
the topmost `FragmentHit { source_node: SourceNodeId, local_point, .. }`
for a point, and it is tested. The *query* exists; the *interaction*
surface on the same view (`InteractionQuery::activation_target`,
`focus_target`) is still stubbed (probe v1, 2026-05-18). So the missing
piece is **wiring, not a new spatial index**:

1. **Identity round-trip.** `hit_test` returns a `SourceNodeId` (serval's
   `opaque_id`), and the reverse `SourceNodeId → NodeId` lookup in
   `ServalLaneView` is currently an O(n) DOM walk. Native dispatch wants
   that reverse index cached — the file already names the seam.
2. **A native dispatch walk** (capture → target → bubble over
   `parentNode`), invoking handlers from a `NodeId × event →
   MessageThunk` registry that `xilem_serval` populates in its `Context`.
   This must **converge with**, not fork from, the capture/target/bubble
   algorithm the scripting tier already runs in JS-bootstrap form in
   [script-runtime-api/dom.rs](../components/script-runtime-api/dom.rs)
   (W0c). One event model, two entry points (native handlers and JS
   listeners), one propagation algorithm.
3. **The window → lane wiring.** A pelt host pointer event feeds
   `ServalLaneView::hit_test`; the hit maps back to a node; the dispatch
   walk fires registered thunks; the runner rebuilds.
   `InteractionQuery::activation_target` is the natural home for "the
   listener-bearing target at this point."

This is the same substrate the **live scripted render loop** needs: a real
page's links, focus, and script `onclick` handlers route through the same
hit-test + dispatch. Building either pulls the other most of the way, so
the two should be sequenced together. The honest revision to earlier
framing: serval already has the hit-test query; Stage 2 *promotes and
wires* it, rather than building it.

## Boundaries and ownership

Two boundaries have to be explicit or this gets muddy fast.

**Same engine does not mean same DOM tree.** serval-as-host means one
engine with *separate document/surface authority*, not chrome and content
casually sharing a single DOM. A chrome surface authored by `xilem_serval`
(Rust app state diffed into nodes) and a content document mutated by page
JS are different capability domains. They need explicit separate roots (or
distinct documents), so app-state diffing, JS mutation, the CSS cascade,
event propagation, and the eventual security boundary stay separable.
`xilem_serval` owns and diffs *its* root; it does not reach into a content
document's tree, and content JS does not see the chrome's.

**A serval-native runner owns scheduling.** `xilem_core` provides view
diffing and message thunks, but it schedules nothing. Something
serval-native has to own state, the root node, the message queue, timers
and microtask checkpoints, render invalidation, and the rebuild cadence. A
small `ServalAppRunner` is therefore a Stage 1 *artifact*, not an
afterthought: it is the `AppRunner` `xilem_core` routes messages to, and
the thing that turns "a message arrived" into "drain mutations →
`IncrementalLayout` → netrender → present." It also decides when a rebuild
runs relative to the microtask/timer checkpoints the scripting tier
already exposes (`run_microtasks`, `run_event_loop`).

## Staging

- **Stage 0 — done (`cc4b30a`).** `insert_before` + `remove_attribute` on
  `LayoutDomMut`/`ScriptedDom`. Both record correct mutations; they are
  also the real DOM methods (`insertBefore`/`removeAttribute`) the JS
  surface wants.
- **Stage 1a — done (`84d7381`).** A minimal `xilem-serval` over
  `ScriptedDom`, exercised by tests, not a window: build an initial tree,
  then a middle `insert`, a `delete`, and an attribute set/remove through
  the `ElementSplice`, asserting the resulting `ScriptedDom` and drained
  mutations. The uniform element type (every node is a `NodeId`) drops
  `xilem_web`'s `AnyNode`/`Box`/downcast and makes `SuperElement` the
  identity; mutations apply eagerly (the `drain_mutations` boundary is the
  batch).
- **Stage 1b — done, decomposed into core + window.**
  - **1b-core (`2e4c2e8`):** `ServalAppRunner` (the real artifact —
    state + view tree + rebuild-on-update) plus the `el`/`text`/`attr`
    vocabulary, and a headless render driver in the new `pelt-live`
    (`scene_from_scripted_dom`: cascade → layout → paint → `netrender::Scene`),
    proven by a counter test offline. `IncrementalLayout` is the eventual
    relayout engine; the probe uses the stateless cascade+layout path.
  - **1b-window (`ef4c026`):** the `pelt-live-counter` bin — a real winit
    window presenting the counter via `netrender::boot` + `render_vello` +
    `compose_external_texture` (the format-bridging blit), a 1 Hz timer
    tick, and click input. Validated on screen 2026-05-28.
- **Stage 2 — done, in two slices.**
  - **2a (`ff22abc`):** the `point → NodeId` half, wiring serval's existing
    `ServalLaneView::hit_test` (no new spatial index). The reverse
    `SourceNodeId → NodeId` is trivial for `ScriptedDom` (`opaque_id` is the
    raw arena index, so `NodeId::from_raw` inverts it — no cached reverse
    map needed).
  - **2b (`9c01c27`):** faithful event dispatch (the chosen path over a
    native handler registry). `on_click` is an `OnEvent`-shaped view that
    registers its `view_path` in `ServalCtx`; `dispatch_click` bubble-walks
    `parentNode` and routes a `PointerClick` through the stock `xilem_core`
    message cycle (`MessageCtx`/`DynMessage`/`View::message`), then
    rebuilds. No `Rc<dyn Fn>` registry, no fork patch.
- **Stage 3 (breadth) — partly done.** Grows from "counter" toward
  authoring real chrome. Done so far:
  - **Component composition + Action-bubbling — done (`84fceae`).**
    `xilem_core`'s generic `lens` / `map_state` / `map_action` / `memoize`
    drive `ServalCtx` with **zero** backend impls (the identity
    `SuperElement` is the only bound they need), so reusable
    independently-stateful components compose for free. A sealed
    `OptionalAction` (mirroring `xilem_web`) lets `on_click`/`on_key`
    handlers return `()` or an `Action`; the runner gained a defaulted
    `Action = ()` generic and `dispatch_*` collect bubbled actions.
  - **Keyboard + focus — done (`09f2bf1`).** `on_key` (the faithful-routing
    twin of `on_click`), a serval-native `KeyEvent`/`Key`/`NamedKey`, a key
    registry on `ServalCtx`, runner `focus` with click-to-focus, and
    `dispatch_key` bubbling from the focused node.
  - **Form controls (text field) — done (`61135de`, caret `4ceac56`).**
    `text_field` over a `TextInput { text, caret }` model: a real
    insertion-point editor (char-indexed, Unicode-correct) with insert /
    Backspace / Delete / ←→, composable via `lens`, plus the winit→`KeyEvent`
    wiring in the demo bin. Now with a real glyph-positioned caret (see below),
    selection, and system clipboard.
  - **Capture phase — done (`abde0a9`).** `.capture(bool)` per listener
    (default bubble); `dispatch_click`/`dispatch_key` run a `root → target`
    capture pass then the `target → root` bubble pass, completing
    capture → target → bubble. Each node's lone listener fires in exactly
    one phase.
  - **`DOM → AccessKit` (production half) — done (`4f18f89`).**
    `accesskit_tree(dom, fragments, focus) -> accesskit::TreeUpdate` in
    `pelt-live`: walks the live `ScriptedDom`, maps each tag to a `Role`,
    takes accessible names from text content, reads bounds from the
    `FragmentPlane`, and points focus at the runner's focused node.
    Concretely demonstrates the thesis that a semantic DOM maps to an a11y
    tree directly (no widget→a11y translation). The live `accesskit_winit`
    adapter that surfaces it to a screen reader is the remaining half (see
    below).

  Since done:
  - **Live a11y adapter — done.** `accesskit_winit::Adapter` is wired into
    the `pelt-live-counter` window (deferred-event model via `UserEvent::
    Accessibility`), so the `accesskit_tree` builder's output reaches a real
    screen reader.
  - **Real caret painting — done.** `serval_layout::caret_rect` /
    `selection_rects` compute glyph-positioned geometry from the parley layout;
    the host overlays a caret bar (snug to the cap-height glyph band) and a
    line-box selection highlight on the scene. Replaces the `|` marker.
  - **`Element` / `Text` split — done.** An `ElementView` marker gates
    element-only ops (`on_click`/`on_key`/`attr`) at compile time while every
    node stays one runtime type (`ServalElement`), so children mix text +
    elements freely.
  - **Per-tag vocabulary + more named keys — done.** `div`/`span`/`p`/`input`/…
    helpers; Home/End.

  Still open:
  - **More pointer events** — `pointermove`/`pointerup`/wheel (the input half
    of scrolling + pointer gestures — a future Lane H axis).

- **Stage 4 (overlays + inline style) — done.** The host-overlay slice, and
  the engine capability it forced.
  - **Inline `style` support (`dfe8702`).** serval's stylo adapter returned
    `None` from `TElement::style_attribute`, so dynamic per-element styling was
    impossible. Now each element's `style="…"` is parsed (Author origin) before
    the cascade and cached on its `StyleEntry`; `style_attribute()` hands the
    block to the cascade, which applies it above author rules — like the
    browser. This is a Lane C **Styling**-axis deliverable: it unblocks *all*
    dynamic styling, not just overlays.
  - **Overlay primitives (`4bf92fb`).** `overlay_at(x, y, content)` — a
    `position: absolute` box placed by an inline style — plus `anchor_point(
    trigger, popup, Placement)` (pure geometry; the host feeds it the trigger's
    laid-out rect, since only the host has layout). The caller owns two serval
    realities: a positioned ancestor (no true viewport-`fixed`), and
    last-in-document order for stacking (the paint walk has no `z-index`).
  - **Live demo (`63939fb`).** Clicking `[ + ]` toggles a `.popup` card
    anchored below it: trigger → host rect query → `anchor_point` → inline-style
    cascade → layout → top-of-stack paint. Validated on screen 2026-05-31.

- **Stage 5 (erased views + `select`) — done.** The first composable control on
  the overlay/inline-style foundation, and the type-erasure it needed to reach
  a named app.
  - **Erased views / `AnyView` (`b6872ff`).** `impl AnyElement for ServalElement`
    so `Box<dyn AnyView<…>>` is a usable view — the foundation for dynamic /
    heterogeneous view trees (conditional content, mixed control types). The
    element type stays uniform (`NodeId`); the one new op is `replace_inner`, the
    in-place node swap when a boxed view changes concrete type, which needed the
    element `Mut` to carry its `parent` (now threaded by the splice + runner).
  - **`select` control (`31deed6`).** A self-positioning dropdown (see T2 below),
    composable via `lens`. Its concrete type is unnameable (per-option closures),
    so the demo holds it as `Box<dyn AnyView>` — the erased-view payoff: a named
    app view (`DemoView`) carries an opaque-typed control (`07a8e3b`).

- **Stage 6 (scrolling) — done.** A Lane H axis, wired from existing netrender
  primitives (`PushClip`/`PushTransform`) rather than a renderer change.
  - **Overflow clip (`3909fe7`).** `paint_emit` wraps an element's descendants
    in `PushClip`/`PopClip` (padding box) when `overflow != visible` — also fixes
    `overflow: hidden`/`clip` generally.
  - **Scroll-offset transform (`aa4fc14`).** A clipping container with an entry
    in a `ScrollOffsets<NodeId>` map translates its clipped content by `-offset`,
    so it scrolls under the fixed clip window.
  - **Host wheel + scrollbar (`d3606eb`, `1709ef6`).** `pelt-live` wheel input →
    clamped offset (vs `content_size` from the fragment); a scrollbar thumb on
    the right edge (height ∝ visible/content, pos ∝ offset/scrollable).
  - **Clip-aware hit-testing — done.** The first naive scroll hit-test leaked
    through clipped content. `ServalLaneView::hit_test` now respects overflow
    clips and per-node `ScrollOffsets`, and the `pelt-live` demo passes the same
    offsets to hit-testing that it passes to paint. `hit_test_is_clip_and_scroll_aware`
    pins the behavior.
  - **Known gap — no `z-index`.** (Closed by Stage 7 Tier 1 below.) Stacking was
    document order, so an overlay (the `select` dropdown) was covered by a later
    sibling (the scroller); the demo worked around it by ordering the select
    last. Full stacking contexts remain a CSS-conformance item.

- **Stage 7 (z-index Tier 1 + remaining form controls) — done.** Scoped in
  `2026-05-31_zindex_form_controls_scope.md`; closes the Stage 6 z-index gap and
  the open T2 controls.
  - **z-index Tier 1 (`e10a3211f82`).** `paint_emit` paints in two passes:
    in-flow in document order, then out-of-flow (`position: absolute`/`fixed`)
    elements on top, ordered by `(z-index, document order)`, each placed at its
    parent's accumulated absolute origin. So overlays paint above in-flow content
    regardless of order — the `select`-last workaround is no longer needed. Full
    CSS painting order / nested stacking contexts are Tier 2 (conformance).
  - **radio (`391af9f09d5`).** `radio_group(state, options)` over a
    `RadioGroup { selected }`: `select` minus the dropdown; `role=radiogroup` /
    `role=radio` + `aria-checked`, composable via `lens`.
  - **textarea (`99998dae76a`).** `textarea` over the existing `TextInput`:
    Enter → `\n`, Up/Down line navigation, line-scoped Home/End. serval feeds
    raw text to parley (which breaks at `\n`), so newlines render with no engine
    work. Hard-line nav (Tier 1); soft-wrap visual-line nav is a later layout-fed
    refinement.
  - **pointer-drag + slider — done.** `on_pointer` plus runner pointer capture
    route `pointerdown`/`move`/`up` to the captured element, and `slider` sits on
    that foundation. The same primitive is reusable for scrollbar-thumb drag,
    resize handles, and drag-tab-out.

## Toward the Mere flip gate: IME + form-control breadth

Mere's [serval-as-host decision brief](../../mere/design_docs/mere_docs/technical_architecture/2026-05-29_serval_as_host_evaluation.md)
gates flipping Mere's chrome onto serval on serval reaching Masonry's
interactive bar — concretely **IME + form-control breadth + the orrery
element decision** (its §8). The orrery is Mere-side; the two pieces this
backend owns are scoped here. Neither is a from-scratch build: both extend
the keyboard/focus + `text_field` foundation already landed.

### IME (the long pole)

Composed text input (CJK, dead-key accents, transliteration) goes through
the platform input-method editor: the OS reports *preedit* (in-progress
composition) and *commit* (final text), and wants the focused caret's
screen rect to place its candidate window. winit surfaces this as
`WindowEvent::Ime(Ime::{Enabled, Preedit(text, cursor), Commit(text),
Disabled})`, gated by `window.set_ime_allowed(true)` and steered by
`window.set_ime_cursor_area(position, size)`.

What the backend needs, in tiers:

- **T1 — commit-only. Done (`42d9d04c4e0`).** `set_ime_allowed(true)` on the
  window; `Ime::Commit(text)` inserts into the focused field by reusing the text
  path (a `Character` key event through `dispatch_key`). Latin typing still
  arrives as `KeyboardInput`; composed input (CJK, transliteration, dead-key
  accents) commits here.
- **T3 — candidate placement. Done (`42d9d04c4e0`).** `caret_screen_rect`
  (cascade → layout → `caret_rect` for the focused field) feeds
  `set_ime_cursor_area` after focus / caret-moving input, so the candidate
  window tracks the caret. Reuses the caret geometry.
- **T2 — preedit display. Done (`944c070018f`).** `TextInput` grew a
  `preedit: String` (the composing text), shown inline at the caret by
  `render_text()` but kept out of the committed buffer; `caret_byte_in_render()`
  places the caret after it. The field renders `render_text()`; `Ime::Preedit`
  sets the preedit (live composing display), `Ime::Commit` clears it + inserts,
  `Ime::Disabled` clears it.
- **Preedit underline. Done (`666e548d149`, `1ea6ab4c153`).** Added a general
  `text-decoration: underline` engine feature: the cascade's
  `text-decoration-line` flows to `InlineRun.underline` → parley
  `StyleProperty::Underline` → paint_emit draws the line (parley supplies the
  geometry, doesn't draw it). The field then renders its content as `(before,
  preedit-span, after)` and underlines the preedit span via inline style, so the
  composing run shows distinctly. (Closes the earlier "no text-decoration"
  caveat. `text-decoration-style` dotted/dashed/wavy is a later refinement; the
  underline is solid.)

**IME is complete** — all three tiers plus the underline-styled preedit —
closing the old long pole in the backend's half of the Mere-flip gate. The
remaining blocker list from this section is also closed in the current tree:
pointer-drag, slider, Tab traversal, and clip-aware scroll hit-testing are
implemented and covered by tests.

IME pays once for the whole engine: serval needs it for content text entry
regardless, so the host chrome gets it for free once content has it (the
brief's §4 framing).

### Form-control breadth

The demo has one control (`text_field`). Mere chrome needs a small but real
set, each a reusable `xilem_serval` component over a tiny state model (the
`text_field`/`counter_button` pattern), built on `on_click`/`on_key`/focus:

- **T1 — the chrome essentials.** `button` (have, as `on_click` on an
  element), `checkbox`/`toggle` (a bool over `on_click`), single-line text
  input (have), and **focus traversal** — `Tab`/`Shift+Tab` moving focus
  across focusable elements in document order. The runner implements traversal
  over the key-handler focusable set, including wraparound.
- **T2 — the common rest.** `select`/dropdown — **done** (`31deed6`): a
  self-positioning control over a `SelectState { selected, open }`, its option
  list `position: absolute; top: 100%` inside the relative select box (so it
  needs no host anchor, unlike `overlay_at`). `radio` group, multi-line
  `textarea`, `slider`/range, and focus traversal are also done. Slider rides
  pointer drag over a track (`pointerdown`/`move`/`up` + capture).
- **T3 — text-editing depth.** Selection (caret → anchor+focus range, shown
  as a highlight), and clipboard (cut/copy/paste via winit's clipboard +
  the selection). Selection geometry shares the caret-rect capability.

The tiering matches platen's needs (§7 of the brief): docked-tile chrome
wants buttons, text fields, and tab-traversable focus first; richer controls
follow as the chrome demands them.

### Shared capability: caret + selection geometry

Three open items — **real caret painting**, **IME candidate placement**
(T3 above), and **text selection** (form T3) — all need the same primitive:
*the screen rect of a character offset within a text node's laid-out run.*
serval already caches the `parley::Layout` per text leaf
([text_measure.rs](../components/serval-layout/text_measure.rs)); the
primitive is a query over it (parley's cursor/selection geometry) returning
a rect for `(text node, byte offset)`. Building it once unblocks all three,
so it is the natural next serval-layout addition when this gate work starts —
and it is the reason caret painting is grouped here rather than treated as a
cosmetic one-off.

## What serval makes simpler

`xilem_web` defers prop application to `PodMut::drop` to batch DOM writes.
serval already batches at the `drain_mutations` → relayout boundary, so
`xilem_serval` can apply eagerly (each `set_attribute` records a
`DomMutation`) and skip the deferred-prop machinery. The batch boundary is
the relayout, not the `Mut` drop. The backend is therefore simpler than
its web sibling.

Accessibility is also more natural here: serval emits `AccessKit` from a
semantic DOM rather than from an arbitrary widget tree. The genuine
engine-completeness costs are form controls and that a11y emission, not
the reactive layer.

## Crate placement

A backend *library* crate (e.g. `components/xilem-serval`, or a sibling
location if it should sit nearer the host than the engine) consumed by a
`pelt` example host. serval already depends on the `crates/xilem` fork
(via [pelt-viewer](../ports/pelt-viewer/Cargo.toml)), so the dependency
direction is established, and an Xilem-authored example beside the static
viewer fits pelt's multi-host-reference role. One maintenance cost to name
plainly: `xilem_core`'s API churns, so the backend tracks it; the fork is
local, so the pace is ours to set.

## Relationship to existing docs

- The **scripted render loop** (JS → DOM → `IncrementalLayout` →
  netrender → present) is the content-side twin of this host-side work.
  Both are gated on the same Gap 2 (wiring the existing hit-test into
  native dispatch). The scripted tier's W0 (DOM surface, node-level
  `EventTarget`) is in
  [2026-05-26_pluggable_engines_testharness_plan.md](./2026-05-26_pluggable_engines_testharness_plan.md);
  its capture/target/bubble `EventTarget` algorithm in
  [script-runtime-api/dom.rs](../components/script-runtime-api/dom.rs) is
  the JS twin of Gap 2's native dispatch, and the two must converge on one
  event model rather than fork.
- The **Blitz/serval convergence** thesis (serval as a Blitz-shaped
  modular engine) is what architecture (3) realizes; this doc is the
  authoring-layer half of that bet.
- [2026-05-25_web_platform_api_shared_middle_plan.md](./2026-05-25_web_platform_api_shared_middle_plan.md)
  and the pluggable-engines plan own the DOM/JS surface `xilem_serval`
  mutates; this doc consumes that surface natively rather than through JS.
- [2026-05-25_js_execution_strategy.md](./2026-05-25_js_execution_strategy.md)
  must be read alongside this, because "scripting" and "app UI" are
  different axes. `xilem_serval` is **Rust app-authoring**, independent of
  any JS engine; the engine axis stays native Nova-first with wasm moving
  to Boa (and weval AOT). Older script-plan language that conflates content
  JS with app authoring should be reconciled to this split: app chrome is
  authored in Rust through `xilem_serval`, content JS runs in the engine,
  and both drive the same serval DOM through separate roots.

## Non-goals

- Replacing the Xilem-chrome + serval-content split (architecture 1). That
  stays the working setup; this enables architecture 3 as an evaluable
  path, not an immediate migration.
- A complete element/view vocabulary. Stage 1 needs only enough to render
  a non-trivial reactive tree; breadth ratchets later.
- Performance of the binding. Correctness and shape first.
- Rendering chrome through serval on top of a host framework
  (architecture 2). Excluded by design.
