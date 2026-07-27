# Text-Editing Primitive Plan

**Date:** 2026-07-25
**Status:** T0 through T4 complete 2026-07-27. The shared primitive, platform
translation, shaped-layout seam, and first real app routes are live. T5 forms
and T6 contenteditable remain downstream consumers; they do not gate knot.
**Companions:** the
[pelt and knot direction](2026-07-24_pelt_knot_direction.md) (the ruling this
plan owns), mere's `2026-07-25_knot_port_plan.md` (K7 is the third consumer),
the [inker/genet adoption plan](2026-07-09_inker_genet_adoption_plan.md).

## The ruling, restated

Editable text is owed three times over: cambium's `text_input` needs real
selection, IME, and undo to be a credible toolkit control; the fullweb lane
needs `input` and `textarea` for real pages, then contenteditable; the knot
editor needs the same machinery. Build it once at the cambium/genet primitive
layer, three consumers on top. `knot-editor-host` stays a consumer, which it
already is architecturally (its `KnotReadout` derives highlights, outline,
folds, and preview from a buffer the host owns; it holds no buffer itself).

This plan exists so the ruled item has an owner. Until it was founded, text
editing was the one ruled capability no plan carried.

## Settled ownership

- Cambium's `TextInput` owns committed text, directed selection, transient
  composition, ghost text, and a bounded undo journal. Its stored caret unit is
  an extended grapheme cluster.
- `TextCommand` is the one platform-neutral mutation vocabulary. DOM controls
  and hosts with their own action spine lower keys and IME events into the same
  commands.
- The Cambium/Genet boundary is byte offset plus visual affinity. Genet and
  Parley own shaped visual movement, bidi order, soft-wrap lines, point
  hit-testing, selection rectangles, and caret geometry.
- `cambium-winit` translates winit key and IME payloads. The host still decides
  which app shortcuts it owns before dispatching the remainder.

## Phases

- **T0. Inventory and home ruling. Complete.** The existing `TextInput`,
  `EditHistory`, winit key translation, Parley selection API, Genet retained
  layout, Woodshed routing, Isometry capture lanes, and Mere document selection
  were checked live. The ownership split above is the result.
- **T1. Selection and caret model. Complete.** Logical editing is
  grapheme-correct. The layout boundary retains byte offsets and affinity.
  Parley supplies visual cluster, word, line, Home/End, bidi, and point
  movement. Mixed-direction and soft-wrap fixtures exercise the layout without
  a widget.
- **T2. Edit operations and undo. Complete.** All mutation enters through
  `TextCommand`; the bounded journal coalesces typing and preserves redo rules.
  An exhaustive four-operation wall over seven edit operations proves undo
  restores the original bytes for every sequence.
- **T3. IME composition. Complete at the code-path level.** Preedit text and
  its byte selection, commit, cancel, selection replacement, inline rendering,
  and retained-layout candidate geometry are wired. `cambium-winit` tests the
  exact winit payload translation on Windows. A physical IME interaction remains
  a headed receipt, not an implementation dependency.
- **T4. First consumers. Complete.** Cambium fields use the shared command path
  for selection, IME, and undo. Woodshed routes search and card rename through
  it with visual-order keys, click-drag selection, overlays, and candidate
  placement. Isometry's command lane now focuses a Cambium field, preserves
  Escape/Enter as app commands, and dispatches remaining keys and IME events.
  This resolves the host-routing question recorded 2026-07-26 without requiring
  every host to surrender its shortcut policy.
- **T5. Fullweb forms.** `input` and `textarea` route through the same core
  in the document lane. Sequenced with the cutover's needs, not ahead of
  them.
- **T6. contenteditable.** After forms, per the direction doc's ordering.

The knot editor (mere's K7) consumes the same core through its host; that
work lives in the knot port plan, not here.

## Non-goals

- Not an IDE substrate; the direction doc's ruling stands (the destination
  is the authoring browser).
- No second text stack: parley is the shaping layer per the established
  direction; cosmic-text stays out.
- No porting of another toolkit's editor wholesale; mature implementations
  (masonry/parley editor code, xi lineage) are read for technique per the
  borrow-technique practice, and anything lifted follows the founding
  license convention.

## Progress

- **2026-07-25.** Founded, phases sketched, queued behind livery. No code.
- **2026-07-26.** A fourth would-be consumer surfaced and was turned away on
  purpose. Isometry's obviation lane reached `caret_text_field`, found it
  unadoptable, and stopped rather than hand-rolling around it; the host
  key-routing constraint that blocked it is now recorded under T4, where it is
  evidence for T0's IME/host inventory instead of a surprise later. Nothing here
  started; the queue position is unchanged.
- **2026-07-26.** One of that consumer's two blockers cleared, and it was the
  boring one: `cambium-winit` split so its key translation is publishable
  (`cambium-winit-a11y` now holds the parts that cannot be). T0's inventory of
  "what IME events genet-winit-host already delivers" has a working reference to
  read: **woodshed already routes winit keys into the DOM**, translating with
  `key_event_from_winit` and handing off to `runner.dispatch_key` after its own
  shortcuts decline (`woodshed-genet/src/main.rs`). That is the seam Isometry
  lacks, already built and in use, so T3/T4 can start from a real
  implementation rather than a design.
- **2026-07-27.** T0 through T4 landed. Cambium now has a grapheme-indexed
  `TextInput`, byte-plus-affinity layout adapters, `TextCommand`, composition
  state, and bounded built-in history. Genet retained layout exposes Parley's
  visual cluster, word, line, Home/End, bidi, point, caret, and selection
  geometry. `cambium-winit` preserves IME preedit ranges. Woodshed is the
  layout-aware consumer and Isometry's command line is the host-capture
  consumer. Verification: Cambium 159 tests, cambium-winit 5, genet-layout 326,
  genet-render 10; Isometry 57 tests plus the desktop binary target; Woodshed
  20 tests plus the desktop binary target. The full Genet workspace check is
  clean with the live Buckram and Livery work present. Warnings were
  pre-existing.
