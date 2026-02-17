# Center Camera Plan (2026-02-11)

## Center Camera Plan
- Goal: Fit the graph cleanly in view with consistent padding.
- Scope: Fit-to-screen behavior and keybind only.
- Dependencies: egui_graphs `fit_to_screen` and navigation settings.
- Phase 1: Behavior audit
  - Confirm bounds include node radii and labels.
  - Ensure stable results after repeated presses.
- Phase 2: UX polish
  - Add optional animation for fit transition.
  - Respect user padding preference.
- Phase 3: Edge cases
  - Handle empty graph gracefully.
  - Handle extreme aspect ratios.
- Outputs
  - Updated fit logic or parameters.
  - Manual validation checklist.

## Findings
- 

## Progress
- 2026-02-11: Plan created.
