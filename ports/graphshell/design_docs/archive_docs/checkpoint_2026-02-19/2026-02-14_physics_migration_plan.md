# Physics Migration Plan (2026-02-14)

## Physics Migration Plan

- Goal: replace custom physics engine/worker wiring with `egui_graphs` force-directed layout in runtime.
- Scope: `app.rs`, `render/mod.rs`, `desktop/gui.rs`, `input/mod.rs`, `lib.rs`, dependency cleanup, and physics/spatial module removal.
- Non-goal: node-identity and Servo signal semantics changes (handled by later tasks).

### Phase 1: Runtime State Migration

1. Remove `PhysicsEngine`/`PhysicsWorker` ownership from `GraphBrowserApp`.
2. Introduce app-owned FR layout state for UI controls and persistence in-memory behavior.
3. Remove worker sync/update call paths from graph mutation flows.

### Phase 2: Render Integration

1. Switch `GraphView` layout type parameters to force-directed types.
2. Bridge app layout state to egui_graphs layout state each frame.
3. Keep keyboard and panel controls (`T`, `P`) working against FR state.
4. Preserve pinned-node behavior by restoring pinned positions after layout step.

### Phase 3: Cleanup

1. Remove `mod physics` and `graph::spatial` module wiring.
2. Delete `physics/mod.rs`, `physics/worker.rs`, `physics/spatial_hash.rs`, and `graph/spatial.rs`.
3. Remove `kiddo` dependency from `Cargo.toml`.
4. Update tests for new physics state model and removed modules.

## Findings

- Current runtime still performs worker-based physics stepping in GUI update and app worker channels.
- `egui_graphs 0.29` exposes `FruchtermanReingoldState` and `LayoutForceDirected<FruchtermanReingold>` suitable for immediate migration.
- `FruchtermanReingoldState` carries the parameters needed by existing physics panel semantics (`is_running`, `damping`, `c_attract`, `c_repulse`, `k_scale`, etc.).

## Progress

- 2026-02-14: Plan created.
- 2026-02-14: Implemented migration: app-owned `FruchtermanReingoldState`, `GraphView` FR layout wiring, pinned-position restoration, worker/spatial module deletion, and `kiddo` dependency removal.
- 2026-02-14: Validation passed with `cargo test -p graphshell --lib` (134 tests).
