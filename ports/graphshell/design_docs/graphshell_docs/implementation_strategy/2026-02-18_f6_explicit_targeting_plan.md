# F6 EGL and WebDriver Explicit Targeting Plan (2026-02-18)

## Purpose

Define a local-first, compatibility-safe implementation plan for F6:

- eliminate implicit active/newest targeting in graphshell EGL/WebDriver flows,
- preserve Servo compatibility and servoshell baseline behavior,
- establish a high, explicit bar before proposing any upstream API changes.

This document is prepared for critique and intentionally calls out decision gates, risks, and escalation criteria.

## Relationship to Existing Plans

- Depends on completed desktop targeting work from `2026-02-17_feature_priority_dependency_plan.md` (F1/F2).
- Complements `2026-02-17_egl_embedder_extension_plan.md` by narrowing to explicit-target semantics and compatibility constraints.
- Must remain consistent with `2026-02-16_architecture_and_navigation_plan.md` control-plane rules.

## Architectural Context (from code audit)

Understanding the actual API boundary reshapes the problem:

**Servo's `WebView` API is handle-based.** Every method (`.load()`, `.go_back()`, `.notify_input_event()`, etc.) targets `self` by calling `self.id()` internally. There is no separate "target" parameter — the `WebView` handle IS the target. The "implicit targeting" problem does not exist in the servo core; it exists entirely in the embedder-side `WebViewCollection` convenience layer and the `input_target_webview()` resolution helper.

**The EGL `app.rs` files are forks, not shared code.** `ports/graphshell/egl/app.rs` (708 lines) and `ports/servoshell/egl/app.rs` (685 lines) are structurally identical forks. The graphshell version has already been modernized: it calls WebView methods directly (`.load()`, `.go_back()`) rather than queuing `UserInterfaceCommand` variants. There is no shared `ports/shared` directory — each port maintains its own copy. **Graphshell can freely modify its EGL API without touching servoshell.**

**`WebViewCollection` is embedder-side, not upstream.** Defined separately in `graphshell/running_app_state.rs` and `servoshell/running_app_state.rs`. The `active_id()`, `newest()`, and `activate_webview()` methods are graphshell-owned code.

**Desktop targeting is already resolved.** `HeadedWindow` overrides `preferred_input_webview_id` to use `gui.focused_tile_webview_id()` with a narrow re-entrant fallback to `active_id()` (at `headed_window.rs:1003-1011`).

**Implication:** The constraint "No Servo behavior regressions for existing servoshell paths" is automatically satisfied because the files are separate. The only compatibility concern is existing OHOS/Android host callers of the graphshell EGL `App` public methods.

## Problem Statement

Graphshell desktop now routes by focused tile/webview target, but EGL and some WebDriver glue still rely on implicit per-window active/newest selection.

The implicit targeting is concentrated in one resolution function:

- `input_target_webview()` at `egl/app.rs:358-367` chains `preferred_input_webview_id` -> fallback to `newest()`.
- All 22 EGL public methods call through this single function.
- This centralization already exists — the plan's work is adding explicit-id overloads, not restructuring scattered resolution.

## Constraints

1. No Servo behavior regressions for existing servoshell paths. (Automatically satisfied — files are forks.)
2. No breaking API changes in graphshell EGL public methods — existing host callers must compile unchanged.
3. No reintroduction of global-active authority into desktop tile flow.
4. Keep prototype velocity: prioritize local implementation and tests over upstream design churn.

## Goals

1. Make graphshell EGL/WebDriver command targeting explicit by `WebViewId` where graphshell owns the call path.
2. Restrict implicit fallback selection to narrow compatibility adapter boundaries.
3. Add diagnostics and tests that prove command routing correctness under multi-webview conditions.
4. Produce objective evidence for whether upstream changes are truly required.

## Non-Goals

1. Rewriting servoshell core abstractions for all embedders.
2. Full EGL multi-window productization in this phase.
3. Any upstream API change without a reproduced, local-unblockable gap.

## Design Principles

1. Local first: exhaust graphshell-owned refactors before upstream asks.
2. Narrow adapters: if fallback is needed, isolate it in one function per subsystem.
3. Preserve compatibility: default behavior remains servoshell-compatible unless graphshell path opts into explicit target mode.
4. Measure before escalations: blockers must be tied to tests and concrete call sites.

---

## Complete Targeting Audit (from code review)

### Resolution Layer

| Location | Function | Behavior | Notes |
|---|---|---|---|
| `graphshell/window.rs:606-608` | `preferred_input_webview_id` (trait default) | Returns `active_id()` | Default for platforms that don't override |
| `graphshell/desktop/headed_window.rs:1003-1011` | `preferred_input_webview_id` (override) | Returns `gui.focused_tile_webview_id()`, falls back to `active_id()` on re-entrant borrow | Desktop is already correct |
| `graphshell/egl/app.rs:358-367` | `input_target_webview()` | Chains `preferred_input_webview_id` -> `newest()` fallback | Single resolution point for all 22 EGL callers |

### EGL Callers of `input_target_webview()` (all in `egl/app.rs`)

All 22 call sites below resolve their target through `input_target_webview()`: 21 are `App` methods and 1 is a `PlatformWindow` state hook. Each can be made explicit by `WebViewId`.

| Line | Method | Category |
|---|---|---|
| 89-94 | `update_user_interface_state()` | state query |
| 402 | `load_uri()` | navigation |
| 411 | `reload()` | navigation |
| 427 | `go_back()` | navigation |
| 435 | `go_forward()` | navigation |
| 443 | `resize()` | display |
| 459 | `scroll()` | input |
| 469 | `touch_down()` | input |
| 481 | `touch_move()` | input |
| 493 | `touch_up()` | input |
| 505 | `touch_cancel()` | input |
| 517 | `mouse_move()` | input |
| 527 | `mouse_down()` | input |
| 539 | `mouse_up()` | input |
| 552 | `pinchzoom_start()` | input |
| 561 | `pinchzoom()` | input |
| 570 | `pinchzoom_end()` | input |
| 577 | `key_down()` | input |
| 587 | `key_up()` | input |
| 602 | `ime_insert_text()` | input |
| 622 | `media_session_action()` | media |
| 629 | `set_throttled()` | display |

**Classification: all `wrapper-needed`.** Each can gain an explicit-id overload. The existing method becomes a compatibility wrapper that calls `input_target_webview()` then delegates.

### Other `preferred_input_webview_id` Call Sites

| File | Line | Context | Classification |
|---|---|---|---|
| `graphshell/window.rs:170` | `repaint_webviews()` | Selects which webview to paint | compatibility baseline (paint is inherently about "the visible one") |
| `graphshell/webdriver.rs:167` | `NewWindow` handler | Discovers webview in newly-created window | protocol-correct (`newest()` after window creation is the right semantic) |
| `graphshell/webdriver.rs:267` | `GetFocusedWebView` | Returns focused webview per WebDriver spec | explicit-ready (spec-correct as-is) |
| `graphshell/running_app_state.rs:610` | `handle_gamepad_events()` | Routes gamepad to focused webview | wrapper-needed |
| `graphshell/desktop/webview_controller.rs:109` | `sync_to_graph_intents()` | Reconciliation layer | compatibility baseline |
| `graphshell/egl/ohos/mod.rs:428` | `FocusWebview` OHOS handler | Reads current before switching | comparison logic, not dispatch |

### `active_id()` Direct Callers

| File | Line | Context | Classification |
|---|---|---|---|
| `graphshell/window.rs:255` | `get_active_webview_index()` | Query (not dispatch) | no action needed |
| `graphshell/window.rs:607` | Trait default `preferred_input_webview_id` | Resolution layer | keep as default, override where needed |
| `graphshell/desktop/headed_window.rs:1010` | Re-entrant fallback | Emergency path | keep (narrow, documented) |

### `newest()` Callers

| File | Line | Context | Classification |
|---|---|---|---|
| `graphshell/egl/app.rs:364` | `input_target_webview()` fallback | Single-window compat shim | add deprecation log, remove when multi-webview EGL ships |
| `graphshell/webdriver.rs:169` | `NewWindow` after window creation | Protocol-correct discovery | no action needed |

### Hard Gaps (upstream API surface)

| Gap                       | Location                              | Severity | Action              |
| ------------------------- | ------------------------------------- | -------- | ------------------- |
| `WebView::stop()` missing | `egl/app.rs:419-423` (no-op stub)     | Low      | Document; see below |

**`WebView::stop()` detail:** The DOM has `stop_loading()` at `components/script/dom/window.rs:834` but it is not exposed through the public `WebView` API. No `EmbedderToConstellationMessage::StopLoading` variant exists. Consider filing a lightweight upstream issue with a one-line additive proposal: `pub fn stop(&self)` that sends `StopLoading(self.id())` to the constellation. Do not gate F6 on this.

### Servoshell Comparison (for reference, no action required)

Servoshell does not have `preferred_input_webview_id`. It uses `active_webview()` directly. Its EGL `app.rs` still queues `UserInterfaceCommand` variants. 7 callers of `active_webview()` in servoshell (4 navigation commands, 1 state query, 1 gamepad, 1 repaint). These are in servoshell's own forked files and are unaffected by any graphshell changes.

---

## Audit Conclusions

1. **The problem is smaller than initially framed.** The targeting indirection is already centralized in `input_target_webview()`. There is no scattered ad-hoc resolution to consolidate.
2. **No upstream changes are needed.** The Servo `WebView` API is handle-based — every method targets `self`. The implicit targeting lives entirely in graphshell's own embedder layer.
3. **WebDriver is already explicit-id** for all command dispatch. The `NewWindow` use of `newest()` is protocol-correct. `GetFocusedWebView` uses `preferred_input_webview_id` which is spec-correct.
4. **The only hard gap is `WebView::stop()`**, which is a low-severity no-op stub. It does not block F6 work.
5. **The EGL files are forks** — graphshell changes cannot regress servoshell. The compatibility concern is limited to existing OHOS/Android host callers of the graphshell `App` public methods.
6. **Scope boundary for this phase:** Explicit-target overloads remove implicit-target coupling in command routing, but do not by themselves remove inherited single-window/single-active scaffolding (for example `App::window()` single-window accessor, active-oriented UI state fields in `EmbeddedPlatformWindow`, and default `WebViewCollection` active-id semantics). Structural removal of that scaffolding is follow-up work outside F6 done criteria.

---

## Plan (Revised Post-Audit)

The original four-phase plan is collapsed to a single implementation pass, since the audit shows the problem is concentrated and the WebDriver path needs minimal work.

### Step 1: Add Explicit-ID Overloads to EGL `App` Methods + 1 Platform Hook

For the 21 `App`-method callers, add an `_for_webview(webview_id: WebViewId, ...)` variant that takes an explicit target. Keep the 1 `PlatformWindow` state hook (`update_user_interface_state`) explicit internally without expanding public `App` API unnecessarily. Existing public methods remain compatibility wrappers:

```rust
// New: explicit target
pub fn load_uri_for_webview(&self, webview_id: WebViewId, uri: &str) { ... }

// Existing: compatibility wrapper (unchanged signature for hosts)
pub fn load_uri(&self, uri: &str) {
    if let Some(wv) = self.input_target_webview() {
        self.load_uri_for_webview(wv.id(), uri);
    }
}
```

Mechanical work. ~100-150 lines. No behavior change for existing callers.

#### Addendum A: API Visibility Policy for `_for_webview` Methods

To avoid unnecessary long-term surface area while preserving host compatibility:

1. Keep existing wrapper signatures public and unchanged.
2. Make `_for_webview` methods `pub` only for host-facing commands that external callers may need to target explicitly (navigation/input/media/display entrypoints exposed by `App`).
3. Prefer `pub(crate)` for purely internal helper splits that do not need external host invocation.
4. Record visibility decisions in code comments near each method group (`navigation`, `input`, `display`) so API intent remains explicit during future cleanup.

### Step 2: Add Deprecation Warning to `newest()` Fallback

In `input_target_webview()`, log a warning when `preferred_input_webview_id` returns `None` and the function falls back to `newest()`:

```rust
fn input_target_webview(&self) -> Option<WebView> {
    let window = self.window();
    let preferred_id = window.platform_window().preferred_input_webview_id(&window);
    let webview_id = if let Some(id) = preferred_id {
        id
    } else {
        log::warn!("input_target_webview: preferred_input returned None, falling back to newest()");
        window.webview_collection.borrow().newest().map(|wv| wv.id())?
    };
    window.webview_by_id(webview_id)
}
```

Makes fallback usage auditable without breaking single-window hosts.

#### Addendum B: Warning Rate Limit Policy

The fallback warning must be rate-limited to avoid log spam under steady input loops.

Recommended policy:

1. Emit at most once per process per minute for identical fallback reason.
2. Store the `last_warning_time` in the `App` struct (wrapped in a `Cell` or `RefCell`) to keep state instance-scoped rather than static.
3. Include a monotonic counter for suppressed repeats.
4. Keep log level at `warn`.
5. Keep implementation local to `egl/app.rs` (no shared global logging infrastructure).

### Step 3: Document `WebView::stop()` Gap

Add a comment in `egl/app.rs` documenting the gap and the upstream path if needed:

```rust
pub fn stop(&self) {
    // Hard gap: Servo's public WebView API does not expose stop_loading().
    // The DOM has Window::stop_loading() (components/script/dom/window.rs)
    // but no EmbedderToConstellationMessage::StopLoading variant exists.
    // If this becomes a user-visible issue, file upstream with a minimal
    // additive proposal: WebView::stop() sending StopLoading(self.id()).
    self.spin_event_loop();
}
```

### Step 4: Add Tests

Two focused tests:

1. **EGL explicit-id routing**: call `load_uri_for_webview` with a specific `WebViewId`, verify the correct webview receives the load (no fallback triggered, no warning logged).
2. **Desktop `GetFocusedWebView`**: verify that `preferred_input_webview_id` returns the tile-focused webview on desktop, not the window-global active.

### Exit Criteria

1. 21 `App` methods have explicit-id overloads, and the 1 platform-window state hook is handled explicitly.
2. Existing public method signatures are unchanged (host compatibility).
3. `newest()` fallback is isolated with a deprecation warning and rate limit.
4. `WebView::stop()` gap is documented.
5. Two tests pass covering explicit routing and desktop focus semantics.
6. `_for_webview` visibility (`pub` vs `pub(crate)`) is documented and intentional per method group.

### Implementation Recommendations (Added 2026-02-17)

1. **Strict Visibility**: Be strict about `pub(crate)` for internal helpers. Only expose `pub` for methods that the Host (Java/C++ layer) physically needs to call.
2. **Instance-Scoped State**: For rate limiting, prefer storing state in the `App` struct over global statics to support potential future multi-window/multi-app scenarios cleanly.

---

## Multiprocess Implications

This plan aligns with Servo's multiprocess architecture. The `WebViewId` is the fundamental cross-process handle used by the Constellation to route messages to the correct content process.

- **Eliminates UI-layer races:** In a multiprocess environment, relying on "active" or "newest" state in the main process is race-prone during rapid window creation. Explicit targeting ensures commands generated for a specific browsing context reach that context regardless of UI focus changes.
- **Matches IPC reality:** The underlying `EmbedderToConstellationMessage` IPC protocol already requires `WebViewId`. This plan removes the mismatch between the ambiguous EGL UI layer and the precise IPC layer.

## Upstream Escalation Gate

Retained from the original plan but with updated assessment:

**Current assessment: Phase 4 will not be entered.** There are no hard gaps that block F6 work. The only candidate (`WebView::stop()`) is a no-op stub with low user impact.

If a hard gap is identified during implementation, required evidence before opening an upstream issue:

1. Reproduction case and failing test.
2. Why local compatibility-layer workaround is insufficient.
3. Minimal additive API proposal (no broad redesign).
4. Backward-compatibility story and default behavior preservation.
5. Prototype impact if deferred.

If evidence is insufficient, do not open upstream request.

---

## Open Questions (Answered)

**Q1: Should fallback-to-newest be allowed at all outside bootstrap/session-init paths?**
A: No, with one exception. The EGL embedder genuinely only supports one window today, and existing OHOS hosts expect the fallback. Keep `newest()` fallback in `input_target_webview()` with a deprecation warning. Remove it when multi-webview EGL ships (at which point callers must use `_for_webview` variants).

**Q2: For `GetFocusedWebView`, do we keep current preferred-id semantics or enforce stricter explicit-target contract?**
A: Keep current semantics. `GetFocusedWebView` returning `preferred_input_webview_id` is correct per WebDriver spec — "focused" means "the one that receives input." The desktop override already returns the tile-focused webview. The EGL default returns `active_id()`, which is correct for single-window.

**Q3: Is Phase 3 test depth sufficient without adding a full event-loop/window harness now?**
A: Two focused unit tests are sufficient for the prototype. A full event-loop harness would be overengineering for the current scope. If multi-webview EGL ships later, expand test coverage then.

**Q4: What is the minimum threshold for opening an upstream issue?**
A: A failing test that demonstrates a user-visible behavior gap which cannot be worked around locally. `WebView::stop()` does not meet this bar because the no-op is not user-visible in practice.

## Risk Register

1. Risk: accidental behavior drift for existing EGL hosts.
   - Mitigation: existing public method signatures preserved as wrappers. New `_for_webview` variants are additive.

2. Risk: hidden reliance on implicit active state in edge paths.
   - Mitigation: deprecation warning in `input_target_webview()` fallback + tests.

3. Risk: over-escalating upstream asks for prototype needs.
   - Mitigation: audit confirms no hard gaps requiring upstream changes. Escalation gate retained but assessed as unlikely to trigger.

## Critique Checklist

Use this checklist during review:

1. Scope clarity:
   - Does the plan stay within graphshell-owned boundaries before escalating upstream? **Yes — audit confirms all targeting code is in graphshell-owned forks.**

2. Compatibility:
   - Are existing servoshell/EGL host entrypoints preserved by wrappers? **Yes — existing method signatures unchanged, new `_for_webview` variants are additive.**

3. Determinism:
   - Are all target resolutions explicit or centralized in a single fallback helper? **Yes — `input_target_webview()` is already the single resolution point. Explicit-id overloads bypass it entirely.**

4. Evidence quality:
   - Are proposed upstream asks tied to reproducible hard gaps, not preference? **No upstream asks proposed. Only hard gap (`WebView::stop()`) does not meet the evidence bar.**

5. Test adequacy:
   - Do tests cover multi-webview routing, focus handoff, and fallback boundaries? **Two focused tests cover explicit routing and desktop focus semantics. Sufficient for prototype scope.**

6. No-legacy policy alignment:
   - Are we avoiding parallel authority systems and unnecessary compatibility branches? **Yes — existing wrappers delegate to explicit-id variants. No parallel authority introduced.**

## Deliverables

1. Explicit-id overloads for 21 EGL `App` methods in `ports/graphshell/egl/app.rs`, plus explicit handling for the 1 platform-window state hook.
2. Deprecation warning in `input_target_webview()` fallback path with rate limiting.
3. Documentation of `WebView::stop()` gap.
4. Two tests covering explicit EGL routing and desktop `GetFocusedWebView` semantics.
5. This targeting audit document (complete).

## Progress

- 2026-02-18: Initial draft created from repo-wide assessment.
- 2026-02-18: Full code audit completed. Audit findings: 37 implicit-targeting instances across graphshell and servoshell; all graphshell instances are in graphshell-owned forks; no upstream changes needed; WebDriver already explicit-id for commands; problem concentrated in `input_target_webview()` single resolution point. Plan revised from four phases to single implementation pass.
- 2026-02-18: Step 1 implemented in `egl/app.rs` with explicit `_for_webview` overloads and compatibility wrappers.
- 2026-02-18: Step 2 implemented with centralized, rate-limited fallback warning in input target resolution.
- 2026-02-18: Step 3 implemented by updating `stop()` hard-gap documentation in code.
- 2026-02-18: Step 4 partially implemented with focused unit tests for EGL target-resolution helper behavior and desktop focused-webview semantics.
- 2026-02-18: Follow-on structural plan created for full single-window/single-active obviation: `2026-02-18_single_window_active_obviation_plan.md`.
- Next: optional follow-up tests for full end-to-end EGL wrapper dispatch under host-driven integration harness.

---

## Appendix: Original Pre-Audit Plan (retained for analysis context)

The plan below was the initial draft before the full code audit was completed. The audit findings (above) led to collapsing the four-phase structure into a single implementation pass. Retained here for traceability.

### Original Phase 0: Targeting Inventory and Invariants

Work:

1. Enumerate every EGL/WebDriver action that currently relies on implicit target selection.
2. Classify each action as:
   - `explicit-ready` (already takes `WebViewId`),
   - `wrapper-needed` (graphshell-owned call can pass id),
   - `hard-gap` (cannot pass target due to missing API surface).
3. Add an invariant list to this doc and keep it updated during implementation.

Exit criteria:

1. Full command inventory exists with file/function references.
2. Each item has an assigned migration strategy.

### Original Phase 1: EGL Explicit-Target Refactor (No Upstream)

Work:

1. Introduce explicit-target variants for EGL app actions (`load`, `reload`, `back`, `forward`, input, resize where applicable).
2. Keep existing public methods as compatibility wrappers that resolve target then delegate to explicit variants.
3. Move any `newest()` fallback into a single compatibility function with a clear comment and telemetry/log hook.
4. Ensure stop-loading behavior remains clearly documented as API-limited (`WebView::stop` unavailable).

Exit criteria:

1. Core EGL command flow no longer performs ad hoc target resolution across multiple methods.
2. Fallback usage is centralized and auditable.
3. Existing hosts compile unchanged.

### Original Phase 2: WebDriver Targeting Tightening (No Upstream)

Work:

1. Ensure all WebDriver command handlers use supplied `WebViewId` when available.
2. For commands that semantically query focus/current context, prefer explicit focused-window + preferred-id semantics, with no `newest()` fallback except where protocol/bootstrap requires it.
3. Isolate unavoidable fallback in a single helper and document protocol rationale.
4. Add tests for new-window, focus, and navigation to prove deterministic target routing.

Exit criteria:

1. WebDriver command routing is explicit-id first and deterministic.
2. Remaining fallback use is protocol-justified and minimal.

### Original Phase 3: Validation Harness and Failure Evidence

Work:

1. Add focused tests for:
   - two-webview routing correctness,
   - no accidental command leakage to non-target webview,
   - close/reopen target consistency,
   - new-window + focus handoff behavior.
2. Add optional debug logging for target resolution decisions in EGL/WebDriver paths.
3. Run matrix on desktop + EGL modes where available.

Exit criteria:

1. Test evidence demonstrates explicit routing correctness.
2. Any residual failure is reproducible and linked to a concrete API limitation.

### Original Phase 4: Upstream Escalation Gate (Only If Needed)

Only enter this phase if Phases 1-3 leave unresolved `hard-gap` items.

Required evidence for each proposed upstream change:

1. Reproduction case and failing test.
2. Why local compatibility-layer workaround is insufficient.
3. Minimal additive API proposal (no broad redesign).
4. Backward-compatibility story and default behavior preservation.
5. Prototype impact if deferred.

If evidence is insufficient, do not open upstream request.

### Original Targeting Audit Table (Initial)

| Area                                                         | Current Pattern                                              | Classification       | Planned Action                                                                  |
| ------------------------------------------------------------ | ------------------------------------------------------------ | -------------------- | ------------------------------------------------------------------------------- |
| EGL navigation/input methods                                 | `input_target_webview()` + preferred/newest fallback         | wrapper-needed       | Add explicit-target variants; centralize fallback                               |
| Platform default target hook                                 | `preferred_input_webview_id` defaults to active-id           | compatibility baseline | Keep default; override/use explicit where graphshell path owns target           |
| WebDriver `NewWindow` selection                              | preferred/newest fallback path                               | wrapper-needed       | isolate fallback + protocol note + tests                                        |
| WebDriver explicit commands (`LoadUrl`, `Refresh`, `GoBack`) | already id-based                                             | explicit-ready       | keep as-is; verify invariants                                                   |
| Stop loading                                                 | no `WebView::stop()`                                         | hard-gap             | document; upstream only with strong evidence                                    |

### Original Open Questions

1. Should fallback-to-newest be allowed at all outside bootstrap/session-init paths?
2. For `GetFocusedWebView`, do we keep current preferred-id semantics or enforce stricter explicit-target contract in graphshell mode?
3. Is Phase 3 test depth sufficient without adding a full event-loop/window harness now?
4. What is the minimum threshold (number/severity of hard gaps) that justifies opening an upstream issue?
