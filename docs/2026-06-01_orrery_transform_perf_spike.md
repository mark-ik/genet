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

## Three serval prerequisites for the orrery's *continuous* motion (all resolved)

The gate above is necessary, not sufficient. The spike surfaced three gaps that
blocked the orrery's actual mechanism (mutate each node's transform every frame).
All three are now fixed; the tripwire tests were flipped to assert the corrected
behaviour and pinned as regression guards.

- **(A) Incremental restyle ignored inline-`style` changes.** Setting
  `style="transform:…"` registered no paint-tier damage on the incremental path:
  `snapshot.rs` marks a `style`-attribute change `other_attributes_changed`, which
  only drives `[attr]`-selector invalidation, and serval emitted no hint to
  re-apply the inline declaration block. **Fix** (`cascade.rs`,
  `restyle_with_snapshots`): on a `style`-attribute mutation, force a full
  re-cascade of the element's subtree (`RestyleHint::restyle_subtree`). The
  inline-style pass re-parses the (mutated) `style` attribute every cascade, so the
  re-cascade re-reads it and re-applies it. Tests
  `inline_style_transform_restyles_repaint_only` (value to value, RepaintOnly +
  `RECALCULATE_OVERFLOW`) and `inline_transform_first_application_relayouts_then_repaints`
  (the materialization rule: none to value relayouts once, then value to value is
  RepaintOnly).
- **(B) A second sequential `RepaintOnly` `apply()` dropped the change.** The first
  transform change registered correctly, but a subsequent one produced no
  paint-tier damage: stylo's `handled_snapshot` bit (per-traversal state) is
  persisted on the entry across `apply()` calls, so a stale `true` made the
  invalidator skip the next pass's snapshot. **Fix**: reset `handled_snapshot` to
  `false` for each attribute-changed element at the start of every incremental
  pass. Test `sequential_repaint_only_applies_each_re_register` (t1 to t2 to t3,
  each re-registers).
- **(C) Paint did not apply the CSS transform to painted position.** `paint_emit`
  used the taffy `Layout.location` (box-model position) and emitted an identity
  transform; the computed `transform`/`translate` was never folded into the
  `PushTransform`. **Fix**: `compute_transform_matrix(styles, id)` reads the
  element's computed `box` style, builds the `translate` matrix and the `transform`
  matrix (`to_transform_3d_matrix`), composes them, and the in-flow `PushTransform`
  carries the result. Test `css_transform_folds_into_pushtransform` (a
  `transform:translate(40,40)` yields a `PushTransform` whose `m41`/`m42` are ~40;
  without it, identity).

### Memory-safety footgun found + fixed during (A)

The first cut of (A) used stylo's narrower `RESTYLE_STYLE_ATTRIBUTE` replacement
hint (cheaper: it replaces only the style-attribute level on the element's existing
rule node rather than re-matching selectors). That **corrupted the heap under
parallel test execution** (intermittent `STATUS_ACCESS_VIOLATION` /
`STATUS_HEAP_CORRUPTION`, ~1/3 of full-suite runs; clean single-threaded and clean
when the inline-style tests were skipped). Root cause: `RESTYLE_STYLE_ATTRIBUTE`
drives stylo's `CascadeWithReplacements`, which reuses the rule node stored on the
prior pass's `ElementData` and operates it against `context.stylist.rule_tree()`.
But `cascade_traverse` builds a **fresh `Stylist` (hence a fresh rule tree) every
pass**, so the reused node dangles into the previous pass's already-dropped rule
tree. That is benign single-threaded (the freed allocation is usually still
intact), but a use-after-free that another thread's allocator can reuse under
parallel runs. **Resolution**: take the full re-cascade path instead
(`restyle_subtree`), which builds fresh rule nodes in the current tree, identical to
what `restyle_structural` already does. Verified: 85/85 single-threaded and 10/10
parallel full-suite runs clean. Damage classification is unaffected, because
`RestyleDamage` is `compute_style_difference(old, new)` regardless of the
match-vs-replace path, so every (A) assertion holds.

### Persistent Stylist: the cheap replacement path, restored (done)

The `restyle_subtree` workaround was correct but re-matched the element's subtree
selectors every frame. The follow-up landed: `IncrementalLayout` now owns a
**persistent `Stylist`** (device + UA/author sheets + rule tree), built once in
`new()` and reused for every pass, mirroring the persistent `SharedRwLock` already
on `StylePlane`. With the rule tree kept alive across passes, the reused rule node
held on `ElementData` is valid, so the cheap `RESTYLE_STYLE_ATTRIBUTE` replacement
hint is sound again — `restyle_with_snapshots` emits it for inline-`style` changes
(set alone, so `restyle_kind` takes `CascadeWithReplacements` and skips selector
re-matching). Confirmed against pinned stylo: `rule_tree: RuleTree` is an owned
field of `Stylist` (so keeping the Stylist alive keeps every node valid), and
`update_rule_at_level` walks the prior node against `context.stylist.rule_tree()` —
the same tree, now that it persists.

Shape of the change:

- `build_stylist(viewport, sheets, base_url, lock)` builds + flushes a `Stylist`
  under the plane's stable lock (so sheets, inline blocks, and guards share one
  `SharedRwLock`). `cascade_traverse` takes `&Stylist` instead of building one;
  `run_cascade` (one-shot, oracle tests) hands it a throwaway, `IncrementalLayout`
  hands it the persistent one (`run_cascade_with_stylist` for the initial cascade,
  `&self.stylist` for each incremental pass).
- Each pass calls `stylist.rule_tree().maybe_gc()` (single-threaded, after the
  traversal) so the per-frame replacement nodes that land on the free list are
  reclaimed once past Stylo's GC interval (300).
- The session's stylesheets are **fixed at `new()`** — the persistent rule tree
  can't be safely rebuilt mid-session (old `ElementData` nodes would dangle, and
  *dropping* them hits the dead free list), so `apply()` debug-asserts the set is
  unchanged. Stylesheet hot-reload (rebuild + force a full re-match that frame,
  dropping old nodes while the old tree is still alive) is the remaining follow-up.

Verified: 86/86 single-threaded and parallel full-suite runs clean — the
parallel-only heap corruption stays gone with the cheap path restored. New
regression test `sustained_inline_transform_motion_stays_repaint_only` drives 400
per-frame inline-transform applies (crossing the GC interval), each `RepaintOnly` +
`RECALCULATE_OVERFLOW`, proving the replacement-path reuse + GC hold up over a long
session. Damage is unchanged from the `restyle_subtree` cut (same
`compute_style_difference`).

## Verdict

Relayout-classification gate: **favorable** (transform is paint-tier; the §8 fear
is retired). Orrery transform-driven motion: **A + B + C all resolved**, all
serval-side, verified single-threaded and parallel — and the inline-transform path
now runs on the **cheap replacement restyle** (no per-frame selector re-match) via
the persistent `Stylist`. The Mere flip plan's orrery phase (P1) is unblocked. The
tests are regression guards pinned to the current stylo rev (572ecba).
