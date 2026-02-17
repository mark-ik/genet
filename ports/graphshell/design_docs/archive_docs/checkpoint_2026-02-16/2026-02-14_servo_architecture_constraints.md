# Servo Architecture Constraints for Graphshell (Validated Analysis, 2026-02-14)

## Disclaimer

This document is **research**, not a committed architecture decision. It intentionally includes only source-validated findings. If something cannot be validated from current source, it is recorded as a **research question**.

---

## Scope and Method

This analysis was validated against current local source in:

- `components/servo/servo.rs`
- `components/servo/webview.rs`
- `components/servo/webview_delegate.rs`
- `components/constellation/constellation.rs`
- `components/shared/embedder/lib.rs`
- `ports/graphshell/running_app_state.rs`
- `ports/graphshell/window.rs`
- `ports/graphshell/desktop/gui.rs`
- `ports/graphshell/desktop/webview_controller.rs`
- `ports/graphshell/desktop/app.rs`

---

## Validated Findings

### 1) Where delegate callbacks are dispatched

- `WebViewDelegate` callbacks are dispatched from Servo's embedder-message handling path during `Servo::spin_event_loop()`.
- Evidence chain:
  - `Servo::spin_event_loop()` -> `ServoInner::spin_event_loop()` (`components/servo/servo.rs`).
  - `ServoInner::spin_event_loop()` drains `embedder_receiver` and calls `handle_embedder_message(...)`.
  - `handle_embedder_message(...)` invokes `WebView` mutators and delegate callbacks.

**Constraint for Graphshell:** callback effects are part of the same event-loop pump that Graphshell drives via `RunningAppState::spin_event_loop()` (`ports/graphshell/running_app_state.rs`).

### 2) Current Graphshell delegate implementation gaps

In `ports/graphshell/running_app_state.rs`:

- Implemented:
  - `request_create_new`
  - `notify_page_title_changed`
  - `notify_history_changed` (currently only `set_needs_update()`)
  - `notify_load_status_changed`
  - `notify_new_frame_ready`
  - `notify_favicon_changed`
- Not implemented:
  - `notify_url_changed` override (trait default no-op remains in effect)

**Constraint for Graphshell:** same-tab navigation semantics are not currently driven through `notify_url_changed`.

### 3) URL/history signals available from Servo

- `WebView::set_history(...)` in Servo updates history and then calls:
  - `delegate.notify_url_changed(...)`
  - `delegate.notify_history_changed(...)`
  (`components/servo/webview.rs`).

**Constraint for Graphshell:** Servo provides explicit URL + history callbacks; polling `webview.url()` is not the only mechanism.

### 4) New tab creation path in Servo + Graphshell

- Servo embedder message `EmbedderMsg::AllowOpeningWebView` triggers `webview.request_create_new(...)` (`components/servo/servo.rs`).
- Graphshell handles `request_create_new(...)` by building a new `WebView`, adding it to window collection, and activating it (unless webdriver mode) (`ports/graphshell/running_app_state.rs`).

**Constraint for Graphshell:** new-webview creation is currently wired at delegate level, but graph node creation is still not in that callback path.

### 5) Current graph mutation path is still polling-based

- `webview_controller::sync_to_graph(...)` inspects `window.webviews()` every update (`ports/graphshell/desktop/webview_controller.rs`).
- On URL mismatch, it explicitly creates a **new node** and an edge (code comment: "Always create a NEW node for the new URL").

**Constraint for Graphshell:** current behavior models same-tab URL changes as structural graph growth.

### 6) Event loop ordering in Graphshell runtime

Validated from `ports/graphshell/desktop/app.rs` and `ports/graphshell/running_app_state.rs`:

- Winit event -> headed window handlers -> then `pump_servo_event_loop(...)`.
- `RunningAppState::spin_event_loop(...)` does, in order:
  1. window interface commands
  2. webdriver handling
  3. `servo.spin_event_loop()`
  4. `window.update_and_request_repaint_if_necessary(...)`

**Constraint for Graphshell:** UI commands and Servo callbacks are already serialized by the app's event-loop pump; mutation discipline is an architecture choice, not an inferred thread-separation requirement.

### 7) Servo embedder messaging primitives

Validated from `components/shared/embedder/lib.rs` and `components/servo/servo.rs`:

- Embedder-facing messages include `ChangePageTitle`, `HistoryChanged`, `NotifyLoadStatusChanged`, `NewFavicon`, `AllowOpeningWebView`, `WebViewClosed`, etc.
- Servo creates embedder channels with `crossbeam_channel::unbounded()` in `create_embedder_channel(...)`.

**Constraint for Graphshell:** message transport already exists and is wake-integrated via `EventLoopWaker`; Graphshell can consume signal-driven events without inventing new transport primitives.

### 8) Constellation deadlock guidance exists and is explicit

Validated from `components/constellation/constellation.rs` docs:

- Declares can-block-on relation and warns about IPC send blocking.
- Recommends routing IPC receivers through router threads when non-blocking behavior is required.

**Constraint for Graphshell:** avoid introducing blocking IPC patterns in Graphshell integration paths; prefer existing generic/crossbeam mechanisms already used by embedder-facing code.

---

## Repudiated (Removed) Claims

The following prior claims are not retained because they are not supported as stated:

1. "WebViewDelegate callbacks run on compositor thread."
- Not validated from current dispatch path. Dispatch is through `Servo::spin_event_loop()` processing.

2. "Graphshell must introduce a cross-thread channel for delegate->GUI correctness."
- Not strictly validated as a correctness requirement from current call graph.
- A reducer/intent queue may still be desirable for determinism and conflict resolution.

3. "Pipeline creation has fixed latency characteristics (e.g., hundreds of ms) and therefore requires pending-node state."
- No local benchmark evidence in current source audit.

---

## Architecture Implications for Graphshell (Validated)

### A. Navigation semantics should be event-driven, not URL-poll inferred

Given validated callback availability (`notify_url_changed`, `notify_history_changed`, `request_create_new`) and current polling misbehavior in `sync_to_graph`:

- Same-tab navigation should update node URL/history in place.
- New-tab requests should create new nodes/edges explicitly.
- Back/forward should update traversal state/history, not create nodes.

### B. Keep mutation ordering explicit and deterministic

Because Graphshell currently has multiple mutation sources (toolbar commands, tile interactions, graph interactions, Servo delegate updates), it should keep or adopt a single apply boundary (intent/reducer style).

This is justified by determinism and conflict resolution needs, not by an unverified callback-thread claim.

### C. Keep authority boundaries explicit

Validated runtime structure supports separation:

- Graph semantic model (node identity/lifecycle/edges)
- Tile/workspace presentation model (pane/tab arrangement)
- Runtime webview instances (Servo `WebView` handles + rendering contexts)

This boundary reduces accidental coupling between presentation edits and semantic graph mutations.

### D. Preserve Servo integration minimalism

Current Graphshell already leverages stable embedder APIs (`WebViewDelegate`, `EmbedderMsg`, `WebViewBuilder`) and `ServoShellWindow` abstraction.

Prefer extending Graphshell-side handling over patching Servo core unless there is a proven API gap.

---

## What Went Wrong (2026-02-15)

The navigation fixes attempted so far did not resolve the behavior because they did not replace polling-driven node creation with delegate-driven updates. `notify_url_changed` remained unused as the authority for same-tab navigation, so `sync_to_graph` continued to interpret URL changes as new nodes. The conclusion is that the delegate callback path must become primary, and polling should be reduced to cleanup only. See NAVIGATION_NEXT_STEPS_OPTIONS.md for the options.

---

## Research Questions (Unvalidated / Needs Measurement)

1. What is measured `request_create_new` -> first usable paint latency across representative pages/hardware for Graphshell's headed mode?

2. Under what workload does delegate-driven event handling require additional buffering/queueing beyond existing event-loop serialization?

3. What ordering guarantees (if any) are observed between `notify_url_changed`, `notify_page_title_changed`, and `notify_history_changed` for complex navigations (redirects, SPA transitions, history traversal)?

4. Do any Graphshell-specific callbacks currently perform heavy work that could regress input/paint latency within a single event-loop pump?

5. What is the minimal migration path to replace URL-polling structural mutation in `sync_to_graph` while preserving existing tile remap cleanup behavior?

---

## Immediate, Source-Backed Next Steps

1. Implement `notify_url_changed` in `ports/graphshell/running_app_state.rs` and route it to Graphshell graph-update logic.
2. Extend `notify_history_changed` usage beyond UI invalidation.
3. Remove URL-change node-creation logic from `ports/graphshell/desktop/webview_controller.rs:sync_to_graph`.
4. Keep stale mapping cleanup and tile remap cleanup, but separate them from semantic node-creation decisions.

---

## Status

Research only. This file intentionally avoids unvalidated assertions and should be read as a validated constraints baseline for follow-on architecture decisions.
