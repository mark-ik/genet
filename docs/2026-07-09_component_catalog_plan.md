# Component catalog: ready compositions, leaves, and widgets

**Date:** 2026-07-09
**Status:** plan, proposed.

Companion to [2026-07-07_chisel_widget_leaf_design.md](./2026-07-07_chisel_widget_leaf_design.md)
(the leaf contract: paint paths, retention gates) and
[2026-07-08_chisel_widget_catalog.md](./2026-07-08_chisel_widget_catalog.md)
(per-consumer leaf coverage). Where the chisel catalog answers "which tier
covers each need," this answers "what ready-made components ship, and how a few
primitives span many of them."

Code samples are illustrative unless marked implementation-ready.

## Goal

A catalog of ready UI components so a consumer assembles an app from parts rather
than from raw DOM. The target set is what every consumer (mere, isometry,
strophe, woodshed) would otherwise hand-compose: palettes, menus, trees, tabs,
dialogs, fields, grids. Two properties make a catalog worth more than each app
rolling its own:

1. **Shared primitives.** A small set of primitives, each parameterized into a
   family of named components. One behaviour to get right, reused everywhere.
2. **Clever collapses.** Components that read as distinct often share a
   substrate. A command palette is a context menu at a different anchor. Build
   the substrate once and configure it: cheaper to build, and more coherent to
   use.

## Three kinds

Colloquially all of these are "widgets." The catalog uses three precise kinds,
because the kind decides the home and the cost. (The chisel four-tier rule is the
finer cost model; see the leaf design doc.)

| Kind | What it is | Home | The tell |
| --- | --- | --- | --- |
| Composition | a view fn assembling DOM plus native controls (plus leaves or arrangements) | xilem-serval | asks nothing CSS cannot do |
| Leaf | a node that paints itself because CSS cannot express it | chisel | needs a pixel CSS cannot make |
| Arrangement | a node that owns its children's x, y, z | chisel engine plus an xilem-serval view | owns child geometry, virtualizes |

Most of the catalog is compositions. Leaves are the geometry cases (waveform,
knob, meter, glyph, canvas). Arrangements are the virtualization cases (grid,
cards, long lists).

## The unification principle

The spine: one primitive per family, and the named components are thin configs
over it. State each family as its primitive plus the axes that vary. The payoff
is a small primitive set with a large catalog surface, where the
"you-would-otherwise-compose-this-yourself" pieces fall out as configuration.

The exemplar is family 1 below: palette, context menu, slash menu, completion
popup, and picker are one positioned filterable list at different anchors and
scopes.

## Homes

- **xilem-serval** already is the composition library in embryo: `button`,
  `toggle`, `radio`, `select`, `slider`, `field` / `styled_field`, `menu`,
  `overlay`, `grid`, `editor`, `highlight`, `arrangement`. Promoted compositions
  land here. It stays serval-downstream (uses `ScriptedDom` concretely) and does
  not spin out, so it is a stable home.
- **chisel** holds the leaves (`GraphGlyph`, `Meter`, `Knob`, `Path`,
  `SceneSlot`) and the arrangement engine (`arrange`, `grid`). A `chisel-glyphs`
  split is the leaf catalog's build-order step 5.
- **Not forme.** `forme` is mere's workbench arrangement authority (graph into
  arrangement, geometry-free pure data). It is domain machinery, not a UI kit.
- A dedicated showcase crate is a possible later spinout, gated on the set being
  broad and stable and worth a public face. This mirrors chisel's
  spinout-when-the-seams-are-ready rule.
- **The pelt port** hosts family 7 (pane layout / docking): the
  `pelt-core::tile` contract and the pelt `TileSurface` renderer, both already
  shared and consumed beyond pelt (meerkat drives them via platen). The catalog
  references these rather than re-homing them. A host-shell tiling surface
  belongs with the host ports.

## Inventory

Status: **have** (shipped in the home crate), **promote** (exists app-side, lift
it), **new** (build it).

| Component | Kind | Family | Status |
| --- | --- | --- | --- |
| button, toggle, radio, select, slider | composition | 3, 4 | have (xilem-serval) |
| field, styled_field, text_input | composition | 3 | have (xilem-serval) |
| menu (positioned rows) | composition | 1 | have (xilem-serval, thin) |
| overlay_at, overlay_rect, anchor_point, Placement | primitive | 2 | have (xilem-serval) |
| editor, highlight (StyleRange) | composition | 3 | have (xilem-serval) |
| data_grid, chisel::grid | arrangement | 5 | have, first consumer this session |
| TileTree contract + pelt TileSurface | arrangement | 7 | have (pelt-core + pelt port; mere drives via platen) |
| GraphGlyph, Meter, Knob, Path, SceneSlot | leaf | 8 | have (chisel) |
| palette, searchable context menu, submenus, keyboard | composition | 1 | promote (mere `chrome_menu.rs`) |
| overlay_panel | composition | 2 | promote (isometry) |
| search_field | composition | 3 | promote (isometry) |
| tab_strip | composition | 4 | promote (isometry) |
| stat_list, record_card | composition | 9 | promote (isometry) |
| tooltip, toast, popover, dialog, sheet, drawer | composition | 2 | new (configs over 2) |
| combobox, tag input, inline edit, stepper | composition | 3 | new (configs over 3, some over 1) |
| segmented control, filter chips | composition | 4 | new (configs over 4) |
| tree, accordion, outline, file explorer | composition | 6 | new |
| kanban, card deck, gallery, list | arrangement | 5 | new (configs over 5) |
| sparkline, badge, progress, sync indicator | leaf | 8 | new (glyph leaves) |

## Families and build order

Done conditions, not durations. Promotion bar: a component graduates from an app
into the catalog when a second consumer wants it, or when one consumer plus a
clear general shape justifies it (the pressure-vessel rule).

### 1. Filterable action list (flagship)

Primitive: a positioned list of actions with optional filter, keyboard
selection, and submenus.
Spans: command palette, context menu, slash menu, completion popup, quick-open,
picker.
Varies by: anchor (cursor, center, element), scope (global, target), filter
shown, submenu depth.
Status: mere has the full machine (`chrome_menu.rs`: query, submenus, keyboard,
`run_context_selection`; plus `palette_open` / `palette_input` /
`run_palette_selection`). xilem-serval has the thin render primitive (`menu()`).
Done when: one xilem-serval primitive (working name `command_menu`) renders
palette, context menu, and picker from one config surface; it owns query,
selection, and keyboard (today `menu()` pushes all of that to the host); mere
re-consumes it, retiring its private render; isometry gets a palette from the
same call.

### 2. Overlay layer (the substrate under 1)

Primitive: a positioned, dismissible surface.
Spans: tooltip, popover, toast, dialog, sheet, drawer.
Varies by: anchor (cursor, element, edge, center), dismiss (hover-out,
click-out, timeout, explicit), modality (scrim or bare).
Status: `overlay_at` / `overlay_rect` / `anchor_point` / `anchor_point_clamped`
/ `Placement` live; isometry's `overlay_panel` is the titled-panel config.
Done when: the dismiss and modality axes are first-class (today the host wires
dismiss), `overlay_panel` is promoted, and a toast and a dialog fall out as
configs.

### 3. Field (value plus edit affordance)

Primitive: display a value, edit it, commit it.
Spans: text input, search field, inline edit, stepper, combobox, tag input.
Varies by: value type, commit trigger, whether results attach (a search field is
a field feeding family 1).
Status: `field` / `styled_field` / `slider` / `text_input` live; isometry's
`search_field` is a promote.
Done when: `search_field` is promoted, and combobox and stepper are configs.

### 4. Single-select from a set

Primitive: choose one of a set, with the choice styled.
Spans: tabs, segmented control, radio group, filter chips.
Status: `radio` / `select` live; isometry's `tab_strip` is a promote.
Done when: `tab_strip` is promoted and segmented and chips are configs.

### 5. Virtualized arrangement of uniform items

Primitive: the arrangement leaf materializing only the visible window.
Spans: data grid, table, list, kanban column, card deck, gallery.
Varies by: axis (row, column, wrap), cell view, sticky regions.
Status: `chisel::grid` plus `data_grid` landed, first production consumer this
session (the isometry compendium). `record_card` / `stat_list` are the cell and
summary configs, a promote.
Done when: a list and a gallery fall out of the same engine, and `record_card`
is a promoted cell.

### 6. Disclosure of nested content (new)

Primitive: an expandable node holding nested content.
Spans: accordion, tree, outline, file explorer, nav sidebar.
Varies by: recursion (flat accordion versus recursive tree), exclusive versus
multi-open.
Status: new. A large tree rides family 5 for virtualization.
Done when: tree and accordion share one node primitive, and a first consumer
uses it.

### 7. Pane layout / docking

Primitive: a recursive tree of splits (row or column with fractional shares) and
tab-stacks (tabbed tiles, one active), rendered by an interactive surface
(dividers, drag-resize, tab bars, drag-to-rearrange, close) that emits events the
host applies.
Spans: tiling workbench, editor groups, dock panels, tab-and-split browser
shells.
Varies by: split axis, tab versus split at a node, which content lane a tile
carries.
Status: shipped, and already beyond one consumer. The contract is
`pelt-core::tile::TileTree` (presentation-only: splits, tabs, fractions, two
content lanes, and by design nothing about graphs, sessions, or relations); the
renderer is the pelt `TileSurface`. Standalone pelt drives it from its own state;
meerkat hosts the same surface as its workbench pane through platen's
`tree_projection`.
Home exception: unlike families 1 to 6, this family's home is the pelt port. The
contract lives in `pelt-core` and the renderer in the pelt port, both already
shared, so the catalog references them rather than re-homing them.
Out of scope: the mere-only semantic layer that targets this contract (forme's
arrangement graph: `CompareWith` / `FocusPath` / `MirrorOf`) and platen's other
projections (lattice, corridor, spatial bench, graph canvas). Those are graph
projections, not general components. The boundary is guarded one way: forme maps
onto the tile-tree, the tile-tree never grows toward forme.
Done when: catalog-side this is a reference to existing shared pieces, not new
work. A future extension (detached or floating tiles, cross-window docking) would
grow the `pelt-core::tile` contract.

### 8. Status glyph (leaf)

Primitive: a small leaf painting one real signal.
Spans: meter, sparkline, badge, progress, sync indicator, graph glyph.
Status: `GraphGlyph` / `Meter` / `Knob` live in chisel; sparkline, badge,
progress, and a real sync indicator are new glyph leaves.
Done when: each new glyph renders through the chisel `_with_leaves` path from real
state, and the sync indicator reflects genuine sync rather than a placebo spinner.

### 9. Record / summary surface

Primitive: a titled block presenting one record's fields.
Spans: card, list row, detail panel, hover card, breadcrumb.
Status: isometry's `record_card` and `stat_list` are the promotes; hover card and
breadcrumb are new.
Done when: `record_card` and `stat_list` are promoted, and a hover card falls out
as a family 9 body inside a family 2 overlay.

## Composition rules (inherited)

From the chisel catalog, unchanged: text stays out of leaves (labels are DOM);
consumers keep their theming (colours and sizes in via CSS vars and classes; the
catalog ships no theme system of its own). One relaxation: the chisel rule
"interaction lives in the view layer, not the leaf" holds for leaves, but a
catalog composition may own its own interaction where that is the point of the
component. A palette owns its query and keyboard because the palette is the view
layer.

## Open questions

1. Interaction ownership. `menu()` pushes query, selection, and keyboard to the
   host. A catalog palette should own them and expose `actions` plus `on_pick`.
   Set the line once so the family stays consistent.
2. Promotion bar. Two consumers, or one plus a clear general shape? Lean two,
   with the exception written down.
3. Home. Grow xilem-serval now (recommended) versus stand up a dedicated
   showcase crate. Revisit when the set is broad and stable.
4. Naming. The catalog may want a name; the primitives certainly do
   (`command_menu`? `disclosure`?). Maintainer's call, per the plain-vocabulary
   rule.

## Cross-references

- [chisel leaf design](./2026-07-07_chisel_widget_leaf_design.md),
  [chisel widget catalog](./2026-07-08_chisel_widget_catalog.md).
- data_grid's first consumer: the isometry compendium (isometry
  `design_docs/2026-07-08_campaign_packs_plan.md`).
- Cards and tear-out share family 2 and 5 with the one-state-N-windows model.
- Family 7 sources: `pelt-core::tile` (the TileTree contract), the pelt
  `TileSurface`, platen's `tree_projection`, and the mere composition spine
  (`repos/mere/design_docs/mere_docs/technical_architecture/2026-05-21_mere_composition_spine.md`),
  which places forme, platen, and the projection ladder above this contract.
