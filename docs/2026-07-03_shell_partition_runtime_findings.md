# Shell Partition Runtime Findings

**Date**: 2026-07-03  
**Scope**: runtime review of the Meerkat host-side shell partition work that consumes the new `serval-layout` subtree/base paint-emission seam.

## Setup

- Fresh binary: `C:\t\meerkat-target\debug\meerkat.exe`
- Launch env:
  - `RUST_LOG=meerkat=info,meerkat::profile=trace`
  - `MEERKAT_DISABLE_NETWORK_ACTORS=1`
- Interaction:
  - let the restored session settle
  - click a focused-orrery node
  - drag-pan the orrery
- Evidence source:
  - `C:\t\meerkat-profile-stdout.log`

## What landed

The host-side split is real and runs end to end:

- Meerkat now builds a chrome base scene excluding the `.orrery` subtree.
- Meerkat now builds a separate `.orrery` DOM subtree scene.
- The frame paint path composes the cached base texture, then the cached `.orrery` DOM texture, over the existing external-texture underlay.

This is not a compile-only claim. The headed pass used the fresh binary above.

## Findings

### 1. The steady base-dirty loop was host overlay churn, and the fix is confirmed

The first partition pass never hit `orrery_only=true` on settled frames. The repeating line was:

- `rebuild=false`
- `structural=false`
- `mut_count=1`
- `base_dirty=true`
- `orrery_dirty=false`

with representative costs:

- `chrome_us=38056`, `chrome_raster_us=51833`
- `chrome_us=41170`, `chrome_raster_us=45689`
- `chrome_us=45062`, `chrome_raster_us=47154`

That mutation is now explained and fixed. `position_chrome_overlays` in Meerkat was re-stamping the
shellbar `style` attribute every frame, and `ScriptedDom::set_attribute` in Serval correctly
records same-value writes as `DomMutation::AttributeChanged`. The host was wasteful; the engine was
standards-correct.

After adding host-side unchanged-write suppression at the overlays seam and rerunning the headed
pass on a restored session (`gnode_count=14`, `shell_node_count=141-142`), settled frames reached:

- `rebuild=false`
- `structural=false`
- `mut_count=0`
- `base_dirty=false`
- `orrery_dirty=false`

with representative settled costs:

- `chrome_us=422`, `chrome_raster_us=4`
- `chrome_us=501`, `chrome_raster_us=3`

So the base-shell cache can now actually stay cold.

### 2. The partition seam now proves both cache states

The rerun also produced non-structural `.orrery`-only work:

- `rebuild=false`
- `structural=false`
- `mut_count=15`
- `base_dirty=false`
- `orrery_dirty=true`
- `orrery_only=true`

Representative cost:

- `chrome_us=29883`
- `chrome_raster_us=41415`

That matters because it proves the split is not just compile-real. The runtime can now distinguish:

- fully settled frames where both shell halves stay cold
- orrery-only frames where the base stays reused and only the focused subtree repaints

### 3. Structural shell batches still dominate the remaining cost

The first click/drag rerun still produced expensive structural bursts:

- `mut_count=3`, `rebuild=true`, `structural=true`, `orrery_only=true`, `chrome_us=53459`, `chrome_raster_us=42146`
- `mut_count=8`, `rebuild=true`, `structural=true`, `orrery_only=true`, `chrome_us=59256`, `chrome_raster_us=43615`
- `mut_count=21`, `rebuild=true`, `structural=true`, `base_dirty=true`, `chrome_us=137475`, `chrome_raster_us=94301`

Each burst later returned to the settled `mut_count=0` / `base_dirty=false` state, so the
remaining problem is not the old per-frame shell churn. It is the cost of the structural batches
that still occur during interaction and shell updates.

The first attribution pass now narrows those bursts further:

- The `mut_count=3` batch was mostly focused-orrery/card churn: two attribute changes plus one
  removal under `div.orrery`.
- The `mut_count=8` batch was focused-card construction: three inserts and five attribute changes,
  including insertion of `div.unvisited-card`.
- The `mut_count=21` batch was **not** a mysterious full-shell rewrite. It was one text-node
  insert and one remove under the shellbar input, plus nineteen attribute changes, including
  `div.shellbar` style and multiple `div.gnode-root[data-member=...]` style writes.

That matters because the expensive path is currently triggered by small structural edits mixed into
otherwise ordinary shell/orrery updates. The next audit should decide whether that input-child swap
can be eliminated or isolated from the retained shell session.

The follow-up rerun after the omnibar seam change answers that question:

- Meerkat now uses the plain single-line field for the omnibar instead of the styled child-emitting
  field path.
- The old `-> input` / `<- input` structural samples disappeared.
- The largest interaction burst dropped from `mut_count=21` to `mut_count=20`.
- That batch is now one same-node text mutation plus nineteen attribute changes:
  `text #text(NodeId(...)) | attr style @ div.shellbar | attr style @ div.gnode-root[...] ...`
- The frame is still expensive (`chrome_us=155892`, `chrome_raster_us=94140`), but the retained
  shell session is no longer being dirtied by input-child insertion/removal.

So the shellbar input structural seam is closed. What remains is ordinary shell text/style churn
plus the focused-card structural batches (`mut_count=3` and `8`), alongside the still-large raster
term.

One more host-side confounder fell immediately after that. The next rerun showed that the
`mut_count=20` shellbar batch was not intrinsic shellbar work either: the active session chip was
wrapping to two lines, which raised the measured toolbar height from `92px` to `122px` and forced a
matching shellbar geometry rewrite. Clamping the session-chip labels to one line removed that jump.
On the next headed pass:

- there was no follow-on `toolbar height changed` event after startup,
- there was no follow-on `overlay geometry style changed class_name=\"shellbar\"` event during the
  interaction,
- the old `mut_count=20` shellbar batch collapsed to `mut_count=1`,
- that remaining batch was just `text #text(NodeId(...))`.

That leaves a much cleaner Serval-side motivator: a single shell text mutation in the loaded
session still produced `chrome_us=128987` and `chrome_raster_us=60520`.

## Interpretation

The diagnosis is now tighter:

- Before the gnode work, focused-pane churn was a plausible primary suspect.
- After the gnode pool work, that suspicion moved to a non-orrery shell mutation.
- After the overlay suppression rerun, that steady-state mutation is gone and the split reaches the intended cache states.

So the partition seam is validated. The remaining Serval-side work is about the cost of the frames
that are still legitimately dirty: base-scene emission, base-scene raster, and the structural shell
batches that trigger both.

## Review Questions

1. Is the steady `mut_count=1` / `base_dirty=true` loop still part of the Serval problem statement?
   No. It was host-side overlay churn, and the rerun confirms the fix.
2. What should motivate the Serval-side plan now?
   A like-for-like loaded-session capture taken after this fix, with `shell_node_count` and session
   identity recorded alongside the frame numbers. The earlier near-empty gnode-plan captures are
   not enough, and this restored session (`shell_node_count=141-142`) is still not the same
   measurement as ui_polish finding 5's loaded roster/gloss case.
3. Should emission and raster stay coupled in the next Serval-side write-up?
   No. This doc now shows both terms explicitly, and the next plan should keep them separate:
   `chrome_us` is the emission-side term, `chrome_raster_us` is the raster-side term.
4. Is there still a Meerkat-side follow-up worth doing?
   Yes, but it is audit work rather than gnode-plan debt: inspect the remaining focused-card
   structural batches (`mut_count=8` and `3`). The former shellbar text/style batch is gone; what
   remains on the base side is a pure text mutation.

## Baseline caveat (updated 2026-07-03)

The post-fix rerun closed the biggest ambiguity: the frightening 38-45ms "settled" frames were not
evidence that the split itself was intrinsically expensive. They were polluted by the host's
per-frame overlay mutation.

One comparison gap still matters, though. The earlier gnode-plan P5 capture that quoted `chrome_us`
~7.3-9.1ms came from a much smaller shell document (`shell_node_count=63`, `gnode_count=1`), while
this rerun used a restored session at `shell_node_count=141-142`, `gnode_count=14`. And ui_polish
finding 5's 100-145ms motivating number came from a loaded roster/gloss/full-orrery session that
was larger again. So this doc now proves the seam and clears the host-churn confounder, but it does
not yet replace the loaded-session Serval-side motivating measurement.

## Recommended next step (revised 2026-07-03)

1. Capture a truly loaded session after this fix, with `shell_node_count`, `gnode_count`, and
   session identity recorded alongside the frame numbers.
2. Scope the Serval-side follow-on with emission and raster named separately from the start.
3. Audit the remaining focused-card structural bursts the same way the overlay churn was audited;
   the loaded-session shell base now already has a clean tiny-dirty measurement (`mut_count=1`
   text-only) suitable for the Serval-side plan.
4. Batch-level mutation instrumentation still fits doctrine §6.3, but it is follow-up work, not
   the gate.

As of the post-fix follow-up, that audit seam is now partly in place on the Meerkat side:
`PaneSession::refresh` logs structural-batch kind counts (`inserted`, `removed`, `attr_changed`,
`text_changed`, `subtree_replaced`) plus a short sample trail of the affected nodes/parents. The
next headed capture should use that output to pin the remaining `mut_count=8` / `3` bursts on
concrete surfaces before changing Serval.

Cross-reference: the mere-side history of this work lives in the
[gnode_pool_plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-07-02_gnode_pool_plan.md)
(P0-P5 + the parked-cull follow-on + the partition pass); this doc is the serval-side receipt for
the seam's first consumer. The Serval-side follow-on now lives in
[2026-07-03_shell_paint_emission_raster_plan.md](./2026-07-03_shell_paint_emission_raster_plan.md).
Keep conclusions synced.
