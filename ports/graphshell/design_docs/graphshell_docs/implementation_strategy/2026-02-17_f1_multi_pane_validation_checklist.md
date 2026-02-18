# F1 Multi-Pane Validation Checklist (Desktop)

## Purpose

Focused validation for F1 exit criteria:
- multiple webview panes visible in one frame,
- focused-target routing correctness,
- no non-focused-pane teardown from focus changes.

## Automated Coverage (Unit)

- [x] Split-open creates linear root and reuses existing linear root.
- [x] Focus hint drives frame activation target (`webview_for_frame_activation`).
- [x] Fallback activation uses active tile when hint is stale/inactive.
- [x] Split layout retains both webview tiles when focus target changes.

Key test file:
- `ports/graphshell/desktop/gui.rs` (`desktop::gui::tests::*split*`, `*focused*`, `*frame_activation*`)

## Headed Manual Checklist

### Test Baseline (Use These URLs)

Use stable low-complexity pages for F1 validation:
- `https://example.com`
- `https://httpbin.org/html`
- `https://neverssl.com`

Avoid using highly dynamic sites (for example Google properties) for pass/fail on F1 architecture behavior.

1. Create two detail panes:
- Open one node in detail view.
- Use `Split+` or `Shift + Double-click` on a second node.
- Confirm both panes remain visible.
Result (2026-02-17, baseline run): Passed (`example.com` + `Split+` kept both panes visible).

2. Focus switch should not hide other pane:
- Click inside pane A webview, then pane B webview.
- Confirm both panes stay rendered and interactive after switching focus.
Result (2026-02-17, baseline run): Passed.

3. Omnibar targets focused pane:
- Focus pane A, submit URL in omnibar, verify pane A navigates.
- Focus pane B, submit URL in omnibar, verify pane B navigates.
Result (2026-02-17, baseline run): Passed.

4. Toolbar back/forward/reload targets focused pane:
- With distinct histories in A and B, focus A then use controls.
- Repeat for B.
- Confirm controls affect focused pane only.
Result (2026-02-17, baseline run): Passed (controls affected focused pane only).

5. Tile close semantics:
- Close pane A tile.
- Confirm pane B remains active.
- Confirm node A remains in graph as `Cold` (reactivatable) unless explicitly deleted.
Result (2026-02-17, baseline run): Passed.

## Known Limits

- Unit tests cannot assert actual GPU compositing in one frame; headed manual validation is required for that final gate.
- EGL/WebDriver explicit-target parity is out of scope for this checklist (desktop-only cycle scope).
- Some websites may emit warnings/errors due to current Servo web-platform support gaps (for example missing `IntersectionObserver`, `AbortError`, script `IndexSizeError`). Treat those as site/runtime noise unless they correlate with deterministic Graphshell routing/lifecycle regressions on the baseline URLs above.
