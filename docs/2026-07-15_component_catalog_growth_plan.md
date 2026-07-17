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

**Landed 2026-07-15.** Genet's retained hit-test boundary rejects retired
nodes before publishing a result. Cambium clears dead focus and pointer capture
after every rebuild, ignores stale host targets at every public dispatch seam,
and exercises focused and captured self-replacement in both runner tests and the
executable component catalog.

Close the stale `NodeId` path in Genet before adding more transient and dragged
surfaces. Add an executable catalog guard that removes, replaces, and reorders
focused, hovered, and pointer-captured children without dispatching through a
retired node.

Done when hit testing cannot return a retired node, Cambium clears dead focus
and capture handles after rebuild, and focused self-replacement has a regression
test.

### C1. Anchored overlay and detail popover

**Landed 2026-07-15.** `OverlaySurface` owns clamped placement, role and label
semantics, optional modal state, outside-click interception, and Escape
dismissal through a passive ancestor key listener that adds no Tab stop.
`detail_popover` adds distinct informational preview and interactive pinned
content, with hover-peek, click-pin, and focus return after dismissal. The
catalog exercises the complete path.

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

**Landed 2026-07-16.** `CommandState` and `CommandItem` now drive three named
configurations. The palette uses combobox/listbox semantics and owns filtering;
the picker uses listbox semantics; the positioned context menu uses
menu/menuitem semantics with a depth-one submenu. All three share active-item,
Home/End, Arrow, Enter, and Escape handling. Palette and picker navigation skip
disabled options; menus keep disabled commands focusable but inert, matching
the WAI-ARIA menu pattern. Disabled commands may carry a visible and accessible
reason. The old `action_list` names and DOM classes delegate to the palette
engine, while the old click-only `menu` remains source-compatible for Mere's
completion popup.
Semantic and keyboard choices follow the W3C WAI-ARIA
[combobox](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/),
[listbox](https://www.w3.org/WAI/ARIA/apg/patterns/listbox/), and
[menu](https://www.w3.org/WAI/ARIA/apg/patterns/menubar/) patterns.

Unify the filtering and keyboard state in `action_list` with the positioning in
`menu`. The shared engine owns query, active item, pattern-appropriate disabled
navigation, and submenu navigation. Named configurations retain their own
semantics: command
palette and picker are combobox/listbox patterns; context and application menus
are menu/menuitem patterns.

Done when palette, context menu, and picker render from one model, Arrow keys,
Home/End, Enter, Escape, and submenu Left/Right follow their standard patterns,
and a disabled action can expose its reason.

### C3. Selection bar

**Landed 2026-07-16.** `SelectionState` and `SelectionItem` now drive tabs,
segmented controls, and filter chips through one roving-focus engine. Tabs
support configurable automatic or manual activation and tab/tabpanel linkage;
segmented controls use radiogroup/radio semantics and select on movement;
filter chips use a toolbar of toggle buttons and move focus independently of
their multi-selection. Every configuration covers Arrow keys, Home/End,
pointer or keyboard activation, focus transfer, and disabled-item skipping.
The behavior follows the W3C WAI-ARIA
[tabs](https://www.w3.org/WAI/ARIA/apg/patterns/tabs/),
[radio group](https://www.w3.org/WAI/ARIA/apg/patterns/radio/),
[toolbar](https://www.w3.org/WAI/ARIA/apg/patterns/toolbar/), and
[toggle button](https://www.w3.org/WAI/ARIA/apg/patterns/button/) patterns.

Extract one selection engine with tab, segmented-control, and filter-chip
configurations. Reuse radio-group movement where possible, while keeping the
distinct roles and activation behavior.

Done when roving focus, Arrow keys, Home/End, selection, and disabled items are
proved for every configuration.

### C4. Reorderable list

**Landed 2026-07-16.** `ReorderState`, `ReorderItem`, and `ReorderMove` now
provide one keyed list interaction for pointer capture and keyboard movement.
Cambium owns the transient drag, roving focus, drop indicator, cancellation,
and polite status announcement; the application receives identity plus final
index and remains responsible for applying and persisting the move. Space or
Enter enters the keyboard move mode, Arrow keys and Home/End place the target,
Escape cancels, and `Alt+Arrow` provides the direct shortcut used by the W3C
WAI-ARIA [rearrangeable listbox example](https://www.w3.org/WAI/ARIA/apg/patterns/listbox/examples/listbox-rearrangeable/).
`reorderable_list_with` lets consumers supply the row body without taking over
the interaction shell.
The catalog proves equivalent pointer and keyboard outputs, cancellation,
focus retention across the keyed DOM move, and capture cleanup when a dragged
row disappears during rebuild.

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

**Landed 2026-07-17.** One internal disclosure control now owns trigger/panel
linkage, expanded state, pointer activation, keyboard activation, focusability,
and disabled behavior. `disclosure`, `accordion`, and `tree_view` compose that
same node into their distinct WAI-ARIA patterns: accordion adds configurable
heading levels, optional panel regions, and single/multiple expansion; the tree
adds nested groups, roving focus, configurable explicit or focus-following
selection, type-ahead, and the standard Arrow/Home/End hierarchy.
`summary_body` is a context-neutral labelled record body with eyebrow,
description, and definition-list facts; the catalog
reuses it in card, compact-row, and accordion-panel contexts. The contracts
follow the W3C WAI-ARIA [disclosure](https://www.w3.org/WAI/ARIA/apg/patterns/disclosure/),
[accordion](https://www.w3.org/WAI/ARIA/apg/patterns/accordion/), and
[tree view](https://www.w3.org/WAI/ARIA/apg/patterns/treeview/) patterns.

Add `disclosure` as the shared node beneath accordion, tree, outline, and editor
fold controls. Add a generic titled record/summary body that can live in a row,
card, detail panel, or C1 popover.

Done when an accordion and recursive tree share one disclosure primitive, and
the summary body is reused by two applications.

## Catalog refinements

**Landed 2026-07-17.** The catalog now renders from configurable narrow and
regular specimen widths, with explicit disabled, empty, error, overflow, dense,
and long-label states. The Knob is a pointer-captured control with a live value
output. Graph-canvas node targets participate in runner focus through the new
`on_focus` view, so retained paint emphasis follows pointer, keyboard, and
programmatic focus; empty, single-node, and crowded subgraphs sit beside the
ordinary card. Self-contained narrow and regular HTML receipts serialize the
same live `ScriptedDom` and stylesheet, and a byte-for-byte test keeps them in
sync while the existing headless interaction and Sprigging paint assertions
remain the fast acceptance wall.

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
