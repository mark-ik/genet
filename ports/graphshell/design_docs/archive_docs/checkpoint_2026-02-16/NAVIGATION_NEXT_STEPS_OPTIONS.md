# Navigation Next Steps: Three Options

Date: 2026-02-15
Status: proposal

This document captures three concrete paths forward for navigation/control-plane work.
Each option has a clear purpose, what it changes, what it does not change, and how to judge success.

---

## Problem Summary

Navigation behavior is inconsistent with the intended model. Same-tab navigation is supposed to update the current node, while new-tab actions should create new nodes and edges. Today, URL polling and window-global targeting can create nodes at the wrong time or target the wrong webview. Delegate callbacks like notify_url_changed are available but not used as the primary driver, so mutations occur implicitly and can diverge across graph, tile, and webview layers.

---

## Mistakes in Prior Fix Attempts

- Overfocused on wrong-target dispatch and treated it as proven before the code changes validated the hypothesis.
- Spent time in a diagnostics loop (logging and reruns) instead of moving sooner to the event-driven callback model.
- Applied a narrow patch (direct webview targeting) that did not address the deeper semantic mismatch between polling and delegate-driven navigation.
- Delayed the shift toward notify_url_changed and intent-based mutation even though the design docs pointed there.
- Underweighted the tile and lifecycle interactions that can invalidate isolated fixes.

---

## Guardrails to Avoid Repeating Mistakes

- Read and restate the relevant design docs before proposing causal claims or fixes.
- Treat hypotheses as unproven until a minimal patch validates them, and say so explicitly.
- Prefer event-driven Servo callbacks over polling when the model says they are the authority.
- Timebox diagnostics: one round of logging, then move to a testable change.
- Check tile/lifecycle interactions before concluding a single-path fix is sufficient.

## Option 1: Write a Doc-First Fix Plan (No Code Yet)

### Deal
Produce a design-grade plan that reconciles current behavior with the intended model in GRAPHSHELL_AS_BROWSER.
It is a fast alignment pass that clarifies intent boundaries and updates the roadmap without touching runtime code.

### What it changes
- Creates a single, shared explanation of navigation semantics and the control plane.
- Defines the exact event sources (Servo delegate callbacks, UI actions, tile actions) and how they should map to graph mutations.
- States the minimal correctness rules (same-tab navigation updates node URL, new-tab creates node/edge, back/forward updates history only).
- Identifies the current code paths that violate the model.

### What it does not change
- No runtime behavior changes.
- No fixes to omnibar or tab navigation.
- No new tests.

### Why choose this
- You want clear agreement on the model before refactoring.
- You need a crisp target spec to prevent another loop of ad-hoc fixes.

### Risks
- Does not unblock the bug by itself.
- Can feel like delay if you need working behavior now.

### Success criteria
- A short, stable spec that the next options can implement directly.

---

## Option 2: Minimal Callback Wiring (Targeted Fix, Low Churn)

### Deal
Implement the smallest set of changes that align runtime behavior with Servo event-driven navigation.
This removes URL polling as a source of structural graph mutations and wires the missing delegate callback.

### What it changes
- Implements notify_url_changed and uses it as the authority for same-tab navigation updates.
- Stops sync_to_graph from creating a new node on every URL change.
- Keeps existing tile and lifecycle mechanics intact.
- Adds focused tests or debug assertions for the new event flow.

### What it does not change
- No intent system or policy layer yet.
- No deep refactor of tile/graph/webview authority boundaries.
- No rewrite of the host runtime.

### Why choose this
- You want an immediate behavioral correction with minimal architectural churn.
- You want to stop the most obvious semantic violation (polling-driven node creation) first.

### Risks
- Leaves multi-source mutation ordering implicit.
- Future features (tile mutations, multi-authority decisions) may still need a stronger intent boundary.

### Success criteria
- Same-tab navigation updates the active node URL without creating a new node.
- New-tab creation still produces a new node and edge.
- Omnibar navigation behaves predictably and does not spawn phantom nodes.

---

## Option 3: Control-Plane Refactor (Intent Boundary + Authority Split)

### Deal
Adopt the intended architecture: intent-based mutation, explicit authority boundaries (graph semantics vs tile presentation vs webview runtime), and event-driven Servo signals. This is the cleanest long-term path but is larger in scope.

### What it changes
- Adds typed intents and a single apply boundary per frame.
- Separates semantic graph mutations from presentation-only tile changes.
- Routes Servo delegate callbacks into intents rather than direct graph mutations.
- Replaces URL polling with event-driven flow across the board.
- Clarifies duplicate URL handling by moving toward stable node identity.

### What it does not change
- Does not require a full host rewrite up front.
- Can keep existing rendering and lifecycle mechanisms while the control plane is refactored.

### Why choose this
- You want to prevent future regressions caused by implicit mutation ordering.
- You want a consistent model for graph, tile, and webview interactions.
- You want to align with the design docs and end the patch loop.

### Risks
- Larger diff and higher short-term cost.
- Requires careful staging to avoid destabilizing unrelated features.

### Success criteria
- All navigation changes originate from explicit intents.
- The graph, tiles, and webview set are consistent after each frame apply.
- Bug class: wrong-target dispatch, polling-driven node creation, and cross-layer races are eliminated by design.

---

## Decision Notes

If you need a fast correction, pick Option 2. If you want durable alignment with the design docs, pick Option 3. Option 1 is the planning step if you want written consensus before changing behavior.
