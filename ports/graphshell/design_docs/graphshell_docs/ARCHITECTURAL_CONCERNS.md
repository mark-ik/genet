# Architectural Concerns and Contradictions

**Last Updated**: February 17, 2026
**Purpose**: This document summarizes key architectural contradictions, gaps, and unresolved questions identified from a review of the design documentation. It is intended to guide refactoring and ensure architectural consistency.

---

## 1. Contradiction in the "Source of Truth"

There is a foundational ambiguity regarding the primary source of application state.

- **Conflict**: [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) and [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) previously diverged on source-of-truth language.
- **Impact**: This conflict was a source of synchronization bugs between graph representation and webview/tile runtime state. The current strategy resolves this via explicit authority domains.
- **Status (Feb 17)**: Largely reduced in runtime paths. Current architecture and implementation strategy converge on graph/intents as the control-plane source of truth, with webviews treated as effectful runtime state reconciled from graph/lifecycle intents.

---

## 2. Gaps in the Delegate-Driven Navigation Plan

The plan to move to a delegate-driven model is a critical improvement, but reveals further challenges.

- **Identity Migration**: Implemented (UUID identity + URL multi-map model). The former URL-identity concern is no longer a blocker.
- **Bidirectional Flow**: Implemented for current scope. User actions in Graphshell UI route through intent/reconciliation paths and direct per-webview targeting where appropriate (legacy command variants removed except `ReloadAll`).
- **Status (Feb 17)**: Remaining delegate concern is empirical callback ordering nuances under navigation patterns; this is now traced and documented in the architecture plan.

---

## 3. Physics Complexity (Historical Concern)

The former custom physics engine was identified as a source of issues.

- **Over-Engineering**: [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md) notes that the custom, multi-threaded physics worker is "unnecessary for browsing-scale graphs."
- **Known Bugs**: The same document identifies a bug in the force calculation ("doubling effective attraction").
- **Resolution**: Migration to `egui_graphs` force-directed layout is the adopted path and active runtime direction.
- **Status (Feb 17)**: Treat this as historical context plus tuning follow-up, not as a core architectural contradiction.

---

## 4. Intent Boundary Completeness

The desired architecture for managing state is not yet fully implemented.

- **The Ideal**: [GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md) describes a clean "intent-based" model where all state mutations are funneled through a single, predictable processing point.
- **The Historical Gap**: Earlier implementations mixed polling/direct wiring with partial intent flow, increasing fragility.
- **Status (Feb 17)**: Significantly addressed. Lifecycle helper-local apply paths were removed, legacy lifecycle path deleted, and frame boundary comments/tests updated. Residual direct runtime APIs are in effect/reconciliation layers by design.

---

## 5. Duplicated State Management

The documentation explicitly identifies areas of duplicated state.

- **Selection State**: [implementation_strategy/2026-02-14_selection_semantics_plan.md](implementation_strategy/2026-02-14_selection_semantics_plan.md) was created to address the problem of duplicated selection state between different components. This is a known weakness in the current component wiring that can lead to UI inconsistencies and bugs.
- **Status (Feb 17)**: Keep as active concern only to the extent unresolved items remain in the selection semantics plan.
---

## 6. Lacking Unit Test Coverage for Critical Components

The architecture of the UI and webview integration components makes them difficult to test in isolation, which is a potential quality risk.

- **Gap**: The `DEVELOPER_GUIDE.md` notes that `desktop/gui.rs` and `desktop/webview_controller.rs` have no dedicated unit tests and are only covered by integration tests.
- **Impact**: These modules contain the most complex and critical logic for integrating with Servo. A lack of unit tests makes refactoring risky and can lead to regressions. An architecture that is difficult to unit-test often indicates tight coupling between components.
- **Status (Feb 17)**: Stale as written. Both modules now have focused unit coverage (intent conversion/order tests, lifecycle reconciliation/backpressure classifier tests, controller reconciliation tests). Remaining risk is complexity/coverage breadth, not complete absence of unit tests.

---

## 7. Underspecified Crash Handling Strategy

The architectural documents do not specify how the application should behave when a sandboxed web content process crashes.

- **Gap**: A robust browser architecture must be resilient to crashes in content processes. It is unclear from the documents whether such a crash would be gracefully handled (e.g., by displaying a "crashed tab" message) or if it would risk taking down the entire Graphshell application.
- **Impact**: Without a clear strategy, the application's stability is at risk from misbehaving web content.
- **Status (Feb 17)**: Implemented for desktop graphshell paths (semantic event -> reducer demote/unmap -> tile crash banner/actions), as documented in [implementation_strategy/2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md) under "Crash Handling Policy (Specified 2026-02-17)". Remaining limitations are upstream API surface concerns (for example web-content accessibility), not missing crash policy wiring in graphshell desktop.

---

## 8. Monolithic UI Component

The primary UI component remains large and may have too many responsibilities, despite recent refactoring.

- **Concern**: `desktop/gui.rs` has grown to ~2,840 lines (as of Feb 17), well beyond the ~800-1000 lines noted in `DEVELOPER_GUIDE.md` and `CODEBASE_MAP.md`.
- **Impact**: Large, monolithic components are difficult to understand, maintain, and test. This file's size suggests it may be a "god object" for the UI layer, and further decomposition might be necessary to improve the health of the UI architecture.

