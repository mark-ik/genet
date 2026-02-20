# EGL Embedder Extension Plan (2026-02-17, Archived 2026-02-20)

## Status: Deferred

**Rationale for deferral**: The Cross-Platform Sync and Extension Plan (2026-02-20) prioritizes thin mobile clients and browser extensions over native EGL enhancements. Single-window EGL model is adequate for sync client use cases. This plan remains referenceable if Android native support is desired later, particularly for semantic event convergence (Phase 2) and host/vsync contract clarification (Phase 3).

## Scope

- `ports/graphshell/egl/app.rs`
- `ports/graphshell/window.rs`
- `ports/graphshell/running_app_state.rs`
- any EGL host bindings that consume the `App` API

Out of scope for this phase:

- true multi-window EGL runtime,
- webview accessibility bridge (blocked on Servo API surface),
- platform-specific rendering backend rewrites.

## Current State

- EGL is single-window by design.
- EGL navigation commands already target explicit webviews directly (`load_uri`, `reload`, `go_back`, `go_forward`).
- Vsync ownership is platform-host driven, forwarded into `VsyncRefreshDriver`.

## Design Principles

1. Preserve single-window runtime initially; design APIs to become multi-window capable later.
2. Keep semantic mutation in `GraphIntent` application boundary.
3. Keep webview create/destroy in reconciliation/effect layers.
4. Prefer incremental convergence with desktop runtime behavior over large embedder rewrites.

## Phase Plan

### Phase 1: API and State Shape Hardening

1. Introduce explicit window handle abstraction in EGL `App` internals even if only one instance exists.
2. Remove single-window assumptions from helper naming (`window()` style APIs) where practical.
3. Ensure all navigation methods resolve via explicit webview identity, not implicit globals.

Acceptance:

- no new global-targeting command queue paths are introduced,
- EGL still compiles/runs unchanged for one-window hosts.

### Phase 2: Semantic Event Convergence

1. Ensure EGL consumes the same delegate semantic events as desktop (`UrlChanged`, `HistoryChanged`, `TitleChanged`, `CreateNewWebView`, `WebViewCrashed`).
2. Keep event ordering semantics consistent with desktop reducer assumptions.
3. Add EGL-specific tests for semantic event to intent conversion and lifecycle transitions.

Acceptance:

- traversal semantics rely on `history_changed` index/list callbacks,
- crash handling follows the same demote/unmap/reopen model as desktop.

### Phase 3: Host/Vsync Interface Cleanup

1. Define a host-facing contract for vsync delivery cadence and repaint guarantees.
2. Keep current host-driven vsync mechanism unless instrumentation shows correctness/perf issues.
3. If migration is needed, move vsync notification ownership behind a single interface boundary first, then switch implementation.

Acceptance:

- no frame starvation under normal host cadence,
- no duplicate repaint loops introduced by embedder glue.

### Phase 4: Optional Multi-Window Enablement

1. Add window collection semantics in EGL state (`id -> EmbeddedPlatformWindow`) without changing host behavior by default.
2. Gate multi-window via explicit capability/feature flag.
3. Validate focus, input routing, and close semantics per window.

Acceptance:

- single-window hosts remain unaffected,
- multi-window mode does not regress navigation targeting.

## Risks and Mitigations

- **Risk**: divergence between desktop and EGL control-plane behavior.  
  **Mitigation**: share event/intents invariants and reducer tests.

- **Risk**: premature multi-window expansion increases embedder instability.  
  **Mitigation**: defer multi-window until Phase 1-3 are stable.

- **Risk**: vsync ownership churn introduces jitter/regressions.  
  **Mitigation**: keep current mechanism as baseline; migrate only with testable win.

## Test Matrix

1. Navigation: direct load/back/forward/reload target correctness.
2. Delegate ordering: redirect, SPA pushState, hash change, back/forward burst.
3. Lifecycle: node promote/demote with reconciliation.
4. Crash behavior: callback handling, node remains, reopen succeeds.
5. Repaint cadence: host-driven vsync under steady interaction.

## Definition of Done (This Plan)

1. EGL extension work is tracked as phased tasks rather than TODO stubs.
2. No contradiction with architecture-and-navigation control-plane rules.
3. Single-window behavior remains stable while multi-window path is clearly prepared.
