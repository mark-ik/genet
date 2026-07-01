# HTML interface table follow-ups (spun out at archive)

**Date:** 2026-07-01. **Parent (archived):**
`archive/2026-06-24_html_interface_table_plan.md`. **Why:** the plan reached
all-phases-done (I1-I3) against its own stated done-conditions, confirmed by a
`html/dom` + `custom-elements` WPT measurement, and was archived. This carries
its residual spec-fidelity gaps so they stay live, prioritized by what that
measurement actually found (a verbose `custom-elements --engine boa` run,
156 failing/errored files categorized below) rather than by guesswork.

## Highest-yield: `adoptedCallback` (unimplemented)

`custom-elements/adopted-callback.html` (142 subtests) and
`custom-elements/registries/adoption.window.html` (36 subtests) both fail
outright — grepping `bootstrap.js` turns up no `adoptedCallback` wiring at
all. 178 subtests behind one callback, same shape as the
connected/disconnected/attributeChanged reactions I3 already landed
(`enqueueCustomElementReaction` + the microtask-scheduled flush). Cheapest,
highest-yield item here — same "small mechanism, big return" shape I1 was for
the whole plan.

## New gap this measurement surfaced: form-associated custom elements / `ElementInternals`

Not named in the parent plan at all. ~20 files across
`custom-elements/form-associated/` plus the top-level `ElementInternals-*.html`
tests fail or error, mostly with "not a callable function" / opaque
`JsError` — confirmed by grep: `attachInternals` and `ElementInternals` don't
exist anywhere in `bootstrap.js`. This is a whole absent feature, roughly the
size of the rest of I3's remaining gaps combined. Worth its own scoped plan
rather than folding into a quick fix here.

## Named in the parent plan, still open

- **Parser-created custom-element timing** — ~10 files under
  `custom-elements/parser/` (`parser-constructs-custom-element-synchronously`,
  `parser-uses-constructed-element`, `parser-fallsback-to-unknown-element`,
  foreign-content and `document.write` variants). Spec requires the parser to
  construct custom elements synchronously during tree construction, not just
  upgrade them after the fact.
- **Construction failure states** — `HTMLElement-constructor*`,
  `customized-built-in-constructor-exceptions`,
  `htmlconstructor/newtarget*`, `microtasks-and-constructors`,
  `perform-microtask-checkpoint-before-construction`, `registries/Construct.html`,
  `constructor-reentry-with-different-definition`. About a dozen files
  exercising the `NewTarget` / prototype-swizzling / reentrancy edge cases the
  spec's "constructing an element" algorithm is fussy about.
- **Fuller custom-element name validation + registry edge cases** —
  `CustomElementRegistry.html` (42/92 today) and
  `CustomElementRegistry-getName.html` carry the remaining validation-ordering
  and error-shape gaps in the registry API surface.
- **Fuller spec reaction-stack semantics** — no single failing file isolates
  this; it's the general rigor gap behind the reaction queue being a flat
  array today rather than the spec's backup-element-queue stack. Lower
  priority than the items above, which each have concrete failing tests.

## Out of scope (already covered by the parent plan's non-goal)

`element-internals-shadowroot.html`, `reactions/ShadowRoot.html`, and the
`registries/ShadowRoot-*` / scoped-registry tests fail, but this is Shadow DOM
/ scoped-custom-element-registry surface — already excluded by the parent
plan's "Shadow DOM / slotting beyond what custom-elements reactions strictly
need" non-goal. Don't re-litigate; re-open only if a shadow-DOM plan lands
first and wants to claim them.

## Smaller nit, already known

`HTMLVideoElement` and `HTMLAudioElement` (`components/script-runtime-api/dom/html_interfaces.rs`)
both declare `parent: "HTMLElement"` directly and duplicate
`src`/`crossOrigin`/`preload`/`autoplay`/`loop`/`controls` instead of
inheriting them from a shared `HTMLMediaElement` entry. Works today (the
duplication is functionally harmless), but `instanceof HTMLMediaElement` is
false for both, which idlharness tests will catch. Trivial fix whenever
someone's back in that file for another interface.
