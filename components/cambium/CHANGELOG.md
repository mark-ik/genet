# Changelog

## 0.3.0 - 2026-07-22

- Add `HoverEvent`, `HoverPhase`, `on_hover`, and runner dispatch seams for
  host-computed Enter, Leave, and Move transitions.
- Expand the component catalog into an executable acceptance surface covering
  controls, editors, action routing, overlays, grid virtualization, semantic
  attributes, keyboard behavior, and Sprigging leaf painting.
- Give data grids explicit grid, row, column-header, and cell semantics, with
  keyboard activation for sortable headers.
- Use the canonical `genet_scripted_dom` Rust crate name throughout Cambium,
  Cambium Nematic, tests, and examples.
- Replace stale Serval-era crate documentation with the current Cambium,
  Meristem, Sprigging, and Genet ownership boundary.
- Add the retained `GraphCanvas` leaf plus Cambium's interactive
  `graph_canvas_swatch`, including focus, data keys, optional expansion, and
  visible node labels.
- Add shared overlay, command-menu, disclosure, summary, detail, sectioned-list,
  split, tab, segmented-control, and reorderable-list surfaces.
- Add `caret_text_field` for DOM-painted carets and forest-DOM projections for
  multiple window roots over one application state.
- Add Cambium Winit scroll policy and the Genet/AccessKit accessibility host.
  The source release is versioned here; crates.io publication waits for the
  standalone `genet-layout` package boundary.

## 0.2.0 - 2026-07-14

- Make `GenetCtx`, `GenetElement`, `GenetAppRunner`, and related `Genet*`
  names canonical. Deprecated `Serval*` aliases remain for migration.
- Make buttons, checkboxes, switches, radio groups, selects, and sliders follow
  standard keyboard and accessibility interaction patterns.
- Add the searchable, keyboard-complete `action_list` component.
- Make normal manifests resolve Genet seams from crates.io so a standalone
  checkout does not require a sibling Genet repository.
- Add CI for formatting, focused Clippy, workspace tests, and package checks.
