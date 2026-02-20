# Single-Window/Single-Active Obviation Plan (2026-02-18)

## Status: Deferred (Archived 2026-02-20)

**Rationale for deferral**: The Cross-Platform Sync and Extension Plan (2026-02-20) adopts a thin-client mobile architecture rather than native multi-window porting. EGL single-window model remains suitable for sync clients. This plan's audit inventory is preserved as reference material should EGL multi-window become a future requirement.

**Recommendation (from code audit, Feb 18):** This plan should not be executed at this time. The audit found that every remaining single-window/single-active structural assumption lives in the EGL embedded path (`egl/app.rs`), which the desktop graphshell prototype does not use. The desktop path is already clean — `HeadedWindow` overrides input routing, `RunningAppState` supports multiple windows, all delegate callbacks route through `window_for_webview_id()`, and WebDriver commands use explicit IDs after F6.

The refactoring proposed here would make EGL code architecturally purer but would not change any observable behavior or unblock any feature. The trigger for revisiting is one of:

- EGL multi-window ships (requires new rendering infrastructure beyond this refactor).
- A desktop bug is traced to `active_webview_id` semantics (fix the specific bug, not a 5-phase refactor).
- Graphshell is proposed for servo upstream (code cleanliness matters for reviewers).

The audit inventory below is preserved as future-work documentation so the engineer who revisits this doesn't need to redo the analysis.

---

## Purpose

Define the follow-on work required to fully remove inherited single-window/single-active structural authority in graphshell EGL paths, after F6 explicit-target routing.

This plan is local-first, compatibility-preserving, and does not assume upstream Servo changes.

## Scope

- `ports/graphshell/egl/app.rs`
- `ports/graphshell/window.rs`
- `ports/graphshell/running_app_state.rs`
- `ports/graphshell/webdriver.rs`
- targeted tests in graphshell modules

Out of scope for this plan:

- rewriting `ports/servoshell/*`
- mandatory upstream API changes
- full productized multi-window UX for all host apps

---

## Complete Audit Inventory (from code review)

### EGL `egl/app.rs`

| Location | Assumption | Type | Prototype Impact |
| --- | --- | --- | --- |
| `App::window()` (line 366-373) | `.nth(0)` returns single window; called ~30 times | (c) Structural | None — EGL only, single-surface model is correct |
| `EmbeddedPlatformWindow` scalar caches (lines 52-61) | `current_title`, `current_url`, `current_can_go_back/forward`, `current_load_status` — one slot each | (b) UI cache | None — EGL host shows one focused view |
| `input_target_webview_id()` (lines 393-402) | Routes through `self.window()` (single window) | (a) Dispatch | None — desktop uses egui, never calls this |
| `_for_webview` methods (F6 additions) | Lookup via `self.window().webview_by_id()` | (c) Structural | None — correct for single-window EGL model |
| `resize_for_webview()` (line 532-534) | Mutates single `viewport_rect` on platform window | (b) UI cache | None — EGL single-surface |
| `notify_vsync/pause/resume_painting` (lines 868-906) | Operate on single platform window rendering context | (c) Structural | None — EGL lifecycle |
| `EmbeddedPlatformWindow::id()` (line 71) | Returns hardcoded `0.into()` | (c) Structural | None — would need counter for multi-window |

### `window.rs`

| Location | Assumption | Type | Prototype Impact |
| --- | --- | --- | --- |
| `ServoShellWindow` struct | Not inherently single-window — holds one `WebViewCollection` per window | (d) Handled | Clean |
| `repaint_webviews()` (lines 169-183) | Paints only `preferred_input_webview_id` result | (a) Dispatch | None — desktop uses egui paint calls per tile |
| `preferred_input_webview_id` trait default (lines 606-608) | Returns `active_id()` | (d) Handled | Desktop overrides in `HeadedWindow` |
| `WebViewCollection` per-window | Each `ServoShellWindow` owns its own collection | (d) Handled | Clean |

### `running_app_state.rs`

| Location | Assumption | Type | Prototype Impact |
| --- | --- | --- | --- |
| `WebViewCollection::active_webview_id` (line 62) | One active per collection | (a) Dispatch | Benign — egui tracks its own tile focus independently |
| `activate_webview()` (lines 119-127) | Calls `show()` + `focus()` but no `hide()`/`blur()` on old | (a) Dispatch | **Intentional and correct** — multiple tiles visible simultaneously |
| `WebViewCollection::add()` (lines 66-71) | `show()` without auto-activate | (d) Handled | Correct separation of concerns |
| `RunningAppState` struct | Multi-window via `HashMap<ServoShellWindowId, Rc<ServoShellWindow>>` | (d) Handled | Clean |
| `request_create_new()` (lines 701-722) | Child webview placed in parent's window | (c) Structural | Correct for graphshell model |

### `webdriver.rs`

| Location | Assumption | Type | Prototype Impact |
| --- | --- | --- | --- |
| `NewWindow` tab fallback (lines 175-180) | `.nth(0)` first window for new tabs | (c) Structural | None — WebDriver path |
| All other commands | Explicit `webview_id` targeting | (d) Handled | Clean after F6 |

### Classification Key

- **(a) Dispatch authority** — code that routes commands based on single-active/single-window state
- **(b) UI state cache** — per-window scalar fields that assume one focused webview
- **(c) Structural/initialization** — hardcoded single-window topology
- **(d) Already handled** — no single-window assumption, or already overridden

---

## Audit Conclusions

1. **All remaining single-window assumptions are in the EGL path** (`egl/app.rs`), which the desktop graphshell prototype never executes.
2. **The desktop path is already clean.** `HeadedWindow` overrides `preferred_input_webview_id`, `RunningAppState` supports multiple windows, all delegates route through `window_for_webview_id()`.
3. **`activate_webview()` not calling `hide()`/`blur()` on the old webview is intentional** — graphshell's multi-tile model requires multiple visible webviews. This is a feature, not a bug.
4. **`App::window()` is topology, not authority.** EGL has one rendering surface, one OS window, one EGL context. The singleton accessor correctly describes the EGL model. Replacing it with `window_for_webview_id()` adds indirection without changing behavior. Multi-window EGL would require new rendering infrastructure (multiple EGL surfaces, multi-window compositor) far beyond what this refactor covers.
5. **The scalar UI state caches are correct for their purpose.** Host platforms (Android ActionBar, OHOS status bar) show one focused view at a time. A per-webview `HashMap` cache with explicit selection would produce identical host callbacks.
6. **`active_webview_id` is already hint-only for the desktop path.** The plan's Phase 2 goal — "no command path dispatches because a webview is globally active" — is already true for desktop.
7. **No upstream changes needed.** The Servo `WebView` API is handle-based. All targeting code is in graphshell-owned forks.

---

## Original Phased Plan (retained for future reference)

The phases below describe what would be needed if EGL multi-window becomes relevant. They are not scheduled for implementation.

### Phase 0: Authority Inventory and Invariants

**Status: Complete (see audit inventory above).**

### Phase 1: Window Model Refactor in EGL App

1. Replace singleton-style `App::window()` authority usage with explicit selection helpers:
   - `window_for_webview_id(...)`,
   - `focused_window()` for focus-only semantics.
2. Keep `App::window()` only as temporary compatibility shim, mark deprecated internally.
3. Ensure command APIs can target explicit webview without consulting singleton window.

Exit criteria:

1. Command dispatch paths no longer require `App::window()` to identify target.
2. Compatibility wrappers still compile existing host callers unchanged.

### Phase 2: Demote `active_webview_id` to Hint-Only

1. Refactor `WebViewCollection` usage so `active_webview_id` is never command authority in graphshell EGL flow.
2. Keep `active_webview_id` for:
   - UI focus hint,
   - protocol-required focused-context queries.
3. Remove any fallback chain where `active -> newest` is used as dispatch authority outside explicit compatibility boundaries.

Exit criteria:

1. No command path dispatches because a webview is globally active.
2. Remaining active-id reads are documented as hint-only.

### Phase 3: Replace Singular UI State with Per-WebView State

1. Replace active-webview singular fields in `EmbeddedPlatformWindow` with per-webview keyed state cache.
2. Update `update_user_interface_state` to compute host-visible state from selected target webview, not from implicit active authority.
3. Maintain host-facing callbacks compatibility.

Exit criteria:

1. UI state bookkeeping is no longer structurally single-active.
2. Target selection for UI sync is explicit and testable.

### Phase 4: Repaint and Update Path Decoupling

1. Refactor repaint/update logic to avoid structural single-view assumptions:
   - explicit target-based repaint where applicable,
   - clear compatibility policy for single-window hosts.
2. Keep safety fallback only at one boundary helper with rate-limited diagnostics.

Exit criteria:

1. Repaint/update flow has explicit targeting or isolated compatibility boundary.
2. No hidden authority coupling remains in frame/update loop.

### Phase 5: Validation and Regression Guardrails

1. Add tests for:
   - two-webview command routing without active-id dependency,
   - focus switch without command retargeting side effects,
   - close/reopen retaining explicit target semantics,
   - WebDriver focused-context semantics still correct.
2. Add assertions/logging for illegal authority use in debug builds.

Exit criteria:

1. Tests pass and demonstrate authority demotion success.
2. No regressions in existing graphshell lib test suite.

---

## Findings

1. F6 eliminated most dispatch coupling by adding explicit `_for_webview` entrypoints.
2. Remaining structural coupling is concentrated in:
   - singleton window access patterns,
   - singular per-window active-derived UI state fields,
   - active-based default target hooks.
3. This work is mostly graphshell-local because graphshell/servoshell files are forked for EGL paths.
4. Upstream is not required unless a new hard API gap appears (current known low-severity gap: `WebView::stop()`).
5. **All of the above is EGL-only.** The desktop prototype is unaffected.

## Risks and Mitigations

1. Risk: breaking OHOS/Android host assumptions.
   - Mitigation: preserve external method signatures via wrappers; make changes additive first.

2. Risk: introducing parallel authority systems during migration.
   - Mitigation: allow one compatibility boundary only; no duplicate control planes.

3. Risk: over-scoping into full multi-window UX product work.
   - Mitigation: keep this plan architecture-focused; defer UX/product expansion to separate plan.

## Reactivation Triggers

Do not execute this plan unless one of these conditions is met:

1. **EGL multi-window ships** — need per-window state, `window_for_webview_id()` lookup, multi-surface rendering.
2. **A desktop bug is traced to `active_webview_id` semantics** — fix the specific bug, then assess whether broader refactoring is warranted.
3. **Graphshell is proposed for upstream merge** — code reviewers will scrutinize single-window patterns. Pre-merge cleanup is justified.
4. **A new hard API gap blocks a user-visible feature** — follow the F6 escalation gate process.

## Progress

- 2026-02-18: Plan created as post-F6 follow-up scope.
- 2026-02-18: Full code audit completed. All remaining single-window assumptions found to be EGL-only with no desktop prototype impact. Plan deferred with audit inventory preserved.
