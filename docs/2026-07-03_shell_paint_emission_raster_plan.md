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

## P0 receipts (2026-07-04)

Spans landed host-side, all under `meerkat::profile` at debug level:

- `PaneSession::refresh` logs `rebuild_us` (stylist + full cascade + layout) or `apply_us` +
  the `Applied` kind.
- `serval_render` logs `emit_us` and `lower_us` per lane (`full` / `base` / `orrery`) with the
  command count, splitting `chrome_us` into scene production vs paint-list-to-Scene lowering.

First capture (restored session, `shell_node_count=108`, `gnode_count=9`, debug build) attributed
the rebuild frame's `chrome_us=129ms` as: rebuild 37ms, base emit 52ms, base lower 18ms, orrery
emit+lower 6ms. Base emitted only 117 commands, so the emit cost was per-face, not per-command:
`FontCollector::intern` copied every font file into every emitted list (`to_vec` per emit), minted
keys 0,1,2 per list so the same key carried different faces across lists (the image-key poisoning,
font twin), and `register_fonts` rewrapped the bytes in a fresh `peniko::Blob` per translate,
minting a fresh vello dedup id per frame so no downstream font/glyph cache could ever hit.

Fix, landed across three seams:

- `paint_list_api::FontResource.data` is now `Arc<Vec<u8>>` (serde `rc`, same wire shape).
- `serval-layout` holds a process-global font registry keyed by content identity (length + TTC
  index + three-window fxhash; a full-file hash measured ~0.6s per rebuild frame in debug and blob
  ids do not survive session rebuilds, so content keying is load-bearing). Each face is copied out
  of parley once per process; keys come from a global counter, stable for the process lifetime.
- `paint_list_render::register_fonts` caches the `peniko::Blob` per byte identity (keyed on the
  producer's `Arc` pointer, entry pins the `Arc`; capped at 256 entries), so vello sees a stable
  font id across frames. This is the contract netrender's own `FontBlob` docs already demanded.

Same-session receipts after the fix: base emit 52ms -> 4.0ms, base lower 18ms -> 0.12ms, orrery
emit (steady) 2.6ms -> 0.4ms, steady `chrome_us` 9.6ms -> 4.3ms, rebuild-frame `chrome_us`
129ms -> 46ms. Verified: serval-layout 233/233, paint_list_render 11/11, servo-paint
html-to-pixels e2e 30/30 (glyphs rasterize through the registry + blob cache), headed run clean.
`inker/document-canvas` still copies faces per emitter build (small lane, `.into()` shim only).

## Revised lane priorities (post-receipts)

- **P1 as originally worded (retained segment emission) is deprioritized**: steady-state emit is
  now sub-millisecond, so segment caching buys ~0.4ms. The design is worked out if it is ever
  needed (segments = direct children of a host-named segment container, cached in-flow spans +
  stacking layers re-sorted by `(z, chunk, local-seq)`, clean segments skipped via the existing
  `skipped_subtrees` walk; naming the orrery camera container as the container makes a camera pan
  dirty only the always-re-emitted residual wrappers). Do not build it before the two lanes below.
- **Emission lane, real remaining cost**: the session rebuild itself, 34ms per structural frame.
  `PaneSession` rebuilds the whole `IncrementalLayout` on ANY structural batch (a single text
  mutation included) because a `Spliced` apply invalidates the box-tree side-table and the session
  stops being emittable. The fix is inside serval-layout: keep `built`/`text_ctx` valid through a
  text-only / subtree splice so the session survives and `apply_us`-scale costs replace
  `rebuild_us`-scale ones. Second order: steady `apply_us` ~2.7ms (restyle) now dominates the
  cheap path.
- **Raster lane is now the top per-frame term** and P3 proceeds as written: steady orrery-only
  frames pay `chrome_raster_us` ~23ms (vs ~4ms of emission-lane work), rebuild frames 76-135ms.
- **P4 pan candidate (from review, 2026-07-04)**: during a camera pan every gnode legitimately
  moves, so even perfect delta-scaled emission re-emits the whole orrery subtree per frame (and the
  raster side repaints every tile). If pan is still over budget after the lanes above, composite
  the cached orrery texture at a camera offset during the gesture and re-render on settle. Hold
  until the measured lanes land.

## P3/P4 receipts and fix (2026-07-04)

P3 spans landed with zero new netrender instrumentation needed: `RenderCore::rasterize*` now logs
`Renderer::last_frame_timings()` per render call under `serval_winit_host::raster` (debug), and
netrender's existing per-render `elapsed_us`/`op_count` line decomposes `chrome_raster_us` per
surface.

The dominant raster cost was neither compose nor vello: **one shared tile cache served every
surface**. Tile invalidation diffs a scene against the PREVIOUS scene, and meerkat rasterizes
several surfaces (chrome base, chrome orrery, orrery canvas, workbench, each card) through one
renderer, so every render diffed against a DIFFERENT surface's scene: 234/234 tiles rebuilt per
settled frame on the big surfaces, and a small card render spent 90-150ms in invalidate+rebuild
against foreign tile state.

Fix (P4, landed): per-surface tile state. netrender `Renderer` holds an LRU-capped map of
`SurfaceTileState` (own `TileCache` + own per-tile scene store) keyed by a host-chosen surface id;
`render_vello_scaled_for(surface, ...)` renders against it (the rasterizer's tile-scene store
became a parameter; unkeyed entries keep the legacy shared pair). `serval-winit-host` exposes
`rasterize_for` / `rasterize_scaled_for`; meerkat keys every rasterize site
(`render/surface_keys.rs`: fixed ids for shell surfaces, hashed ids per content card).

Receipts (same restored session, debug): steady orrery-canvas render 10.7ms -> 7.2ms with
`dirty_tiles` 234 -> 0 (rebuild 2.5ms -> 0.09ms); workbench likewise; the structural interaction
frame's `chrome_raster_us` 68ms -> 45ms (base rebuilds 71 tiles, not 608). Verified: netrender
40/40, pixels e2e 30/30, meerkat bin 216 passing (same three pre-existing failures).

**Remaining raster whale, precisely named**: the focused/live card surface (378x1012) re-rasters
on settled frames at ~92-260ms apiece, all 24 of its own tiles dirty every time, invalidate alone
40-66ms — its scene genuinely differs (or grows) every emit, and its op density makes
`TileCache::invalidate` expensive (invalidate is O(ops x tiles)). Its FIRST render costs 7ms with
0.3ms invalidate, so this is content churn, not cache overhead. That is the focused-card audit
this plan already scoped out as separate meerkat-side work: find why a settled session keeps
re-emitting a changing card scene (`scene_version` churn), then (netrender-side, if still needed)
bound invalidate for op-dense scenes.

## Focused-card churn: identified (2026-07-05)

The settled-session card re-raster source is pinned. The focused card is the Wikipedia page
(HTML/serval scripted lane). Instrumented receipts: the host's scroll-band dedupe HOLDS (exactly
two band re-commands per run, both initial), yet the content actor ships a fresh band scene every
~7s cycle unprompted — `scene_version` climbs 1 per delivery forever, each costing a 90-260ms
re-raster of the op-dense 378x1012 band. Mark's read matches: the page visibly reformats (basic
layout, then the table of contents lands) — script/hydration reflow plus element loading. The
problem is it never converges into silence.

Two candidate fixes, in order:
1. Actor-side identical-scene suppression at the `emit_scene` seam (`content/handlers.rs`, five
   call sites, state on `Content`): fingerprint the outgoing band (the transfer path already
   serializes every update — reuse that encoding for a sound fingerprint, not a weak op-count
   hash) and skip the emit when it equals the last shipped one. No version bump, no host wake, no
   raster. Catches reflow-that-converges. Harmless to real changes.
2. If the page genuinely mutates forever (animated/scripted content), a policy lever: preview
   cards should not pump page scripts at full cadence (pause or slow scripts for unfocused
   cards). Product decision, Mark's call.

Next probe if 1 does not silence it: log actor-side WHAT triggered each re-render (script timer
vs hydration pass vs host command) in the scripted-lane pump.

Also noted: the sibling card (`c9a4...`) loops a cheap no-op doc-lane re-attempt every frame
(`version=0`, no packet, network actors disabled) — harmless cost, but the doc lane could cache
the "no packet" outcome instead of re-deriving per frame.

## Progress

- **2026-07-03**: plan written after the post-fix shell-partition runtime pass closed the biggest
  host-side confounders. Scoped against the live `serval-layout` seam
  (`emit_paint_list_excluding_subtrees`, `emit_subtree_paint_list`) and netrender's existing frame
  timing surface (`tile_invalidate`, `dirty_tile_rebuild`, `master_compose`, `vello_render`).
- **2026-07-04**: P0 spans landed (meerkat side); receipts captured pre/post; per-frame font-bytes
  copy + unstable font keys found and fixed across paint_list_api / serval-layout /
  paint_list_render (details above); lane priorities revised from the receipts. Harness note: the
  `MEERKAT_CAPTURE_DIR` chrome self-capture only fires on the unpartitioned chrome texture, so it
  is dead in partitioned mode; repair when next needed headed.
- **2026-07-04 (splice survival)**: the emission lane's remaining whale is closed. The structural
  splice now grafts its scoped box tree + shaped text into the retained side-table
  (`BoxTree::graft_subtree`: arena-append at `base` with child/`node_map`/text-key remap, old
  subtree purged to orphans, root location pinned; `TextMeasureCtx::{purge_keys,absorb_remapped}`;
  `ImagePlane::merge_from` for spliced-in `<img>`), so `IncrementalLayout` stays emittable through
  every apply path and `PaneSession` no longer rebuilds the session on structural batches (rebuild
  triggers left: first build, resize, theme; plus a `paint_ready` heal-by-rebuild belt-and-braces
  that never fired in captures). Supporting changes: `CharacterDataChanged` invalidation roots
  lift to the nearest element ancestor (they rooted at the TEXT node, which owns no fragment, so
  every text edit full-relaid-out); the scoped pass lays out at the root's prior border-box size
  instead of the viewport (non-full-width subtrees used to trip the outer-size guard by
  construction); splice fallbacks log their reason under `serval_layout::splice`. Tried and
  REVERTED: ancestor-escalation retries on splice bail (a ladder of scoped layouts up a
  shrink-to-fit chain measured 90ms where the direct full-relayout fallback cost 20ms).
  Receipts (same restored session, debug): the interaction structural frame went
  `rebuild_us=34.8ms` + cold emits (`chrome_us` 46ms; 129ms pre-font-fix) to
  `apply_us=19.6ms` (`chrome_us` 24.3ms) — the remaining cost is the in-session full relayout the
  batch legitimately needs (its dirty root is shrink-to-fit, `dw=-9`, so the size guard is right
  to fall back; fixed-size roots splice at low ms). Steady frames 4.2-4.6ms, settled ~0.5ms, zero
  heals. Verified: serval-layout 236/236 (three new splice-parity tests: retained emit ==
  fresh-session emit command-for-command after text / insert / removal splices, hit-test through
  the graft), pixels e2e 30/30, meerkat bin 216 passing with only the three pre-existing
  unrelated failures. Next per revised lanes: the raster term (`chrome_raster_us` ~22ms steady,
  ~68ms on the structural frame) is now the dominant per-frame cost — P3 characterization.
- **2026-07-04 (raster lane)**: P3 receipts + P4 fix landed — per-surface tile caches (see the
  "P3/P4 receipts and fix" section above). Retained shell surfaces now converge to
  `dirty_tiles=0`; the one remaining raster whale is the focused/live card's per-frame scene
  churn (meerkat-side audit, next).
