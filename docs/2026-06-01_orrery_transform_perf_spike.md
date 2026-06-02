# Orrery transform-motion perf spike (serval-layout)

**Date**: 2026-06-01
**Context**: the perf spike gating Mere's serval-as-host orrery flip (mere
`design_docs/mere_docs/implementation_strategy/2026-06-01_serval_host_flip_plan.md`
P0 / `2026-05-29_serval_as_host_evaluation.md` §8). Cross-repo: requested by Mere,
measured here because the machinery is serval's.
**Where**: `components/serval-layout/incremental.rs` (`#[cfg(test)]`) +
`cascade.rs` (instrumentation).

---

## The question

The orrery moves nodes by their CSS transform. §8's worry: does transform-driven
node motion force a full relayout (a document engine feeling worse than widgets)
at orrery scale (hundreds–thousands of nodes, 60fps)? "Needs measurement, not
invention."

## Signal + instrumentation

`IncrementalLayout::apply()` returns `Applied`: `RepaintOnly` means the restyle
was paint-tier and **layout was skipped** (the prior `FragmentPlane` is reused — no
`lay_out` call), vs `Restyled`/`FullRecompute` (layout ran). Upstream gate:
`RestyleOutcome::needs_relayout = damage.contains(RestyleDamage::RELAYOUT)`.

Added (≈6 lines, additive): `RestyleOutcome.damage` carries the aggregate
`RestyleDamage` union (already computed), surfaced as `IncrementalLayout::last_damage()`.
This is the *positive* proof — a test can assert a transform change registered
`RECALCULATE_OVERFLOW`, ruling out a misleading `RepaintOnly` from a silent no-op.

## Result: the relayout worry is retired

`transform_change_is_repaint_only_not_relayout` (N ∈ {200, 1000}): a transform
value change → `Applied::RepaintOnly`, `last_damage()` contains
`RECALCULATE_OVERFLOW` but **not** `RELAYOUT`, and every node's box geometry is
unchanged. Control `width_change_relayouts_control`: a width change →
`Applied::Restyled` + `RELAYOUT` (the harness genuinely sees relayout, so the
`RepaintOnly` results are trustworthy).

Source-grounded (pinned stylo): both `transform` and `translate` declare
`servo_restyle_damage = "recalculate_overflow"` = `0b0111`, which does not contain
`RELAYOUT` = `0b1111`. serval gates relayout solely on `contains(RELAYOUT)`
(`cascade.rs`), so a transform change is paint-tier. **Transform motion does not
force reflow.** The central §8 fear is unfounded.

## But: three serval prerequisites for the orrery's *continuous* motion

The gate above is necessary, not sufficient. The spike surfaced three gaps that
block the orrery's actual mechanism (mutate each node's transform every frame):

- **(A) Incremental restyle ignores inline-`style` changes.** Test
  `inline_style_transform_is_ignored_by_incremental_restyle`: setting
  `style="transform:…"` registers no paint-tier damage. `snapshot.rs` marks a
  `style`-attribute change `other_attributes_changed`, which only drives
  `[attr]`-**selector** invalidation; inline-style re-cascade needs a
  `RESTYLE_STYLE_ATTRIBUTE` hint serval does not emit on the incremental path. The
  full `run_cascade` *does* apply inline style (dfe8702), so this is an
  incremental-path gap, not a parser gap. The orrery moves nodes via inline
  transform, so this must be wired.
- **(B) A second sequential `RepaintOnly` `apply()` drops the change.** Test
  `sequential_repaint_only_applies_drop_the_second_change`: the first transform
  change registers correctly, but a subsequent one produces no paint-tier damage.
  Repeated per-frame applies do not re-register — the `RepaintOnly` layout-skip
  appears to leave stylo's restyle state uncleared for the next pass. Continuous
  motion via repeated `apply()` (the orrery's pattern) does not work today.
- **(C) Paint does not apply the CSS transform to painted position.** `paint_emit`
  uses the taffy `Layout.location` (box-model position) and emits an identity
  transform; the computed `transform`/`translate` is not folded into a
  `PushTransform`. So even a correctly-cascaded transform would not visibly move
  the node. (Source-read; not exercised here — it lives in the paint path, not
  serval-layout.)

(A) and (B) are also tripwires: when serval fixes them, the assertions FLIP,
signalling the orrery's mechanism is unblocked.

## Verdict

Relayout-classification gate: **favorable** (transform is paint-tier; the §8 fear
is retired). Orrery transform-driven motion: **gated on A + B + C**, all serval-side.
These are recorded as prerequisites in the Mere flip plan's orrery phase (P1). The
tests are regression guards pinned to the current stylo rev.
