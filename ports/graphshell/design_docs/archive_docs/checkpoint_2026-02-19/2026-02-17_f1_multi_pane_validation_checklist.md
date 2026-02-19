# F1 Multi-Pane Validation Checklist (Desktop)

## Purpose

Focused validation for F1 exit criteria:
- multiple webview panes visible in one frame,
- focused-target routing correctness,
- no non-focused-pane teardown from focus changes,
- explicit focused-pane affordance and cold-node reactivation behavior.

## Automated Coverage (Unit)

- [x] Split-open creates linear root and reuses existing linear root.
- [x] Focus hint drives frame activation target (`webview_for_frame_activation`).
- [x] Fallback activation uses active tile when hint is stale/inactive.
- [x] Split layout retains both webview tiles when focus target changes.
- [x] Toolbar target resolution falls back to selected node when no live focused webview exists (`desktop::nav_targeting::tests::test_focused_toolbar_node_falls_back_to_selected_node_when_no_live_focus`).
- [x] Pointer focus-retarget rule is deterministic (retarget on mouse press only) (`desktop::headed_window::tests::test_should_retarget_webview_focus_only_on_press`).

Key test file:
- `ports/graphshell/desktop/gui.rs` (`desktop::gui::tests::*split*`, `*focused*`, `*frame_activation*`)
- `ports/graphshell/desktop/nav_targeting.rs`
- `ports/graphshell/desktop/headed_window.rs`

Automated status (2026-02-18):
- `cargo test -p graphshell --lib`: Passed.

## Headed Manual Checklist

### Test Baseline (Use These URLs)

Use stable low-complexity pages for F1 validation:
- `https://example.com`
- `https://httpbin.org/html`
- `https://neverssl.com`

Avoid using highly dynamic sites (for example Google properties) for pass/fail on F1 architecture behavior.

1. Create two detail panes:
- Open one node in detail view.
- Drag a tab out to split, or use `Split+` / `Shift + Double-click` as fallback.
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

## Headed Manual Re-Run (2026-02-18, Post-Hardening)

Run this pass after desktop UX hardening (focused-pane ring, deterministic click focus, cold-node Reactivate action):

1. Focus affordance:
- Open two panes.
- Click pane A, then pane B.
- Confirm focused pane shows visible focus ring and ring moves on click.
Result (2026-02-18): Confirmed.

2. Focus switching does not hide other pane:
- With both panes visible, click/scroll/type in each pane.
- Confirm non-focused pane remains visible and interactive.
Result (2026-02-18): Confirmed.

3. Omnibar targeting with cold-pane fallback:
- Close pane A tile, leaving node A as `Cold`.
- Re-open node A tile so it has no active webview.
- With pane A selected, submit URL in omnibar.
- Confirm node A is promoted/reactivated and navigates in pane A (not a random/new pane).
Result (2026-02-18): Confirmed.

4. In-pane Reactivate action:
- For a pane showing "No active WebView", click `Reactivate`.
- Confirm node lifecycle promotes to `Active` and webview appears without requiring a new tab.
Result (2026-02-18): Confirmed.

## Headed Grouping Behavior Addendum (Next Slice)

Use this short pass for deterministic `UserGrouped` trigger validation:

1. Split trigger creates edge:
- Select node A, then `Shift + Double-click` node B.
- Confirm exactly one `UserGrouped` edge A->B is created.

2. Drag-into-same-tab-group trigger:
- Open nodes A and B in separate detail panes.
- Drag one pane into the same tabs container as the other.
- Confirm expected `UserGrouped` edge behavior (create once, no duplicates).

3. Explicit group-with-focused action:
- Focus node A/pane A.
- Run explicit "group with focused" command on node B.
- Confirm exactly one `UserGrouped` edge A->B is created.

4. No-trigger paths:
- Switch pane focus, switch tabs, and navigate URLs.
- Confirm no new `UserGrouped` edges are created from those actions alone.

## Known Limits

- Unit tests cannot assert actual GPU compositing in one frame; headed manual validation is required for that final gate.
- EGL/WebDriver explicit-target parity is out of scope for this checklist (desktop-only cycle scope).
- Some websites may emit warnings/errors due to current Servo web-platform support gaps (for example missing `IntersectionObserver`, `AbortError`, script `IndexSizeError`). Treat those as site/runtime noise unless they correlate with deterministic Graphshell routing/lifecycle regressions on the baseline URLs above.
