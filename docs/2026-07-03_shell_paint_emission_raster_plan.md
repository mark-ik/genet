# Shell Paint Emission And Raster Plan

**Date**: 2026-07-03  
**Status**: planning, scoped from the post-fix shell-partition runtime receipts.  
**Related**: [2026-07-03_shell_partition_runtime_findings.md](./2026-07-03_shell_partition_runtime_findings.md), [2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md), [2026-05-20_paintlist_extraction_plan.md](./2026-05-20_paintlist_extraction_plan.md), [2026-07-01_ui_polish_plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-07-01_ui_polish_plan.md) finding 5.

The Meerkat-side confounders are now stripped away far enough to justify a real Serval follow-on.
In the post-fix restored-session capture (`shell_node_count=141-142`, `gnode_count=14`), a single
shell text mutation still produced `chrome_us=128987` and `chrome_raster_us=60520`. That frame is
the new motivator. It is small-dirty and loaded enough to be worth optimizing, without the old
per-frame host churn polluting the result.

This plan splits the remaining work into the two terms the runtime findings now separate:

1. `chrome_us`: the Serval-side scene-production lane, with `serval-layout` paint emission as the
   main suspected cost center.
2. `chrome_raster_us`: the netrender raster lane, where tile invalidation, dirty-tile rebuild, and
   master composition decide what the GPU-side frame actually costs.

## What is already true

- The shell partition seam is already real. `IncrementalLayout` exposes
  `emit_paint_list_excluding_subtrees` and `emit_subtree_paint_list`, and `paint_emit.rs` has the
  matching free functions plus tests.
- The follow-on should not pretend its first task is inventing partitioning. That part is already
  live and already consumed by Meerkat.
- Netrender already exposes timing spans through `Renderer::last_frame_timings()`:
  `tile_invalidate`, `dirty_tile_rebuild`, `master_compose`, and `vello_render`.
- The remaining Meerkat-side focused-card structural bursts (`mut_count=8` and `mut_count=3`) are
  separate audit work. They are not blockers for this plan.

## Boundaries

- Do not "fix" this by suppressing same-value DOM writes inside `ScriptedDom`. For scripted
  documents that would be standards-wrong. Host seams should avoid wasteful writes before they hit
  the engine.
- Do not move invalidation ownership onto `LayoutDomMut` or script-engine traits. Keep it in
  `serval-layout` and the renderer/scheduler layers.
- Do not fold the raster half into vague "layout is slow" prose. The runtime receipts already show
  that `chrome_us` and `chrome_raster_us` must be discussed separately.

## P0: Loaded-session attribution receipts

Before changing the hot path, make the loaded-session evidence directly actionable from the log.

- Thread a finer split through the shell profile path so one loaded-session frame reports:
  - mutation batch summary,
  - layout/paint-session state (`rebuild`, `structural`, `base_dirty`, `orrery_dirty`),
  - Serval-side scene-production spans, especially paint emission,
  - netrender frame timings (`tile_invalidate`, `dirty_tile_rebuild`, `master_compose`,
    `vello_render`),
  - dirty-tile count.
- Keep the session identity, `shell_node_count`, and `gnode_count` in the same receipt.
- Reuse the loaded restored-session harness from the runtime findings doc rather than switching back
  to near-empty graphs.

Done when one headed loaded-session capture can point to a single tiny-dirty frame and say exactly
how much time was spent in scene production versus raster.

## P1: Retained paint emission inside `serval-layout`

The current subtree/base split still pays for a full base-shell paint walk whenever the base is
legitimately dirty. That is the first Serval-side cost shape to attack.

- Add explicit paint-emission timing around the retained `IncrementalLayout` emit path so the plan
  stops inferring from the larger `chrome_us` bucket.
- Retain paint output below the document root in cacheable segments keyed to stable box-tree
  subtrees or another equally local unit already owned by `IncrementalLayout`.
- Attribute-only and text-only dirtiness should invalidate only the affected segment set. A tiny
  shell mutation should not force a complete re-walk and re-encode of unrelated base-shell
  children.
- Structural splices may still fall back more broadly, but only where the side-table or segment
  boundaries are actually invalidated. Keep the current correctness rule that stale paint-side
  tables require relayout first.
- Preserve the existing shell-partition API shape. Meerkat should keep calling the same coarse
  base/subtree seam while Serval gets cheaper under it.

Done when instrumentation shows that a `mut_count=1` text-only shell frame reuses unaffected base
segments, and the number of walked/emitted nodes tracks the dirty surface rather than total shell
document size.

## P2: Correctness net for retained emission

The new cache cannot become another silent side-table that only works on the happy path.

- Add focused tests in `serval-layout` for:
  - attribute-only mutation reusing unaffected paint segments,
  - text-only mutation localizing invalidation,
  - structural splice invalidating the right retained paint state,
  - excluded-subtree emit and subtree-local emit remaining behavior-identical to today's output.
- Keep the current "relayout first after structural splice" assertions unless the retained state is
  proven valid through that splice.

Done when the retained-emission cache has direct tests for attribute, text, and structural cases,
not just a headed browser receipt.

## P3: Raster-side characterization in netrender

Once the scene-production lane is measured cleanly, pin the raster half on actual spans instead of
carrying it as a generic tail cost.

- Feed the same loaded-session frame into `Renderer::last_frame_timings()` and `vello_last_dirty_count()`.
- Distinguish these cases:
  - dirty-tile count stays small, but `master_compose` or `vello_render` remains large,
  - dirty-tile count explodes from a tiny shell mutation,
  - ordered external-texture tail redraw is dominating because the session has mid-scene external
    boundaries.
- Record which of `tile_invalidate`, `dirty_tile_rebuild`, `master_compose`, or `vello_render`
  actually dominates before proposing a fix.

Done when the review doc can name the dominant raster span for the loaded-session tiny-dirty frame,
with dirty-tile count beside it.

## P4: Raster fix lane

The raster fix should be chosen from the receipts, not guessed in advance, but the candidate lanes
are already narrow enough to name.

- If dirty-tile count is too broad for tiny shell edits, tighten the scene bounds feeding tile
  invalidation so small text/style changes stop splashing across large regions.
- If `master_compose` dominates despite a small dirty set, avoid rebuilding or reblending more of
  the master scene than the dirty tiles actually require.
- If ordered external-texture composition dominates, keep the topmost fast path for cases that do
  not need mid-scene boundaries and reduce avoidable full-viewport tail redraw.

Done when the chosen raster fix is justified by one dominant measured span and verified against the
same loaded-session harness.

## Review questions

1. Is this still one plan?
   Yes, but only because P0 is shared. After that, the work naturally splits into a `serval-layout`
   emission lane and a netrender raster lane.
2. Is the Meerkat host still the likely source of the loaded-session tiny-dirty frame cost?
   Not as the primary explanation. The host-side steady churn and shellbar structural confounders
   are already stripped away in the runtime findings.
3. Should this plan absorb the remaining focused-card structural batches?
   No. Those are useful audit work, but they are separate from the now-clean shell base tiny-dirty
   motivator.

## Progress

- **2026-07-03**: plan written after the post-fix shell-partition runtime pass closed the biggest
  host-side confounders. Scoped against the live `serval-layout` seam
  (`emit_paint_list_excluding_subtrees`, `emit_subtree_paint_list`) and netrender's existing frame
  timing surface (`tile_invalidate`, `dirty_tile_rebuild`, `master_compose`, `vello_render`).
