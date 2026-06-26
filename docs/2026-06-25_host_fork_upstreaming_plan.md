# Upstreaming host (mere/meerkat) forks into serval

**Date**: 2026-06-25
**Status**: P1 (keystone) executing 2026-06-25. P2-P5 open.
**Source**: a 4-agent "host-fork-outpaces-engine" sweep over mere ↔ serval (the inverse of the mere
capability-misuse sweep). The mere host is serval's de-facto integration test: where it forked a serval
capability, it found a private/missing primitive or a correctness gap and fixed it in place. Upstreaming
pays triple — it deletes the host's re-rolls, deletes serval's *own* internal duplications, and hands
pelt-live + serval-rendered web content the same correctness.

Host-side adoption (meerkat dropping its copies once these land) is tracked in the mere plans:
`mere/design_docs/mere_docs/implementation_strategy/2026-06-25_{host_scroll_engine_adoption,overlay_primitive_adoption,xilem_serval_control_adoption}_plan.md`.

## The keystone (P1): publish the geometry serval already computes

The dominant finding: serval computes a family of box geometry correctly but keeps it private, so the
absolute-origin chain-sum is hand-rolled at 7+ host sites **and re-rolled 3-4× inside serval itself**, and
serval-render's own `push_scrollbars` is buggy precisely because it skipped it.

- `absolute_origin` (`serval-layout/serval_lane.rs:538`) is written + correct but `pub(crate)` (used by
  `scroll_to_element` / `scroll_element_into_view`). The only public `FragmentPlane` read is `rect_of`
  (parent-relative). **Make `absolute_origin` public; add `IncrementalLayout::absolute_rect(dom, node) ->
  Option<(x,y,w,h)>`.** meerkat holds the `IncrementalLayout` session and depends on serval-layout
  directly (not on `engine_observables_api::ServalLaneView`), so put it on `IncrementalLayout`/
  `FragmentPlane`, not only on the `FragmentQuery::box_model` trait path.
- `scroll_extent` (`serval-layout/incremental.rs:570`) is private and **more** capable than the host's
  `max_scroll` (both axes + absolute-overflow union via `content_far_edge`). **Make it `pub`.** The
  host's two `max_scroll` copies (`roster_view.rs`, `list_pane.rs`) then gain x-axis + abs-overflow for
  free; `push_scrollbars` stops inlining the inset formula.
- **Fix `serval-render/render.rs::push_scrollbars`** to place the thumb at the absolute origin (the mere
  copy's fix; the engine's own docstring defers it as "nested scrollers would need origin accumulation").
  This lands on top of the keystone.

Retires ~8 host re-rolls + 3-4 serval-internal copies; additive (no behavior change beyond the scrollbar
fix); widest blast radius for the smallest change.

## P2: promote the origin-accumulation maps

- Promote the host's whole-tree `accumulate_origins(dom, fragments, root, &mut map)` (one O(n) pass) into
  serval-layout public, and make the single-target `absolute_origin` a thin wrapper. Delete the three
  private copies (`caret.rs`, `serval_lane.rs`, `viewport.rs::extend_scrollable`) and serval-render
  `a11y::build`'s 4th copy.
- Add `accumulate_painted_origins(.. , scroll)` — the **scroll-aware** origin map (subtracts each scroll
  container's offset from descendants). serval has **no public answer** today to "where does this paint
  after ancestors' scroll", which overlays / selection handles / IME anchoring all need.

## P3: serval-render paint-glue gaps the host filled

- **Focus ring**: serval-render paints caret + selection + scrollbars but no `:focus` outline. Port the
  host's `push_focus_ring` + the painted-origin walk; the transform primitive (`accumulated_translate`,
  `incremental.rs:193`) and caret-color already exist, so it is glue, not new logic. pelt-live + web
  content get `:focus-visible`.
- **`caret-color: auto`**: `push_caret` hardcodes `CARET_COLOR` (invisible on a dark field). serval-layout
  already ships `caret_color` (`incremental.rs:902`); wire it, keep the const as the resolve-miss fallback.
- **`TextCursor.editable`**: add the bool so a focused *button* rings without a spurious caret (gate
  caret/selection on it; the focus ring stays unconditional).

## P4: accessibility-correct clickable default (highest user impact)

serval's bare `button`/`checkbox` ship keyboard-unreachable — their own `focusable.rs` docstring names the
gap. meerkat re-expresses `focusable(on_click(el(..)))` 7× to fix it. **Add `clickable(child, handler)`**
(= `focusable(on_click(..))`), or fold `focusable` into `button`/`checkbox` by default. Then every serval
consumer's controls are Tab-reachable + screen-reader-activatable by default, not by per-caller discipline.

## P5: view/DOM ergonomics

- `LayoutDom` class/tag queries (`first_with_class`/`all_with_class`/`has_class`/`first_tag`) re-rolled in
  the host **and ~6 places inside serval** → provided default-impl trait methods on `LayoutDom`.
- **Sized overlay** (`positioned_box`/`overlay_rect` carrying width/height): `overlay_at` is point-only, so
  the host hand-rolls sized `position:absolute` 7× (comms pane, shellbar, window panes). (= overlay plan P3.)
- **Flip/clamp-aware `anchor_point`**: `card.rs::anchored_card_rect` and the new submenu both hand-roll
  flip-to-opposite-side + viewport-clamp; extend `anchor_point` to take the available bounds and do it.
- `text_field_lens` / `button_classed` boilerplate (marginal).

## Not a host win (bounds the work)

serval's hit-testing (`serval_lane.rs::walk_for_hit`) is already fully clip/scroll/transform/fixed-aware;
meerkat delegates to it. Baseline chrome a11y bounds are identical to serval's (neither folds transform;
only the orrery-specific host path does). So the host is not ahead on hit-testing or baseline a11y geometry.

## Progress

- 2026-06-25: Drafted from the host-fork sweep.
- 2026-06-25: **P1 (keystone) landed.** Published `absolute_origin` (`serval_lane.rs`, re-exported from
  `lib.rs`); added `IncrementalLayout::absolute_rect(dom, node) -> Option<(x,y,w,h)>`; published
  `scroll_extent`. Fixed `serval-render::push_scrollbars` to place the thumb at the container's absolute
  origin (the mere copy's fix), so a nested scroller's bar lands on its real edge (top-level scrollers
  unchanged — their absolute origin equals the raw location). `cargo check -p serval-layout -p
  serval-render` clean (the lone `private_interfaces` warning is pre-existing `PseudoKind`); 206
  serval-layout + 25 serval-render lib tests green. meerkat adoption (drop its `accumulate_origins` /
  `max_scroll` copies for these accessors) is the mere scroll-plan's P1/P4.
