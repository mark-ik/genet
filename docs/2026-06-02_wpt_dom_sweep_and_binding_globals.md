# WPT dom/ sweep (both engines) + binding globals

Status: **2026-06-02.** Follow-up to the regress regex swap. Widened the WPT
testharness sweep across the whole `dom/` tree on both engines, and added the
top missing host globals to the binding bootstrap. No regressions.

## (2) Widened WPT sweep

Previously swept: `dom/nodes`, `dom/traversal`, `dom/collections`. Now the rest of
`dom/`, on Boa and Nova (release `serval-wpt`):

| subset | files | nova panics | boa panics | boa↔nova divergences |
|---|---|---|---|---|
| dom/events | 153 | 0 | 0 | 4 |
| dom/ranges | 55 | 0 | 0 | 0 |
| dom/lists | 5 | 0 | 0 | 0 |
| dom/abort | 3 | 0 | 0 | 0 |

So across the **entire** `dom/` tree: **zero panics on either engine**, and the
only divergences are a thin subtest-**count** tail (4 in dom/events, 3 in
dom/nodes). The regress swap + the WTF-8 indexing fixes closed every engine-level
divergence in `dom/`; both engines now agree on pass/fail status everywhere and
differ only in how many subtests a few generated-test files enumerate.

### The count tail is not a missing-global gap

The "3 dom/nodes divergences" were assumed to be missing-global gaps. They are
not. `Node-parentNode.html` has five `test()` calls: **Nova's 5 subtests is
correct; Boa reports 10** (a 2× over-count). `ParentNode-replaceChildren` has
Nova running *more* than Boa (29 vs 18). These are feature-dependent /
harness-artifact subtest-count differences, both engines FAILing either way —
not something a host global fixes (confirmed: adding HTMLElement left all three
unchanged). Tracked, low priority.

## (3) Binding globals

The sweep's error messages did show real missing host globals corpus-wide (top:
`HTMLElement` ~49×, then `frames`, `customElements`, per-tag interfaces). Added
to the DOM bootstrap (`components/script-runtime-api/dom.rs`):

- **`HTMLElement`** — inserted into the prototype chain
  (`HTMLElement.prototype → Element.prototype → Node.prototype`); `wrapNode` now
  gives every element this prototype. Makes `instanceof HTMLElement` and
  `class X extends HTMLElement` work (the single biggest missing global) while
  keeping `instanceof Element`. The static-DOM harness only has HTML elements, so
  this is correct for it.
- **`customElements` + `CustomElementRegistry`** — a minimal registry
  (`define`/`get`/`getName`/`whenDefined`/`upgrade`) that records definitions and
  validates names. Clears the `ReferenceError`s; it does **not** upgrade existing
  elements (no live mutable tree here).
- **`frames`** — the window itself (no child browsing contexts in the harness).

Verified no regressions: dom/nodes still 0 panics / 3 count divergences;
dom/lists unchanged; Document-createElement and Element-classList stay
byte-identical Boa↔Nova. custom-elements/ tests now *run* (globals exist) instead
of erroring, though most still fail.

## Not done (larger efforts, deliberately deferred)

- **Per-tag HTML interface hierarchy** (`HTMLButtonElement`, `HTMLDivElement`, …):
  ~100 interfaces plus a tag→interface map so `createElement('button')` instances
  are `HTMLButtonElement`. Customized-built-in custom-element tests need it.
- **Real custom-element upgrade / lifecycle / shadow DOM**: the registry stub
  records definitions but does not construct or upgrade. custom-elements/ needs
  this to pass (3/2807 subtests today).
- **`URL`**: needs a real WHATWG parser; best wired as a native fn over the Rust
  `url` crate rather than a fragile JS reimplementation. Not a top `dom/` gap.
