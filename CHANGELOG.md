# Changelog

## 0.2.0 - 2026-07-14

- Make `GenetCtx`, `GenetElement`, `GenetAppRunner`, and related `Genet*`
  names canonical. Deprecated `Serval*` aliases remain for migration.
- Make buttons, checkboxes, switches, radio groups, selects, and sliders follow
  standard keyboard and accessibility interaction patterns.
- Add the searchable, keyboard-complete `action_list` component.
- Make normal manifests resolve Genet seams from crates.io so a standalone
  checkout does not require a sibling Genet repository.
- Add CI for formatting, focused Clippy, workspace tests, and package checks.
