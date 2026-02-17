# Graph-Tile Parity Research Note (2026-02-14)

## Disclaimer

This document is **research and analysis**, not an adopted architecture decision. It captures skeptical critique of recent design discussions around graph/tile parity and proposes implications to evaluate. Treat this as input for design review, not as normative spec.

## Research Focus

Question: Should Graphshell enforce parity between graph structure and tile-tree structure?

Short answer from this analysis: **enforce semantic parity, avoid structural parity**.

## Skeptical Critique

### 1. Structural parity is appealing but usually wrong

A strict 1:1 mapping between graph clusters/edges and pane/tab arrangement looks clean in theory, but creates fragile coupling in practice.

- Graph structure carries browsing semantics (identity, navigation provenance, grouping intent).
- Tile tree structure carries workspace presentation (split ratios, tab order, focus, filtered views).

Forcing these to mirror each other makes UI operations accidentally semantic and semantic operations accidentally visual.

### 2. "Tiles are derived views" is necessary but can be overstated

A strong critique suggested one-way flow where tiles never mutate graph. That reduces accidental coupling, but taken literally it fails to model legitimate user intent from tile interactions.

Some tile actions are purely presentational (reorder, resize, focus). Others can be semantic if explicitly chosen by the user (close tab as close-node, deliberate grouping gesture).

So the practical boundary is not "tiles never mutate graph"; it is "tiles never mutate graph implicitly".

### 3. One-way framing alone does not remove contradiction risk

Even with graph-authoritative data, contradiction still appears if multiple systems mutate state directly:

- Servo callbacks
- Graph view interactions
- Tile interactions
- Keyboard shortcuts
- Restore/recovery paths

The real safeguard is a single mutation boundary (intent reducer/apply stage), not merely directional rhetoric.

### 4. The old wording around source-of-truth is underspecified

Current docs mix statements like:

- webview set is source of truth,
- tile tree is authority for pane membership,
- graph is persistence/semantic model.

These can coexist, but only if scoped as separate authorities. Without that scope, it reads as competing primaries and invites implementation drift.

### 5. P2P implications favor semantic/model authority over layout authority

For collaboration, shared graph semantics and local workspace layout are a strong default:

- Shared: node identity/history/edges/intents.
- Local: pane layout, tab ordering, filtered projections.

If layout is tightly coupled to graph topology, collaboration conflicts increase with little semantic value.

## Improved Architecture Implications

### A. Use three explicit stores/authorities

1. `GraphStore` (semantic authority)
- Node identity (UUID)
- Node lifecycle state
- Edge semantics/provenance
- Per-node navigation/history metadata

2. `WorkspaceStore` (presentation authority)
- Pane hierarchy/splits
- Tab order and active tab per pane
- View filters/sorts/projection settings
- Local user preferences

3. `RuntimeStore` (ephemeral authority)
- Live webview instances
- Rendering contexts
- Texture caches and runtime-only resources

### B. Classify operations by intent type

1. Semantic operations
- Create node/tab
- Close/delete node
- URL/title/history semantic updates
- Create semantic edges (`Hyperlink`, `History`, `UserGrouped`)

2. Presentation operations
- Reorder tab
- Move tab across panes (presentation-only by default)
- Resize/split panes
- Focus/visibility/filter changes

3. Runtime operations
- Instantiate/suspend/destroy webview instances
- Rebind rendering context

Cross-store mutation should happen only at reducer/apply stage.

### C. Make semantic mutation explicit

Tile interactions should not create semantic graph changes implicitly.

Example policy:

- Drag tab to pane: presentation-only by default.
- "Group with" command/gesture: explicit semantic operation creating `UserGrouped` edge.

### D. Keep semantic parity invariants, allow structural divergence

Required invariants:

- Every tile node reference must exist in graph.
- Graph node may exist without tile.
- URL change in same tab must not create a new node.
- New-tab action creates exactly one new node with stable UUID.

Allowed divergence:

- Same graph rendered in different pane layouts.
- Filtered/temporary workspace projections.
- Local layout differences across peers.

### E. Persist both graph and workspace

Persisting both is preferable to over-optimizing derivation:

- Graph persistence preserves semantics/history.
- Workspace persistence preserves UX continuity.
- On restore: validate workspace references against graph and prune stale entries.

## Contradictions This Research Intends to Resolve

1. "Single source of truth" ambiguity across docs.
2. URL identity remnants vs UUID direction.
3. Same-tab URL change semantics vs polling-driven node creation behavior.
4. Lifecycle naming drift (`Cold` vs `Inactive`).
5. Implicit assumption that pane cluster topology must track edge topology.

## Proposed Documentation Direction (for follow-up review)

1. Update wording to "semantic authority vs presentation authority vs runtime authority".
2. Document explicit operation classes (semantic/presentation/runtime).
3. State that structural parity is not a goal.
4. Keep intent-based mutation as the only write path.
5. Preserve optional future shared-workspace capability without coupling it to core graph sync.

## Status

Research only. No architecture decision is finalized by this file.
