# Text-Editing Primitive Plan

**Date:** 2026-07-25
**Status:** founded as the owner of the 2026-07-24 ruling; scoping only, no
code task yet. Queued behind livery focus per the
[cutover plan](2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md)'s
sequencing; T0 is cheap and can run whenever a gap in that cadence opens.
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

## Phases

- **T0. Inventory and the home ruling.** Verify what actually exists before
  designing: what cambium's `text_input` does today; what parley ships for
  editing (its editor support, grown for masonry's text input, is the
  candidate floor — evaluate rather than assume); what IME events
  genet-winit-host already delivers per platform; what selection machinery
  the fullweb lane has. Then rule the home: one core crate or module (the
  buffer, selection model, edit ops, undo journal, IME composition state)
  with cambium and the document lane as its two direct consumers, and where
  precisely it lives. Done when the inventory is written into this plan and
  the home is ruled with Mark.
- **T1. Selection and caret model.** Grapheme-correct movement, affinity,
  bidi-aware ranges, word and line units, over parley shaping. Done when a
  fixture wall covers movement and selection invariants without any widget
  attached.
- **T2. Edit operations and undo.** Splice, insert, delete by unit, an undo
  journal with coalescing rules. Done when property tests hold
  (undo of any op sequence restores the prior buffer byte-exactly).
- **T3. IME composition.** Preedit range, commit, cancel, over the host's
  IME events. Done when composition round-trips on the primary desktop
  (Windows) and degrades cleanly where events are absent.
- **T4. First consumer: cambium `text_input`.** The control rides the core;
  selection, IME, and undo work in a cambium view. This is the consumer that
  makes the toolkit credible.
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
