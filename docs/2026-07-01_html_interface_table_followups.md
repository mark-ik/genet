# HTML interface table follow-ups (spun out at archive)

**Date:** 2026-07-01. **Parent (archived):**
`archive/2026-06-24_html_interface_table_plan.md`. **Why:** the plan reached
all-phases-done (I1-I3) against its own stated done-conditions, confirmed by a
`html/dom` + `custom-elements` WPT measurement, and was archived. This carries
its residual spec-fidelity gaps so they stay live, prioritized by what that
measurement actually found (a verbose `custom-elements --engine boa` run,
156 failing/errored files categorized below) rather than by guesswork.

## Highest-yield: `adoptedCallback` (minimal core landed 2026-07-02)

The outright "no wiring at all" gap is closed. `bootstrap.js` now tracks
per-node `ownerDocument`, updates it on cross-document insertion and
`Document.adoptNode`, and enqueues `adoptedCallback(oldDocument, newDocument)`
through the same microtask reaction queue already used for
connected/disconnected/attributeChanged. Focused Boa/Nova runtime tests cover:

- detached custom element insertion into another document,
- connected cross-document moves (`disconnected` -> `adopted` -> `connected`),
- `Document.adoptNode` for detached and connected elements,
- ancestor moves carrying adopted custom-element descendants.

What remains from the old bucket is the broader registry/shadow-root adoption
surface. The local WPT path is `custom-elements/registries/adoption.window.js`
now, and its remaining failures depend on missing Shadow DOM /
`customElementRegistry` support rather than on `adoptedCallback` still being
absent.

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
- **Construction failure states** — the direct HTML-constructor core landed on
  2026-07-02: interface constructors now consult the registry's constructor
  table, `new HTMLElement()` and unregistered/wrong-base constructors throw
  `TypeError`, registered autonomous/customized constructors mint real elements,
  and `Reflect.construct(HTMLElement, [], PlainCtor)` now uses the registered
  custom-element definition instead of only the upgrade-time construction
  stack. What still remains in the old bucket is the harder failure/reporting
  side: `Document-createElement*` fallback-to-unknown behavior, returned-value
  validation, exact proxy-`NewTarget` / prototype-access-count fidelity,
  parser-time construction failures, and reentrancy corners like
  `constructor-reentry-with-different-definition`.
- **Fuller custom-element registry semantics** — the contained validation /
  ordering slice is now landed: valid-name checks reject uppercase + reserved
  names, `whenDefined` keeps one pending promise per unresolved valid name and
  rejects invalid names, `getName` type-checks its argument, `define` enforces
  an element-definition-running flag, and constructor/prototype property access
  follows the WPT-observed order (`prototype`, callback probes,
  `observedAttributes` when needed, then `disabledFeatures` /
  `formAssociated`). What still remains in `CustomElementRegistry.html`,
  `CustomElementRegistry-getName.html`, and `registries/Construct.html` is the
  larger constructor-failure / `NewTarget` / shadow-registry side, not the
  small API-contract holes.
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

## Smaller nit, landed 2026-07-02

`HTMLVideoElement` and `HTMLAudioElement` (`components/script-runtime-api/dom/html_interfaces.rs`)
now inherit from a shared `HTMLMediaElement` entry instead of each duplicating
the common reflected surface under `HTMLElement`. `instanceof HTMLMediaElement`
is now true for both audio and video, which closes the idlharness-visible
inheritance hole that used to sit here.
