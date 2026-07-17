# Component catalog acceptance surface

`crates/cambium/examples/component_catalog.rs` is both a renderable catalog and
an executable acceptance test. It is the first place a new Cambium component
should appear.

The ordered expansion and promotion rules live in
[`2026-07-15_component_catalog_growth_plan.md`](2026-07-15_component_catalog_growth_plan.md).

## Covered contracts

| Family | Acceptance evidence |
| --- | --- |
| Button, checkbox, switch | Pointer activation, keyboard activation, state rebuild, ARIA state |
| Hover target | Enter, Move, and Leave routing through the retained message path |
| Radio group | One selected item, accessible group name, arrow-key selection |
| Select | Combobox state, End selection, Escape dismissal |
| Slider | Accessible value and configurable PageUp step |
| Text fields | Single-line editing, multiline structure, styled runs |
| Command surfaces | One model rendered as palette, picker, and positioned context menu; pattern-specific roles, disabled reasons, shared navigation, and depth-one submenus |
| Action list compatibility | Existing API delegates to the command palette engine |
| Selection bars | Shared roving focus rendered as linked tabs, a single-select segmented control, and multi-select filter chips |
| Reorderable list | Keyed rows with pointer capture, keyboard move mode, direct Alt+Arrow movement, cancellation, drop indication, and application-owned persistence |
| Overlay surface | Edge-aware placement, dialog semantics, outside-click dismissal, passive Escape routing |
| Detail popover | Hover preview, click-pinned interactive detail, Escape dismissal, and trigger focus return |
| Data grid | Grid/row/header/cell semantics, keyboard-sortable headers, and bounded DOM rows for a 10,000-row model |
| Graph-canvas swatch | Shared graph paint path, native node targets, click/hover routing, selected/focus state, expand action |
| Sprigging leaves | Five catalog leaf elements, retained registry entries, paint commands, clean repaint gate |
| Retained lifecycle | Self-replacing focused and pointer-captured controls retire stale handles before the next dispatch |

The companion CSS is application-owned and uses custom properties. It includes
visible focus, disabled, selected, editor, menu, grid, and leaf states so a
visual host can expose structural regressions immediately.

## Commands

Run the executable headless contract:

```sh
cargo run -p cambium --example component_catalog --all-features
```

Run it through the test harness:

```sh
cargo test -p cambium --example component_catalog --all-features
```

CI uses `cargo test --workspace --all-features --all-targets --locked`, which
keeps the example in the required test set.

## Addition rule

A new reusable component is catalog-complete when:

1. the catalog renders its normal and meaningful alternate states;
2. the headless contract asserts its semantic attributes;
3. one pointer or keyboard path proves its state or action routing;
4. the theme exposes its focus, selected, disabled, or error state as relevant;
5. the example still packages with the `cambium` crate.

Genet's layout, paint-list integration, platform input, and final AccessKit tree
remain provider-side acceptance work. The catalog checks Cambium's DOM contract
and Sprigging's portable paint output without duplicating those engine tests.
