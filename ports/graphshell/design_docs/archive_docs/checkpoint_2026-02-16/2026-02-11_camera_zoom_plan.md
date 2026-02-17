# Camera Zoom Plan (2026-02-11)

## Camera Zoom Plan
- Goal: Keep zoom smooth, bounded, and consistent across frames.
- Scope: Graph view zoom and input handling only.
- Dependencies: egui_graphs settings, metadata frame access.
- Phase 1: Behavior audit
  - Confirm zoom bounds are enforced in all view states.
  - Verify zoom centers around cursor or view center.
- Phase 2: Input tuning
  - Adjust scroll scaling for trackpad vs mouse wheel.
  - Add optional keybinds for discrete zoom steps.
- Phase 3: Regression tests
  - Validate no zoom drift after repeated clamps.
  - Validate node dragging behavior at extreme zoom.
- Outputs
  - Updated tests or manual test checklist.
  - Configurable zoom bounds if needed.

## Findings
- 

## Progress
- 2026-02-11: Plan created.
