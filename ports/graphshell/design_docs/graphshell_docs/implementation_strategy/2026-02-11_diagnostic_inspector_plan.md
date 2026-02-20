# Diagnostic Inspector Plan (2026-02-11)

**Architecture update (2026-02-20):**
This plan predates the frame execution order architecture, `EdgePayload` traversal model,
and settings plan. The following table maps old concepts to current architecture:

| Old concept | Current equivalent |
| --- | --- |
| "Diagnostic graph" node/edge types | `DiagnosticGraph` — separate ephemeral struct; NOT `GraphBrowserApp` browsing graph |
| "Toggle to switch to diagnostic view" | Third exclusive view mode alongside `is_graph_view`; frame execution step 8 gains a third branch |
| Diagnostic events in main loop | `crossbeam_channel` + ring buffer, tick-rate-limited (10 Hz render, 60 Hz egui unchanged) |
| Phase 4 export | Routes through settings architecture plan export section (no new export path) |
| Any persistence | **None** — diagnostic data is ephemeral, session-only; not written to fjall or redb |

**Feature flag:** All diagnostic inspector code should be gated behind
`cfg(feature = "diagnostics")` to keep it out of release builds. No production cost.

---

## Diagnostic Inspector Plan

- Goal: Visualize Servo internals (threads, IPC channels, message counts, latency) as a
  live diagnostic graph for debugging and performance profiling.
- Scope: `tracing` span instrumentation, `DiagnosticGraph` data structure, third view
  mode in egui frame loop, tick-rate-limited render pipeline.
- Dependencies: `tracing` crate, `crossbeam_channel`, egui_graphs (reused with custom
  node/edge style), settings architecture plan (export), `cfg(feature = "diagnostics")`.
- Phase 1: Instrumentation
  - Add `tracing` spans at thread and IPC boundaries in Servo compositor, layout, and
    script pipelines.
  - Define `DiagnosticEvent` enum: `SpanEnter`, `SpanExit`, `MessageSent { channel_id,
    byte_len }`, `MessageReceived { channel_id, latency_us }`.
  - Produce events via `crossbeam_channel::Sender<DiagnosticEvent>`; consumer lives in
    the diagnostic aggregator (Phase 2). No impact on main render loop.
- Phase 2: Data pipeline
  - `DiagnosticGraph` struct holds thread nodes and channel edges. Completely separate
    from `GraphBrowserApp`'s `StableGraph<Node, EdgePayload>` — no `AddNode`/`AddEdge`
    intents, no fjall/redb persistence, no tags.
  - Ring buffer (bounded capacity, e.g., 512 events) drains at 10 Hz tick into
    `DiagnosticGraph` aggregated state: edge weights = recent message count, edge label =
    p95 latency. Old events dropped; no memory growth.
  - Thread topology (nodes) is mostly static (Servo thread pool is fixed at startup);
    channel edges are dynamic.
- Phase 3: UI mode
  - Add `DiagnosticView` as a third exclusive view mode alongside `is_graph_view` and
    detail view. In `gui.rs` frame execution step 8 (view rendering), the check becomes:
    `if is_diagnostic_view { render_diagnostic() } else if is_graph_view { ... } else { ... }`.
  - Keyboard shortcut: `F12` or `Ctrl+Shift+D` to toggle diagnostic view.
  - Render with egui_graphs, but with custom node/edge style: thread nodes as boxes,
    channel edges with weight thickness and latency label; bottleneck edges highlighted
    in red (latency > threshold).
  - Diagnostic view is gated: `cfg(feature = "diagnostics")` — release builds show no
    toggle and the shortcut does nothing.
- Phase 4: Export
  - "Export diagnostic snapshot" action in the settings panel (cross-ref settings
    architecture plan export section) — no new export path needed.
  - Formats: SVG (egui_graphs render to SVG) and JSON (serialize `DiagnosticGraph`
    state at point-in-time).

## Validation Tests

- Events appear in the ring buffer during a typical browsing session (navigate to a URL,
  at least N span events collected within 1 s).
- Diagnostic render tick runs at ≤10 Hz; main egui FPS does not degrade (measured via
  `egui::Context::frame_nr()` delta).
- `DiagnosticGraph` struct does not appear in fjall or redb after restart (ephemeral).
- In release builds (`cfg(feature = "diagnostics")` absent), `F12` does nothing;
  no diagnostic code paths reachable.
- Export produces valid JSON; nodes and edges in JSON match current `DiagnosticGraph`
  in-memory state.

## Outputs

- `DiagnosticGraph` struct and `DiagnosticEvent` enum (feature-gated).
- Diagnostic view mode integration in `gui.rs` frame loop.
- Export action in settings panel.

## Findings

- (See architecture update note at top of file.)
- `DiagnosticGraph` is intentionally separate from the browsing graph. Mixing them would
  pollute the user's graph with internal implementation details and break tag semantics,
  omnibar search, persistence, and physics.
- The 10 Hz diagnostic render tick is sufficient for latency/throughput visualization and
  avoids cache invalidation pressure on the main 60 Hz egui render.
- Servo's thread topology is semi-static: compositor, layout, and script threads are
  present from startup. Dynamic channels (IPC pipes per new pipeline/frame) add edges at
  runtime. The ring buffer handles the dynamic case gracefully.

## Progress

- 2026-02-11: Plan created.
- 2026-02-20: Aligned with frame execution order architecture (third view mode, step 8
  branch), `DiagnosticGraph` isolation from `GraphBrowserApp` browsing graph, ring-buffer
  with tick-rate-limited pipeline, `cfg(feature = "diagnostics")` feature flag, and
  settings plan export routing.
