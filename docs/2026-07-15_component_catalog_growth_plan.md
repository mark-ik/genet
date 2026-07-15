# Component catalog growth plan

**Status:** active. This replaces the proposed 2026-07-09 component-catalog
plan still preserved in Genet. Cambium owns reusable view compositions;
Sprigging owns portable paint leaves. Applications continue to own product
policy and their CSS themes.

## Current acceptance surface

`crates/cambium/examples/component_catalog.rs` is the executable catalog. It
currently proves controls, text editing, a searchable action list, positioned
menu rows, a virtualized data grid, retained hover routing, Sprigging leaves,
and the interactive graph-canvas swatch.

The catalog is strongest at individual controls and paint/view composition. Its
remaining gap is the family layer: several applications still compose the same
anchored surfaces, command lists, selection bars, and reorder interactions
independently.

## Decisions

1. One behavior engine may support several named components, but each component
   keeps the standard semantics for its pattern. Menus use menu roles, choosers
   use listbox roles, and tabs use tab roles.
2. Interaction belongs in Cambium's view layer. Sprigging leaves paint geometry
   and expose semantics; application callbacks own navigation, persistence, and
   domain mutation.
3. A component enters the catalog with normal and meaningful alternate states,
   a semantic contract, a keyboard or pointer route, and a lifecycle test when
   it retains a target across rebuilds.
4. Full pane layout and docking remain in Pelt. The catalog may show an
   integration specimen, but Cambium will not grow a competing splitter tree.
5. Product canvases stay with their applications. Promote a paint leaf after a
   genuine second consumer establishes the shared contract.

## Ordered work

### C0. Retained lifecycle wall

Close the stale `NodeId` path in Genet before adding more transient and dragged
surfaces. Add an executable catalog guard that removes, replaces, and reorders
focused, hovered, and pointer-captured children without dispatching through a
retired node.

Done when hit testing cannot return a retired node, Cambium clears dead focus
and capture handles after rebuild, and focused self-replacement has a regression
test.

### C1. Anchored overlay and detail popover

Build one positioned, dismissible surface over `overlay_at`, `overlay_rect`, and
`anchor_point_clamped`. Its axes are anchor, edge placement, dismissal, and
modality. `detail_popover` is the first configuration: hover-peek, click-pin,
Escape and outside-click dismissal, optional interactive content, and focus
restoration.

First consumers are Woodshed marker details and Isometry token/tile tooltips.
The same substrate later yields toast, dialog, sheet, and drawer configurations.

Done when the catalog proves hover peek, pinned interaction, edge flipping,
Escape, outside click, and keyboard reachability.

### C2. Command surface

Unify the filtering and keyboard state in `action_list` with the positioning in
`menu`. The shared engine owns query, active item, disabled-item skipping, and
submenu navigation. Named configurations retain their own semantics: command
palette and picker are combobox/listbox patterns; context and application menus
are menu/menuitem patterns.

Done when palette, context menu, and picker render from one model, Arrow keys,
Home/End, Enter, Escape, and submenu Left/Right follow their standard patterns,
and a disabled action can expose its reason.

### C3. Selection bar

Extract one selection engine with tab, segmented-control, and filter-chip
configurations. Reuse radio-group movement where possible, while keeping the
distinct roles and activation behavior.

Done when roving focus, Arrow keys, Home/End, selection, and disabled items are
proved for every configuration.

### C4. Reorderable list

Add a keyed list interaction that reports an identity and destination to the
application. It owns pointer capture, a drop indicator, cancellation, and a
keyboard move path. The application remains responsible for applying and
persisting the reorder.

First consumers are Woodshed set rows, Isometry initiative, and Mere tile or tab
movement.

Done when pointer and keyboard reorders emit the same move, Escape restores the
original order, focus follows the moved item, and an interrupted rebuild leaves
neither capture nor a stale target.

### C5. Disclosure and summary surfaces

Add `disclosure` as the shared node beneath accordion, tree, outline, and editor
fold controls. Add a generic titled record/summary body that can live in a row,
card, detail panel, or C1 popover.

Done when an accordion and recursive tree share one disclosure primitive, and
the summary body is reused by two applications.

## Catalog refinements

- Replace the fixed-width presentation with narrow and regular specimens.
- Show disabled, empty, error, overflow, dense, and long-label states where
  relevant.
- Make the Knob specimen draggable through `on_pointer` and expose its changing
  value.
- Make graph-canvas painted focus follow actual node-button focus, and cover
  empty, single-node, and crowded subgraphs.
- Add rendered receipts at narrow and regular widths while retaining the
  headless contract as the fast acceptance wall.

## Explicit deferrals

- Pelt remains the pane/docking implementation.
- Retinue currently creates no reusable GUI pressure.
- Waveforms, automation editors, fretboards, boards, and the orrery remain
  application-owned until their second consumer fixes a smaller shared shape.
- Badge, progress, and sync indicators should use native DOM/CSS unless their
  geometry or update rate proves a Sprigging leaf is warranted.

