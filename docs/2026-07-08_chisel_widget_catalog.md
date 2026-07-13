# chisel widget catalog: coverage across the xilem-serval consumers

**Status (2026-07-08):** proposed catalog + build order. Companion to
[2026-07-07_chisel_widget_leaf_design.md](./2026-07-07_chisel_widget_leaf_design.md)
(the leaf contract, paint paths, retention gates). This doc maps what each
xilem-serval consumer (Mere/meerkat, Strophe, Woodshed, Isometry) needs, what
CSS/native views already cover, which widgets are chisel leaves, and what gets
built in what order.

Code samples are **illustrative** unless marked implementation-ready.

---

## The four-tier sorting rule

Every widget lands in exactly one tier, and the tier decides its cost:

| Tier | Mechanism | Use for | Cost |
| --- | --- | --- | --- |
| 1 | **CSS / native views** | boxes, text, state: layout (flex/grid), borders, gradients, shadows, hover/focus, scrolling, form controls, transitions, media queries, theming via custom properties | free (already shipped) |
| 2 | **Path-A leaf** | pure vector geometry driven by data: fills, strokes with cap/join/dash (landed 2026-07-08, netrender 1e59984c0), quads/cubics | cheap: tile-cached, portable, no GPU coupling |
| 3 | **Arrangement leaf** | leaf owns x/y/z of real DOM children | the **virtualization mechanism**: CSS cannot say "materialize rows 400..430 of 100k"; an arrangement leaf can |
| 4 | **Path-B leaf** | genuinely per-pixel / per-frame imperative content | one texture + GPU rasterize per leaf; reserve for canvases |

Tier-1 coverage note: the form-control catalog (button, checkbox, text fields,
select, radio, slider, textarea, IME, focus traversal) is native xilem-serval
today, and theming rides CSS custom properties, so the orrery NODE_SHEET colors
become CSS vars once and every representation inherits node identity for free
(the "representations carry node identity" rule).

## Two composition rules

1. **Text stays out of Path-A leaves.** `DrawText` needs shaped glyph runs and
   font-instance keys that layout owns; a leaf cannot mint them cheaply. So:
   geometry in the leaf, labels/ticks/values as DOM siblings (plain CSS or an
   arrangement leaf places them). This also yields better widgets: labels stay
   selectable and a11y-visible.
2. **Interaction lives in the view layer, not the leaf.** A knob is
   `on_pointer(knob_leaf)` where drag delta maps to value in the xilem-serval
   handler; the leaf only paints. `Leaf::event` remains for the rare
   truly-internal case. Leaves stay dumb, testable, and reusable.

## Per-consumer coverage

### Mere / meerkat (graph browser)

| Widget | Tier | Notes |
| --- | --- | --- |
| Node swatches | 2 | `Swatch` exists (first leaf); color from NODE_SHEET |
| Graph-glyph buttons | 1 + 2 | native `button` wrapping a tiny `GraphGlyph` leaf: precomputed layout, circles + edge paths at ~20px. Same leaf scales to link previews, breadcrumb thumbnails, hover cards |
| Cards (summonable focus/snapshot family) | 3 | card **frame** = arrangement leaf owning x/y/z (drag, snap, stacking via `paint_stacking`); card **content** = ordinary DOM; `overlay_at` / `anchor_point` already exist for positioning. Same primitive serves tear-out in the one-state-N-windows model |
| Orrery canvas | 4 | designated first Path-B consumer (retires a hardcoded `*_SCENE_KEY` branch) |
| Gloss minimap | 4 → 2? | Path-B port now; may drop to Path-A shapes if churn allows |
| Tab strips, tile handles (gs::TileTab) | 1 | plain CSS |
| Sync indicator | 2 | tiny state glyph driven by real sync state (no placebo) |

### Strophe + Woodshed (the shared audio family)

Incubate in Strophe per the pressure-vessel doctrine; promote stable leaves to
a shared crate. Woodshed's fretboard-family views (arpeggio / exercise /
progression boards, lens strips, pills) are the proof the family is shared.

| Widget | Tier | Notes |
| --- | --- | --- |
| Meters (peak/RMS/LUFS) | 2 | rects + gradient; per-frame update = `paint_dirty` on ~6 rects; four-gate retention makes it nearly free |
| Knobs | 2 | arc + needle + tick dashes; pointer-drag in the view layer; shared by strophe device panels and woodshed amp/pedal panels, themed per consumer |
| Envelope / automation editors | 2 + 3 | Path-A curve + arrangement-leaf handles (real DOM children: hit-test + focus free) |
| Waveform | 2 / 4 | thumbnail/overview = Path-A polyline (round caps); zoomed live view = Path-B |
| Piano roll / step grid / loop lanes | 3 over 2 | arrangement leaf over a Path-A grid background; notes as real children when they need selection/a11y, painted rects when dense decoration |
| Spectrum | 2 (4 at high bin counts) | 64 bars is fine as Path-A rects |
| Tuner | 2 | knob variant (needle) |
| Fretboard | 2 | one leaf (strings, frets, dot markers), three woodshed views' worth of data |
| Transport, faders | 1 | native controls, styled |

### Isometry (pixel VTT)

| Widget | Tier | Notes |
| --- | --- | --- |
| Board (map, fog, tokens) | 4 | a game renderer; owns its scene |
| Measurement templates (cones, circles, rulers) | 2 | overlays, or in-scene |
| Character sheets / system content | 1 + 2 | schema-driven form renderer (systems are schema + Lua) with sparkline/meter accents; the data-oriented GUI case |
| Token palettes, tile pickers | 1 | image grids; `image-rendering: pixelated` works (netrender nearest-neighbor sampling landed) |
| Dice | 2 (4 someday) | die glyphs now; a 3D roller later because it would be delightful |

## Cross-cutting widgets

**Embedded TUI.** A terminal is a cell grid: monospace runs + background rects
+ a block cursor. Rows as DOM text (real selectable text, shaped once per dirty
row) over Path-A background-attribute rects, fed by a `vte`-parsed grid model.
Gives Mere an in-graph terminal (a node *is* a shell session) and every repo an
embedded REPL/log pane. Retention is what makes it viable: only dirty rows
reshape.

**The djot-to-IDE editor ladder.** The seed exists: `styled_text_field` /
`styled_textarea` with `StyleRange` is syntax highlighting in embryo. Ladder:

1. djot parse → StyleRanges (live highlighting);
2. gutter as an arrangement-leaf sibling column (line numbers, fold arrows,
   diagnostic dots as Path-A glyphs);
3. virtualized buffer via arrangement leaf for large files;
4. structure-aware decorations: heading anchors, link previews as overlay
   cards, wiki-links rendered with the same `GraphGlyph` leaf from Mere.

An IDE-grade editor is mostly tiers 1+3; chisel only paints the margins.

**Data grid.** The flagship arrangement-leaf widget: virtualized rows, sticky
headers via z-order, sparkline-in-cell, sortable columns. Every consumer wants
it (Mere node tables, Strophe clip lists, Isometry encounter tables). Build
once. **Landed 2026-07-08:** `chisel::grid` (GridColumn / GridSpec) +
`xilem_serval::data_grid`. Sticky-by-construction header (scroll is caller
state; the header never scrolls) with `on_header_click(col)` for caller-owned
sort; only the window's rows exist as DOM; any view rides as a cell (tested
with a `chisel_leaf` sparkline-in-cell). Wheel wiring + theming stay with the
caller (`on_wheel` + `GridSpec::max_scroll`; `grid-*` classes). This also
resolves open question 2 below: the header is DOM children of the grid root,
not a synced leaf.

**gpui-style authoring sugar.** gpui-the-framework stays excluded, but its
chained Rust styling (`el.flex().p(4).bg(var)`) is borrowable technique: a
typed `Style` builder compiling to the `style` attribute (or class + generated
sheet). Pure views-layer sugar crate, zero engine changes.

## Crate structure

- **chisel core** stays tiny: `Leaf`, `PaintCx` (+ `stroke_path` / `fill_path`
  / `arc` helpers), `LeafRegistry`, `RenderedLeaves`.
- **Catalog crates by family, not by consumer:**
  - `chisel-glyphs`: swatch, graph glyph, state dots, sparkline, gauge/knob,
    meter. Small, data-in geometry-out.
  - audio family: incubates in Strophe (pressure vessel), promotes when stable.
  - data grid, TUI: their own efforts on the arrangement leaf.
- Consumers keep their theming (colors/sizes in, no theme system in chisel).

## Build order (done conditions, not durations)

1. **PaintCx path helpers.** `stroke_path` / `fill_path` / `arc` on `PaintCx`;
   done when a polyline leaf reads as three lines of leaf code.
2. **First glyph leaves: GraphGlyph, Meter, Knob.** ~~Done when each renders via
   the genet-render `_with_leaves` test path, Knob proves the
   pointer-wrap-around-leaf pattern, and GraphGlyph draws inside a native
   button.~~ **Landed 2026-07-09.** The leaves themselves already existed in
   `chisel::glyphs`; this closed the three done-conditions:
   - *renders via `_with_leaves`*: `ports/pelt-desktop/smoke_chisel.rs` paints all
     three through `scene_from_session_dom_with_leaves`.
   - *Knob proves pointer-wrap-around-leaf*: `on_pointer(chisel_leaf(..))` drives
     `Knob::set_value` from the view layer; the leaf implements no `Leaf::event`.
     Guard `dragging_a_pointer_wrapped_leaf_drives_the_knob_from_the_view_layer`
     (xilem-serval) covers capture, clamping, and release.
   - *GraphGlyph inside a native button*: added `xilem_serval::button_with(child,
     handler)`; the existing `button` took only a text label, so a leaf could not
     ride inside a native control. Guard
     `graph_glyph_leaf_draws_inside_a_native_button_and_the_button_owns_the_click`.
     **Caveat, a real engine gap:** the button must be `display: block`. See open
     question 5.
3. **Arrangement leaf.** ~~Done when a card frame drags/stacks with DOM
   content, and a 10k-row list materializes only visible rows.~~ **Landed
   2026-07-08:** `chisel::arrange` (`Placement`, `VirtualWindow`) +
   xilem-serval `placed` / `arrangement` views; both done conditions hit in
   tests (10k rows → <30 DOM children at the honest full extent; drag/raise =
   attribute diff on a retained node), and z-over-DOM paint order proven at
   the genet-render level. Unlocks cards, grids, editors, TUI. (Note:
   xilem-serval's `overlay::Placement` is an unrelated type; `chisel::Placement`
   is not re-exported at that crate's root.)
4. **Path-B rasterize seam.** ~~done when the orrery renders as a chisel leaf~~
   **Landed 2026-07-08, in two shapes** (the port taught the split):
   - *chisel `SceneSlot` / `PaintCx::scene`* (genet `2f38249`): a leaf encodes a
     `vello::Scene`; `RenderedLeaves` splices one `DrawExternalTexture` at the box
     and epoch-gates the scene for a host rasterize pass. For vello-native widget
     leaves (a future waveform's zoomed view).
   - *host texture registry* (mere orrery port): meerkat's orrery + gloss
     scenes are **`netrender::Scene`** (the renderer's own scene — graph, camera,
     layout, gnode pool), rasterized by the host's `rasterize_for`. They ride the
     same `<external-texture>` element but as a host `HashMap<key, texture>` with
     the host's own change gate (`orrery_redraw` / resize), not a chisel leaf.
   Both retire meerkat's hardcoded `ORRERY_SCENE_KEY` / `GLOSS_MINIMAP_SCENE_KEY`
   compose branch: it is now a uniform key→texture lookup, which also dissolves
   the "no generic key→texture registry" objection the old design review raised.
   Pick by scene source: vello-built → `SceneSlot`; host `netrender::Scene`
   canvas → registry.
5. **Family crates.** Split `chisel-glyphs` out of chisel core once ≥3 leaves
   exist; audio family per the Strophe promotion rule.

## Next actions (2026-07-09)

The 2026-07-08 session landed the two hard mechanisms out of build order: tier 3
(arrangement leaf) and tier 4 (Path-B seam, both shapes) shipped, jumping over
build-order item 2 (the cheap tier-2 glyph leaves). Path-B is spent for the
orrery: the orrery/gloss port chose the **host texture registry** path (host
`netrender::Scene` rasterized by `rasterize_for`), not a chisel leaf, and
already retired `ORRERY_SCENE_KEY` / `GLOSS_MINIMAP_SCENE_KEY`. So the orrery is
not a remaining Path-B *leaf* consumer; the first `SceneSlot` **leaf** consumer
is a future Strophe waveform's zoomed view, not Mere. That leaves two Mere GUI
items, ranked:

1. **NODE_SHEET colors → CSS custom properties (highest leverage, tier 1).**
   ~~A one-time move, independent of the leaf ladder.~~ **Landed 2026-07-09.**
   `orrery::palette` is now the single source of truth (`NodeAccent` per state,
   selection-wins in one place). It emits `--node-*` custom-property
   declarations that `.stage` (the canvas node document) and `.gloss-outline`
   (a chrome panel root) paste into their root rule; descendant rules `var()`
   them, so the sheet names no color. Retired four copies of the palette
   (NODE_SHEET literals, `accent_rgb`, `cluster::state_color`, workbench's
   `TabAccent` arms). Guards: `gnode_fill_resolves_from_the_palette_custom_properties`
   and `each_gnode_state_class_resolves_its_own_palette_entry` assert the
   **resolved** computed color, because an undefined `var()` degrades silently to
   `rgba(0, 0, 0, 0)` rather than erroring (verified by falsification).

   *Structural finding:* "every representation inherits for free" holds only for
   DOM consumers. Chisel leaves and the `TabAccent` contract paint outside the
   cascade and cannot resolve a `var()`, so they read the same Rust table
   directly. The single source of truth is therefore the palette **module**, with
   custom properties as its CSS projection, not the custom properties themselves.
   Any future leaf that wants node identity takes `palette::accent`, not a var.
2. **GraphGlyph tier-2 family (build-order item 2).** **Landed 2026-07-09.**
   *Correction to this section's first draft:* it claimed `Swatch` was the only
   glyph leaf. Wrong — `chisel::glyphs` already exported `GraphGlyph`,
   `GraphGlyphNode`, `Meter`, and `Knob`, and `smoke_chisel` already painted all
   three. The claim came from reading item 2's unticked checkbox instead of the
   code. What was actually missing were item 2's *composition* proofs, not its
   leaves: nothing anywhere wrapped a leaf with pointer handling, and `button`
   accepted only a text label. Both are now closed (see build order item 2), and
   the attempt surfaced open question 5.

## Open questions

1. Where does `GraphGlyph`'s layout come from: caller-precomputed (lean this;
   keeps the leaf dumb) or a tiny embedded force pass for ≤12 nodes?
2. Data grid column model: header as DOM children of the arrangement leaf, or
   a separate synced leaf? (Sticky headers argue for children + z.)
3. TUI text: DOM rows are selectable but churn the splice on full-screen
   redraws (vim). Measure before committing; a Path-B fallback for
   full-screen-app mode may be pragmatic.
4. gpui-style sugar: `style` attribute strings (simple, stringly) vs generated
   class + sheet (cacheable, one indirection). Lean attribute first, measure.
5. ~~**Inline leaves get no box.**~~ **Found and fixed 2026-07-09.** Leaves now
   paint in inline formatting contexts, so a leaf works at any host display.
   - *Was:* `chisel_leaf_key` was stamped only on the block replaced-leaf path
     (`box_tree.rs`). A `<chisel-leaf>` in any inline formatting context was
     gathered into `InlineContent`, never minted a box, and painted nothing. A
     leaf inside a `<button>` therefore worked only if the button was
     `display: block`.
   - *Fix (engine, not a host workaround):* `InlineBoxItem` gained a
     `chisel_leaf_key`, populated for inline replaced elements in
     `construct/gather.rs` exactly as `BoxNode` does on the block path.
     `BoxTree::chisel_leaf_boxes` now also walks inline content (recursing
     through inline-blocks) so the host renders those leaves, and `paint_emit`'s
     inline-box arm splices their Path-A commands at the laid-out inline rect,
     chained onto the same replaced-payload `if`/`else if` as the inline `<img>`.
   - *Unblocked, and a bug fix in its own right:* `<button>` now gets its
     standards-correct `button { display: inline-block }` UA default. genet
     previously shipped **no** `display` rule for `<button>`, leaving it `inline`,
     which silently ignored CSS `width`/`height` on every button.
   - Guards: `a_chisel_leaf_inside_a_button_is_reported_at_every_button_display`
     (block / inline-block / inline all report the leaf) and
     `inline_chisel_leaf_splices_its_path_a_commands_at_its_inline_rect` (the
     paint half, with a translate to the inline origin).
   - *Form-control pass, done 2026-07-09:* `<input>`, `<select>`, and `<textarea>`
     had no UA `display` either, so they were `inline` and unsizable too. All four
     now get `display: inline-block`, plus `input[type=hidden] { display: none }`.
     Guards: `form_controls_get_the_inline_block_ua_display` (the UA rule, incl.
     that the type selector does not swallow other input types) and
     `a_sized_input_paints_at_its_css_size` (end-to-end: an inline-block paints its
     background at its used rect, so a sized input yields a 200x30 rect; falsified
     by dropping `input` from the rule).
   - *Still open:* intrinsic sizing. An unsized control shrink-to-fits its content
     rather than honoring `<input size>` / `<textarea rows cols>`, and genet draws
     no control widgets at all (no border, no `<select>` popup); a `<select>` just
     flows its `<option>` text. Authored CSS sizing now works, which is what the
     leaf work needed. The TUI open question (3) is unaffected — that one is about
     reshaping churn, not boxes.
