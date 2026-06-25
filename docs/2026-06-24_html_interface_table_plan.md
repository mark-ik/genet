# HTML interface hierarchy + interface-table plan

**Date:** 2026-06-24
**Status:** plan. Spun out of the grand audit (`2026-06-24_grand_audit.md` §2, lever 4); continues the DOM-binding sweep (`2026-06-02_wpt_dom_sweep_and_binding_globals.md`).
**Thesis:** serval's DOM surface is built from a ~900-line JS bootstrap string with exactly one per-tag subclass wired (HTMLCanvasElement). Scaling to the ~100 HTML interfaces by hand-extending that string does not scale. Build the mechanism first (a declarative interface table the bootstrap consumes), then the ~100-interface payload is incremental, and custom-elements customized-built-ins are unblocked.

## The gap, code-grounded

- `createElement('button')` returns a generic HTMLElement; per-tag HTML interfaces present = 2 of ~100 (HTMLElement, HTMLCanvasElement).
- The whole DOM surface is a template-literal bootstrap string starting at `components/script-runtime-api/dom.rs:1066`; `elementSubclassProto` hangs subclasses at `:1154`, `HTMLElement.prototype` at `:1985`, and the only wired subclass (HTMLCanvasElement) is `:1989-2015`.
- The prototype-chain + reflected-attribute machinery already exists to hang more on; what is missing is a declarative way to declare interfaces and their reflected IDL attributes instead of growing the string.

## Phases (done-conditions, not dates)

### I1 — Interface table mechanism

Introduce a declarative table (Rust-side data, or a minimal codegen) describing each interface: name, parent interface, the tags that instantiate it, and its reflected IDL attributes (name, type, default, content-attribute mapping). The bootstrap builds prototypes and reflected accessors by iterating the table rather than by hand-written string entries.
- **Done when** HTMLCanvasElement is expressed as a table entry (not bespoke string), `createElement` resolves the right interface per tag from the table, and adding a new interface is a table entry plus its reflected-attribute rows.

### I2 — The ~100-interface payload

Populate the table for the common HTML interfaces (anchor, button, input family, img, form, select/option, table family, media, div/span/p, headings, list, etc.), prioritized by WPT html/dom + idlharness yield. Reflected IDL attributes drive a large share of html/dom subtests (the audit notes a reflected-IDL-attribute lever moved html/dom 4936 -> 35515 subtests).
- **Done when** the common interfaces resolve with their prototype chain + reflected attributes, measured by a html/dom subtest delta on the harness (post the WPT-harness plan's H1/H2 so the count is trustworthy).

### I3 — custom-elements upgrade + reaction queue

With a real interface hierarchy and the gc_arena mutable tree, build customized-built-ins: upgrade existing elements on `define`, the reaction queue, and microtask-timed callbacks.
- Today: `customElements` registry stub exists (`dom.rs` ~2528: define/get/whenDefined) but does not upgrade existing elements (the dom-sweep doc confirms "does not upgrade existing elements"); custom-elements is 3/2807.
- **Done when** `define` upgrades matching existing elements, reactions fire at the right microtask checkpoints, and custom-elements moves materially off 3/2807.

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

- 2026-06-24 — Plan created from the grand audit. No code yet. I1 (the table mechanism) is the entry point and the highest-confidence piece.
