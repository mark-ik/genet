# HTML interface hierarchy + interface-table plan

**Status (archived 2026-07-01):** All three phases done, each verified against
its own stated done-condition rather than just landed-in-prose: I1's mechanism
is real (`createElement` resolves per-tag from `html_interfaces.rs`, not
hand-wiring), I2's payload is measured (html/dom 40708/59675 subtests on Boa,
near-parity on Nova), and I3's upgrade + reaction queue is measured
(custom-elements 404/3877 on Boa, materially off the 3/2807 baseline).
Residual spec-fidelity gaps — `adoptedCallback` (unimplemented, ~178
subtests), form-associated custom elements / `ElementInternals` (a
previously-unnamed gap this measurement surfaced), parser-created-element
timing, and construction-failure states — spun out to
`2026-07-01_html_interface_table_followups.md` before archiving. Kept here as
the record of the phase design + the as-built progress log.

---

**Date:** 2026-06-24
**Status:** All phases (I1, I2, I3) done — reconciled 2026-07-01. I1 is
landed, I2's payload is landed and measured (html/dom 40708/59675 subtests on
Boa, near-parity on Nova), and I3's upgrade+reaction slice is landed and
measured (custom-elements 404/3877 on Boa, materially off the 3/2807
baseline). Spun out of the grand audit (`2026-06-24_grand_audit.md` §2, lever
4); continued the DOM-binding sweep
(`2026-06-02_wpt_dom_sweep_and_binding_globals.md`).
**Thesis:** serval's DOM surface is built from a ~900-line JS bootstrap string with exactly one per-tag subclass wired (HTMLCanvasElement). Scaling to the ~100 HTML interfaces by hand-extending that string does not scale. Build the mechanism first (a declarative interface table the bootstrap consumes), then the ~100-interface payload is incremental, and custom-elements customized-built-ins are unblocked.

## Original gap, code-grounded

- At plan time, `createElement('button')` returned a generic HTMLElement; per-tag HTML interfaces present = 2 of ~100 (HTMLElement, HTMLCanvasElement).
- At plan time, the whole DOM surface was a template-literal bootstrap string starting at `components/script-runtime-api/dom.rs:1066`; `elementSubclassProto` hung subclasses at `:1154`, `HTMLElement.prototype` at `:1985`, and the only wired subclass (HTMLCanvasElement) was `:1989-2015`.
- The prototype-chain + reflected-attribute machinery already existed to hang more on; what was missing was a declarative way to declare interfaces and their reflected IDL attributes instead of growing the string.

## Phases (done-conditions, not dates)

### I1 — Interface table mechanism

Introduce a declarative table (Rust-side data, or a minimal codegen) describing each interface: name, parent interface, the tags that instantiate it, and its reflected IDL attributes (name, type, default, content-attribute mapping). The bootstrap builds prototypes and reflected accessors by iterating the table rather than by hand-written string entries.
- **Done when** HTMLCanvasElement is expressed as a table entry (not bespoke string), `createElement` resolves the right interface per tag from the table, and adding a new interface is a table entry plus its reflected-attribute rows.

### I2 — The ~100-interface payload

Populate the table for the common HTML interfaces (anchor, button, input family, img, form, select/option, table family, media, div/span/p, headings, list, etc.), prioritized by WPT html/dom + idlharness yield. Reflected IDL attributes drive a large share of html/dom subtests (the audit notes a reflected-IDL-attribute lever moved html/dom 4936 -> 35515 subtests).
- **Done when** the common interfaces resolve with their prototype chain + reflected attributes, measured by a html/dom subtest delta on the harness (post the WPT-harness plan's H1/H2 so the count is trustworthy).
- **Measured 2026-07-01** (same day H1 and H2 both landed): `html/dom` is 22 all-pass / 158 with-failures / 64 errored / 1 no-results / 140 skipped of 385 files, subtests **40708/59675** on Boa and 40715/59671 on Nova — same coarse buckets, confirming H2's Nova stack-stability fix holds beyond the plain `dom` subset. The last recorded figure (35515 passing, 2026-05-26) predates I1/I2 *and* the H1 manifest-discovery switch, so it isn't a clean before/after baseline; treat 40708/59675 as the new reference point rather than a precise delta.

### I3 — custom-elements upgrade + reaction queue

With a real interface hierarchy and the gc_arena mutable tree, build customized-built-ins: upgrade existing elements on `define`, the reaction queue, and microtask-timed callbacks.
- Today: `customElements` registry stub exists (`dom.rs` ~2528: define/get/whenDefined) but does not upgrade existing elements (the dom-sweep doc confirms "does not upgrade existing elements"); custom-elements is 3/2807.
- **Done when** `define` upgrades matching existing elements, reactions fire at the right microtask checkpoints, and custom-elements moves materially off 3/2807.
- **Measured 2026-07-01:** `custom-elements` is 2 all-pass / 147 with-failures / 27 errored / 1 no-results / 10 skipped of 187 files, subtests **404/3877** on Boa (375/3689 on Nova) — materially off the 3/2807 baseline. Some of the denominator growth (2807 -> 3877) is the H1 manifest-vs-walk discovery switch rather than new tests, but the pass-count jump (3 -> 404) is real signal from the upgrade+reaction work. Remaining I3 items below aren't yet reflected in further gains.

## Sequencing

I1 -> I2 -> I3. I3 depends on both the interface hierarchy (I1/I2, for customized built-ins) and the mutable arena tree from the gc_arena DOM work. Run measurement on top of the WPT-harness plan (H1/H2) so deltas are real, not artifacts of mis-enumeration.

## Honest hedge

The dom-sweep work found that adding HTMLElement alone did not fix the dom/nodes count-tail divergences. The win here is breadth across many small per-interface tests plus the reflected-attribute multiplier, not one systematic jump. Confidence on the magnitude is medium; the mechanism (I1) is the high-confidence, load-bearing piece.

## Non-goals

- Web-IDL codegen from `.idl` files (a heavier path; a hand-maintained table is the proportionate mechanism for now, and can be generated later without changing the consumers).
- Shadow DOM / slotting beyond what custom-elements reactions strictly need.

## Reference shape for I1: formal-web's runtime bindings registry

The gterzian/formal-web harvest (`2026-06-24_formal_web_lessons.md`) supplies a
working reference for the I1 table mechanism over the *same engine* (Boa), worth
mirroring rather than re-deriving:

- A `WebIdlInterface` trait whose `define_members` pushes `OperationDef` /
  `AttributeDef` / `ConstantDef`; `register_interface_spec::<T>(ctx)` materializes
  the prototype and installs members as Boa `NativeFunction`s. This *is* the
  declarative interface table I1 calls for, expressed as a trait + registration
  call rather than a data table (either works; the trait form gives type-checked
  member signatures).
- Platform objects are `#[derive(Trace, Finalize, JsData)]` Rust structs inside
  `JsObject`s; `downcast_ref::<T>()` is the runtime type check (the WebIDL
  "inherited interfaces" operation). This gives typed, GC-traced native nodes in
  place of the bootstrap string's opaque handles.
- Exotic objects (WindowProxy, Location) use Boa's **public** `JsProxyBuilder`
  traps and never touch `pub(crate)` internals — respects serval's no-fork-deps
  doctrine verbatim.
- Async operations keep promise resolvers in a **side table keyed by
  `request_id`**, so algorithm bodies hold no `JsValue`. This maps onto serval's
  `new_host_promise` / `settle_host_promise` (`components/script-engine-api/lib.rs:201`)
  and is what keeps the binding engine-neutral — the prerequisite for the dual
  Boa/Nova goal (a `JsValue`-free interface table can back either engine).

Take this as the I1 target shape; the `JsValue`-free, public-API-only constraints
are the load-bearing ones for serval.

## Findings

- 2026-06-24 (grand audit, verified): script-runtime-api `dom.rs` is 3,259-3,737 LOC; the bootstrap-string pattern is the named manual ceiling. Bound interface counts today: ~16 DOM globals, ~15 fetch/stream/encoding globals, 6 WebGL interfaces, ~40 native DOM sinks, 2 per-tag HTML interfaces.

## Progress

- 2026-06-24 — Plan created from the grand audit. I1 (the table mechanism) was the entry point and the highest-confidence piece.
- 2026-06-30 — I1 landed: `components/script-runtime-api/dom/html_interfaces.rs` owns the Rust-side interface table, `install_dom_surface` injects it before `bootstrap.js`, and `HTMLCanvasElement` is table-driven rather than bespoke string wiring. Focused verification: `cargo test -p script-runtime-api html_interface_table --lib` passes on Boa and Nova.
- 2026-06-30 — I2 broad payload present: common HTML interfaces now resolve through the table with per-interface reflected attributes, including anchors, buttons, inputs/forms, image/canvas/media, table family, headings, lists, body/head/title, and related legacy reflected attributes.
- 2026-06-30 — I3 first slice landed: the registry records autonomous definitions and customized built-ins, `document.createElement(tag, { is })` records the `is` attribute and upgrades when defined, `define()` upgrades matching existing document nodes, class constructors get a construction-stack target for `super()`, and `customElements.upgrade(root)` handles detached subtrees. Focused verification: `cargo test -p script-runtime-api custom_elements_customized_builtins --lib` passes on Boa and Nova.
- 2026-06-30 — I3 reaction slice landed: custom-element reactions are queued onto the runtime's Promise microtask checkpoint; `connectedCallback`, `disconnectedCallback`, and observed-attribute `attributeChangedCallback` run after the current script, including initial observed attributes during upgrade. Focused verification: `cargo test -p script-runtime-api custom_elements_customized_builtins --lib` passes on Boa and Nova.
- 2026-07-01 — First post-landing measurement, taken the same day the WPT-harness plan's H2 gate cleared (broad `dom --engine nova` stack-overflow fix): `html/dom` 40708/59675 subtests on Boa, 40715/59671 on Nova (matching coarse buckets); `custom-elements` 404/3877 on Boa, 375/3689 on Nova. Both engines run these subsets cleanly post-H2, and custom-elements is now materially off its 3/2807 starting point. See the measured notes under I2/I3 above for caveats on what's a clean delta vs. a discovery-methodology shift.
- 2026-07-01 — Plan archived: all three phases meet their stated done-conditions. A verbose run of `custom-elements --engine boa` categorized the 156 remaining failing/errored files; residual spec-fidelity gaps (led by unimplemented `adoptedCallback` at ~178 subtests, and a previously-unnamed form-associated-custom-elements/`ElementInternals` gap the measurement surfaced) are spun out to `2026-07-01_html_interface_table_followups.md` rather than tracked here.
