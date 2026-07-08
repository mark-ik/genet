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
once.

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
2. **First glyph leaves: GraphGlyph, Meter, Knob.** Done when each renders via
   the serval-render `_with_leaves` test path, Knob proves the
   pointer-wrap-around-leaf pattern, and GraphGlyph draws inside a native
   button.
3. **Arrangement leaf.** Done when a card frame drags/stacks with DOM content,
   and a 10k-row list materializes only visible rows. Unlocks cards, grids,
   editors, TUI.
4. **Path-B rasterize seam.** Host-side vello-scene-to-texture +
   `install_external_texture`; done when the orrery renders as a chisel leaf
   and its `compose.rs` branch is retired.
5. **Family crates.** Split `chisel-glyphs` out of chisel core once ≥3 leaves
   exist; audio family per the Strophe promotion rule.

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
