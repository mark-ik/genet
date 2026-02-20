# Architecture and Navigation Plan (2026-02-16)

**Last Updated**: 2026-02-20 (edge traversal model alignment)

Consolidated from prior research and plans. Originals archived in `archive_docs/checkpoint_2026-02-16/`.

**Cross-Reference**: Edge semantics updated to align with `2026-02-20_edge_traversal_model_research.md` — edges now accumulate `Vec<Traversal>` records instead of using `EdgeType` enum.

## Decided Model

**Semantic parity, not structural parity.** Three authority domains:

| Domain | Authoritative For | Examples |
| ------ | ----------------- | -------- |
| **Graph** | Node identity (UUID), lifecycle, edge semantics | Add/remove node, URL change, traversal records, user-asserted edges |
| **Tile Tree** | Pane layout, tab order, focus, visibility | Reorder tabs, resize panes, focus pane |
| **Webviews** | Live runtime instances, rendering contexts | Create/destroy webview, bind rendering context |

Key rules:

- Graph nodes may exist without tiles. Tiles must reference existing graph nodes.
- Tile interactions never mutate graph implicitly. Explicit intent required for semantic operations.
- All state mutations go through `GraphIntent` reducer at a single apply boundary per frame.
- Navigation driven by Servo delegate callbacks, not URL polling.

## Comparative Context

Graphshell's architecture sits between servoshell (synchronous, simple) and verso (async, message-based). The two-phase apply model is a deliberate choice to keep servoshell's simplicity while gaining verso's separation of concerns.

| Aspect | Servoshell | Verso (archived Oct 2025) | Firefox/Gecko |
| ------ | ---------- | ------------------------- | ------------- |
| **State mutation** | Immediate (RefCell) | Batched (WebRender transactions) | Immediate in parent, async to children |
| **Side effects** | Synchronous (`WebViewBuilder::build()` blocks) | Async (channel messages to Constellation) | Fully async (IPDL actor pairs, pre-launched process pool) |
| **Command pattern** | Queue-then-drain per frame | Message-passing via channels | Actor pairs with async messages |
| **Primary failure mode** | Double-close race, RefCell panics | Pipeline mapping staleness, message ordering | Child process crash, message serialization failures |

Key insight: as you move from servoshell to verso to Firefox, the gap between "deciding to do something" and "the thing actually happening" widens. Servoshell is nearly synchronous. Firefox makes everything async and treats process death as routine. Graphshell's two-phase model keeps the gap sub-frame (microseconds) while cleanly separating pure state from side effects.

## Current Plumbing (validated)

Before implementation, note what already exists:

- **`notify_url_changed` path exists**: `running_app_state.rs` callback -> `window.notify_url_changed()` -> `window.pending_graph_events` queue -> drained in `gui.rs` -> converted to `GraphIntent`. The work is finishing unification of all semantics through this path, not building new plumbing.
- **WebView pane rendering is tile-driven**: blitting already uses active tile rects in `gui.rs`, not only the legacy fullscreen path. The open question is architectural placement (centralized compositing vs pane handler), not missing implementation.
- **UUID identity is implemented**: `id_to_node: HashMap<Uuid, NodeKey>` and `url_to_nodes: HashMap<String, Vec<NodeKey>>` exist in `graph/mod.rs`. Persistence types carry `node_id` (UUID) throughout. Snapshot round-trip parses UUIDs. Remaining work is removing residual URL-era assumptions, not designing the schema.
- **Tile/graph integrity has partial mechanisms**: `prune_stale_webview_tiles` and invariant checks exist. Gap is formalized policy, not missing code.
- **`sync_to_graph` is mostly reduced**: remaining scope is stale mapping cleanup + active selection reconciliation. Decision needed: keep as reconciliation or fold into semantic-event pass.

## Dual Dispatch Inventory

Two parallel dispatch systems coexist today, sharing `ServoShellWindow`. Understanding exactly who uses which path is prerequisite to migration.

### Path 1: Servoshell command queue (window-global targeting)

```text
User action → queue_user_interface_command(Go/Back/Forward/Reload)
  → pending_commands: RefCell<Vec<UserInterfaceCommand>>
    → handle_interface_commands() drains queue
      → window.active_webview().go_back(1)  // targets whatever Servo thinks is "active"
```

**Callers today:**

- Mouse Back/Forward buttons (`headed_window.rs:626,634`) — `UserInterfaceCommand::Back/Forward`
- Address bar fallback (`webview_controller.rs:290`) — `UserInterfaceCommand::Go` when `focused_webview` is None
- EGL path (`egl/app.rs:386-416`) — all navigation (`load_uri`, `go_back`, `go_forward`, `reload`)

**Targeting mechanism:** `window.active_webview()` via `WebViewCollection` — returns whatever webview last received `activate_webview()`. This is a window-global concept.

### Path 2: GraphShell tile-explicit targeting

```text
User action → resolve active_webview_node from tile tree
  → graph_app.get_webview_for_node(node_key)
    → window.webview_by_id(webview_id)
      → webview.go_back(1)  // targets explicit webview
```

**Callers today:**

- Toolbar back/forward/reload buttons (`gui.rs:707-767`)
- Address bar primary path (`webview_controller.rs:276-287`) — `webview.load()` when `focused_webview` is Some

**Targeting mechanism:** Tile tree focus → `NodeKey` → `get_webview_for_node()` → `WebViewId` → `webview_by_id()`. This is tile-explicit.

### Path 3: Servo delegate → intent reducer (event-driven)

```text
Servo callback → window.pending_graph_events.push(GraphSemanticEvent::*)
  → gui.rs drains → graph_intents_from_semantic_events()
    → graph_app.apply_intents(frame_intents)
```

Handles structural mutations (new nodes, URL changes, history, titles). Does not handle navigation commands.

### Targeting disagreement risk

Paths 1 and 2 can disagree within the same frame. If tile A is focused in the tile tree but Servo's `active_webview` is webview B (because `window.activate_webview()` wasn't called after a tile switch), the command queue path acts on B while the toolbar acts on A. This is resolved by eliminating Path 1 callers (Phases B+D).

### Edge glue: lifecycle reconciliation helpers

Legacy `manage_lifecycle()` has been removed. The bridge now lives in focused helpers in `webview_controller.rs` and `gui.rs`:
- reconciliation emits lifecycle intents (`MapWebviewToNode`, `UnmapWebview`, `PromoteNodeToActive`, `DemoteNodeToCold`),
- frame code applies semantic intents first, then reconciliation/lifecycle intents at frame boundaries.

### `handle_interface_commands()` fate

`handle_interface_commands()` (`window.rs:402-449`) drains the `pending_commands` queue. After Phases B+D delete `Go/Back/Forward/Reload` variants, only `ReloadAll` remains. The function reduces to a single match arm and can be inlined or kept as-is.

## Identity Invariants

Formalized from code audit (Feb 16, 2026):

- **Node identity** is UUID (`node.id: Uuid`), stable across sessions via persistence.
- **`NodeKey`** (petgraph `NodeIndex`) is the in-memory handle. Not stable across sessions (indices change on graph rebuild).
- **`WebViewId` -> `NodeKey`** mapping is the runtime bridge between Servo webviews and graph nodes (`webview_to_node` / `node_to_webview` in `app.rs`).
- **URL is a mutable property**, not identity. Duplicate URLs are expected (same URL open in multiple tabs = multiple independent nodes).
- **Reducer resolves nodes by `NodeKey` or `WebViewId`**, never by URL. All `WebView*` intent variants use `get_node_for_webview(webview_id)` to find the target node.
- **`url_to_nodes`** exists for search/lookup and persistence recovery, not for identity resolution in the reducer.
- **Production reducer is already clean**: `get_node_by_url()` is only called in tests (verified by grep). No URL-as-identity in the intent handling path.

## Reducer/Effect Boundary

**Decided: Two-phase apply.**

The `GraphIntent` reducer in `app.rs` is pure synchronous state mutation. Lifecycle operations (webview create/destroy) require `ServoShellWindow` access and OpenGL context -- these are side effects that cannot live in the reducer.

**Two-phase frame model:**

```text
Frame loop:
  1. Collect intents (keyboard, graph events, Servo delegate, UI)
  2. apply_intents(intents)             <- pure state: graph, lifecycle flags, selection
  3. reconcile_webview_lifecycle()      <- side effects: create/destroy webviews
  4. Render
```

- **Phase 1** (`apply_intents`): Pure state mutation. Graph structure, lifecycle flags, selection, persistence log. No Servo API calls, no OpenGL, no window access. Fully testable without a running browser. **"Pure" means**: the reducer may mutate any field on `GraphBrowserApp` (including runtime metadata like webview mappings and lifecycle flags), but must never call Servo, window, or rendering APIs. The boundary is API calls, not data scope.
- **Phase 2** (`reconcile_webview_lifecycle`): Compares desired state (graph lifecycle flags) against actual state (live webviews). Creates missing webviews, destroys stale ones. This is where `ServoShellWindow`, `OffscreenRenderingContext`, etc. are needed.

**Why not the alternatives:**

- *Option 1 (intents return effects)*: Would require `apply_intent()` to return `Vec<SideEffect>`, changing every call site. Conflates intent semantics with effect scheduling. The reducer becomes aware of the side-effect vocabulary.
- *Option 3 (keep current pattern)*: Lifecycle mutations bypass the intent boundary entirely. Phase C (routing lifecycle through GraphIntent) becomes impossible.

**Phase gap invariant**: Nothing reads lifecycle state between `apply_intents()` and `reconcile_webview_lifecycle()`. These two calls must be adjacent in the frame loop with no rendering or state queries between them. In `gui.rs`, the frame order must be:

1. `handle_keyboard_actions()` / collect UI intents / `graph_intents_from_pending_semantic_events()`
2. `graph_app.apply_intents(frame_intents)` (currently at `gui.rs:1331`)
3. `reconcile_webview_lifecycle()` (new — replaces current `manage_lifecycle()` call)
4. Toolbar, tab bar, physics update, view rendering

This invariant should be enforced with a code comment at the apply site and, in debug builds, an assertion that no webview queries occur between steps 2 and 3.

## Atomicity Policy

- **Graph mutations** (add/remove node, add edge, update URL): atomic per intent, logged to persistence.
- **Lifecycle flag changes** (promote/demote): atomic per intent, **not logged** (derived from runtime state, not persistent).
- **Reconciliation**: best-effort with backpressure. If webview creation fails, retry up to 3 frames, then demote to Cold and log a warning. Prevents infinite retry loops (e.g., GPU memory exhaustion).
- **No rollback across intents** in a batch. Each intent is independent. If intent 2 of 5 fails, intents 1, 3, 4, 5 still apply.

## Lifecycle Intent Vocabulary

Four new `GraphIntent` variants:

- **`PromoteNodeToActive { key: NodeKey }`** -- sets `node.lifecycle = Active`. Does not create a webview (that's reconciliation's job).
- **`DemoteNodeToCold { key: NodeKey }`** -- sets `node.lifecycle = Cold`, clears webview mapping.
- **`MapWebviewToNode { webview_id: WebViewId, key: NodeKey }`** -- registers bidirectional mapping in `webview_to_node` / `node_to_webview`.
- **`UnmapWebview { webview_id: WebViewId }`** -- removes mapping.

**Answer to the Phase A success question**: "When `GraphIntent::PromoteNodeToActive` is applied, what creates the webview?" -- The reconciliation pass sees an Active node without a webview and creates one.

## Implementation Phases

### Phase A: Implement reducer/effect boundary

**Dependency**: None (decisions resolved above)

**Status**: Implemented with pragmatic backpressure heuristic.

**Migration checklist:**

- [x] Add 4 lifecycle intent variants to `GraphIntent` enum (`app.rs`)
- [x] Implement handlers in `apply_intent()` (`promote_node_to_active`, `demote_node_to_cold`, `map_webview_to_node`, `unmap_webview`)
- [x] Extract reconciliation duties out of the legacy `manage_lifecycle()` shape
- [x] Remove legacy `manage_lifecycle()` path (lifecycle now emitted as intents from reconciliation helpers)
- [x] Update frame loop in `gui.rs` to apply semantic intents, reconcile lifecycle, then apply lifecycle intents
- [x] Update `WebViewCreated` handler (`app.rs`) to use lifecycle intents internally
- [x] Add failure backpressure: retry counter on nodes, demote after 3 failed creation probes (timeout/no-confirmation heuristic in `gui.rs`)
- [x] Add tests for lifecycle intents/reconciliation behavior (`app.rs` + `webview_controller.rs` unit coverage)
- [x] Document phase gap invariant in code comments (`gui.rs`)

**Risks and mitigations:**

- **Stale state between phases**: After apply sets Active, before reconcile creates webview, any code reading lifecycle state sees "active but no webview." *Mitigation*: phase gap invariant -- no reads between apply and reconcile. The gap is sub-frame (microseconds).
- **Reconciliation loses intent context**: It sees "node Active, no webview" but not *why* (user pressed N? Servo callback? Restoration from graph view?). *Mitigation*: all webview creations are currently identical (URL + rendering context). If differentiated creation is needed later, encode in node data, not in the reconciliation pass.
- **Infinite retry without backpressure**: If webview creation fails (e.g., GPU memory), reconcile retries every frame forever. *Mitigation*: retry counter on nodes, demote to Cold after 3 failures.
- **Reducer scope growth**: 21 variants (17 current + 4 new). *Mitigation*: reducer stays pure and testable. Split into sub-reducers by domain (graph, lifecycle, UI) later if needed.

**Comparison**: Servoshell uses synchronous command execution (no gap between intent and effect). Firefox uses fully async actor pairs (gap is large but handled by explicit "not yet ready" states). Graphshell's two-phase is the middle ground -- gap exists but is sub-frame.

**Testable invariants:**

- `grep -rn 'promote_node_to_active\|demote_node_to_cold\|map_webview_to_node\|unmap_webview' app.rs` shows these are only called inside `apply_intent()` match arms (not from gui.rs or webview_controller.rs directly).
- Unit tests: `PromoteNodeToActive` intent sets lifecycle flag; reconciliation (with mock window) creates webview for Active node without one.
- Debug assertion: no `window.webviews()` or `window.active_webview()` calls between `apply_intents` and `reconcile_webview_lifecycle` in gui.rs frame loop.
- Runtime adjacency test: add a `#[cfg(debug_assertions)]` flag on `GraphBrowserApp` (e.g., `intents_applied_pending_reconcile: bool`) set to `true` after `apply_intents`, cleared after `reconcile_webview_lifecycle`. Any webview query while the flag is true triggers `debug_assert!(false, "webview query between apply and reconcile")`. This prevents the ordering from silently drifting.

### Phase B: Finalize delegate-driven semantics

**Dependency**: Phase A complete (lifecycle intents available)

The Servo delegate -> `GraphIntent` path already works (`window.rs` -> `pending_graph_events` -> `gui.rs` -> `graph_intents_from_semantic_events()` -> `apply_intents()`). This phase unifies all navigation semantics through it.

**Tasks:**

1. Ensure `notify_url_changed` intent path handles same-tab URL updates without creating new nodes. (Already works -- `WebViewUrlChanged` handler at `app.rs:405-426` updates URL on existing node.)
2. **Traversal record creation**: `WebViewHistoryChanged` now stores metadata and creates `Traversal` records on navigation transitions. **Per edge traversal model (2026-02-20)**: instead of `EdgeType::History` singletons, navigation events append to `Vec<Traversal>` with timestamp, trigger (Back/Forward/ClickedLink), and snapshot URLs. This is commutative (order-independent append) and eliminates the "when to create edge" complexity.
3. Ensure `request_create_new` emits graph-meaningful intent (new node + initial traversal record). (Already works -- `WebViewCreated` handler at `app.rs:380-404` does this; will need update when traversal model is implemented.)
4. Remove URL-change -> new-node creation path from `sync_to_graph` in `webview_controller.rs`. (Already done -- `sync_to_graph_intents` at line 210 is reconciliation-only.)
5. **Decision**: keep `sync_to_graph_intents` as reconciliation-only (stale mapping cleanup + active selection), not structural node creation.
6. Remove `PHASE 0 PROOF` comment and convert fallback `Go` command at `webview_controller.rs:263-290`. **Done** (detail-mode no-focused-webview path emits `CreateNodeAtUrl` intent).

**Risks and mitigations:**

- **Delegate ordering under redirects**: If `notify_url_changed` fires multiple times during a redirect chain, each fires a `WebViewUrlChanged` intent. Last URL wins (correct), but rapid fire causes unnecessary persistence log entries. *Mitigation*: debounce URL log writes (only log if URL differs from last logged).
- **Traversal record semantics** (simplified by edge traversal model): **Old complexity eliminated**: The previous "when does a history entry become an edge" problem no longer exists. With `Vec<Traversal>`, every navigation that crosses node boundaries creates a traversal record (timestamp, from_url, to_url, trigger). Traversal records are **commutative** (order-independent append), so concurrent navigations from P2P sync merge automatically with no conflicts. **State transition detection** (retained from old logic): On `WebViewHistoryChanged`, compare `new_idx` vs `old_idx` to determine trigger:
  - **Back**: `new_idx < old_idx` → `NavigationTrigger::HistoryBack`
  - **Forward**: `new_idx > old_idx` AND list length unchanged → `NavigationTrigger::HistoryForward`
  - **Normal navigation**: `new_idx > old_idx` AND list grew → `NavigationTrigger::ClickedLink` or `TypedUrl` (distinguish via other signals)
  - Then resolve old/new URLs to `NodeKey`s and append traversal record to edge (or create edge + traversal if first navigation between that pair).
- **SPA transitions**: Single-page apps fire `notify_url_changed` for fragment/pushState changes without real navigation. *Mitigation*: compare old and new URL; skip traversal creation for same-origin fragment-only changes (or tag as `NavigationTrigger::SpaTransition` for filtering in UI).
- **`sync_to_graph` removal timing**: If we remove the reconciliation pass before all its duties are covered by events, we lose stale mapping cleanup. *Mitigation*: keep `sync_to_graph_intents` running during Phase B; only consider removal in Phase D after verifying no regressions.

**Comparison**: Servoshell uses `UserInterfaceCommand::Go/Back/Forward` dispatched to `active_webview()` -- window-global targeting. Graphshell already routes to specific webviews via tile -> node -> webview mapping. Firefox routes to specific `BrowsingContext` via JSActors -- no global dispatch. Phase B aligns graphshell with Firefox's model (explicit targeting) over servoshell's (global dispatch).

**Success criteria:**

- Same-tab navigation updates node URL without creating a new node.
- New-tab action creates exactly one node and one edge with initial traversal record (trigger: `NewTabFromParent` or equivalent).
- History callbacks create traversal records on back/forward with correct triggers (`HistoryBack`/`HistoryForward`), not on every page load.
- Traversal records are appended to existing edges (if edge exists between node pair) or create new edge + first traversal (if first navigation between that pair).
- No node creation from polling path.
- `PHASE 0 PROOF` comment and `UserInterfaceCommand::Go` fallback removed.

**Testable invariants:**

- `grep -rn 'add_node_and_sync' webview_controller.rs` returns zero hits (no URL-polling structural node creation).
- Unit test: `WebViewHistoryChanged` with decreasing `history_index` appends a traversal record with `NavigationTrigger::HistoryBack`; with increasing index and growing list, it does not create a traversal (or creates one tagged as normal navigation).
- After implementing traversal model: `EdgePayload.traversals.len() >= 0` for all edges (user-asserted edges may have zero traversals; navigation-created edges have at least one).
- `grep -rn 'PHASE 0 PROOF' ports/graphshell/` returns zero hits.

### Phase C: Route lifecycle mutations through GraphIntent

**Dependency**: Phase A boundary implemented, Phase B complete

**Tasks:**

1. Replace direct lifecycle mutation calls in `gui.rs` with intent emission + reconciliation.
2. The 4 lifecycle intents from Phase A are already in the reducer. This phase wires the callers.
3. Legacy `manage_lifecycle()` path is removed; lifecycle helpers now emit `Vec<GraphIntent>` and frame code applies them at boundary points.

**Risks and mitigations:**

- **Graph-view teardown ordering**: Currently `manage_lifecycle()` saves active nodes, then destroys webviews, then unmaps, all in one function. If split into intents (`DemoteNodeToCold` x N) + reconciliation (destroy webviews), the save-before-destroy must still happen atomically. *Mitigation*: keep the save logic (`active_webview_nodes`) in the caller before emitting demote intents. Or add a `SaveActiveWebviewNodes` intent.
- **Double lifecycle transition**: If a node is promoted and demoted in the same frame, the intents cancel out. Reconciliation sees Cold node -- correct. *Mitigation*: lifecycle flags are not logged (per atomicity policy), so no persistence noise.
- **Webview creation requires rendering context**: The reconciliation pass needs `OffscreenRenderingContext` and `WindowRenderingContext`. *Mitigation*: reconciliation takes the same parameters as current `manage_lifecycle()`. No new threading required.

**Comparison**: Servoshell mixes state mutation and side effects in `handle_interface_commands()` -- no separation. Verso separates via message channels. Firefox separates via process boundaries (parent decides, child executes). Phase C gives graphshell verso-level separation without message channels.

**Success criteria:**

- No direct `app.promote_node_to_active()` / `app.demote_node_to_cold()` calls outside `apply_intent()`.
- Lifecycle helpers return intents; no helper-local `apply_intents()` calls in `gui.rs`/`webview_controller.rs` lifecycle paths.
- Webview create/destroy still works through reconciliation.

**Testable invariants:**

- `grep -rn 'promote_node_to_active\|demote_node_to_cold' webview_controller.rs gui.rs` returns zero direct calls (all go through `GraphIntent` variants).
- `grep -rn 'manage_lifecycle\\(' ports/graphshell/` returns docs-only hits (no runtime code path).

### Phase D: Delete legacy paths and close UI loose ends

**Dependency**: Phase C complete

**Tasks:**

1. Delete legacy fullscreen-detail fallback path (`gui.rs:1282-1328` else branch).
2. Delete `UserInterfaceCommand::{Go, Back, Forward, Reload}` variants from the desktop path -- toolbar already routes directly to per-webview calls (`gui.rs:707-767`). **EGL/embedded impact**: `egl/app.rs:386-416` uses these variants for `load_uri()`, `reload()`, `go_back()`, `go_forward()`. These must be refactored to direct webview calls at the same time, or the EGL path breaks. Phase D is **not desktop-only** — it requires equivalent refactoring in `egl/app.rs`.
   - **Scope decision checkpoint**: If a given iteration is explicitly "desktop tile architecture only," document EGL/WebDriver targeting semantics as out-of-scope for that iteration. If full-stack consistency is in scope, refactor both `egl/app.rs` and `webdriver.rs` to explicit webview IDs (no window-global active dispatch).
   - **Current decision (Feb 17)**: desktop tile architecture is in focus for this cycle. EGL/WebDriver explicit-target refactor remains important follow-up work, tracked but deferred.
3. Keep `ReloadAll` for multi-window coordination (`gui.rs:792`).
4. Remove stop button (`gui.rs:744`). Servo's `WebView` API has no `stop()` method (verified Feb 16). Remove the button entirely rather than leaving a stub. If Servo adds `stop()` later, the button can be re-added.
5. Remove `UserInterfaceCommand::Back/Forward` from mouse button handlers (`headed_window.rs:626,634`) -- replace with direct webview calls via active tile.
6. Resolve tab key handling (`headed_window.rs:680` -- TODO about tab key and `consumed` flag).
7. Address fullscreen anti-phishing mitigation (`gui.rs:689` TODO) or document deferral.
8. Clean up stale comments (`PHASE 0 PROOF` if not already removed in Phase B).

**Risks and mitigations:**

- **Removing the fallback path**: The `gui.rs:1282` else branch catches tile runtime init failures. Without it, a missing tile root means a blank screen. *Mitigation*: `ensure_tiles_tree_root()` already guarantees a root tile exists. Add a debug assertion that tile root is never None when rendering.
- **Mouse button Back/Forward**: Currently routes through `UserInterfaceCommand` which targets `active_webview()`. Replacing with direct webview call requires knowing the active tile's webview from `headed_window.rs` context. *Mitigation*: thread the active tile webview ID through the event handler, or keep these two commands as a thin wrapper that resolves via active tile.
- **Stop button removal**: Servo's `WebView` API has no `stop()` method (verified). Removing the button is straightforward but changes the toolbar layout slightly during page load (reload button shows immediately instead of stop→reload transition). *Mitigation*: accept simplified toolbar. Re-add stop button if/when Servo exposes the API.
- **Tab key handling**: Servo doesn't yet support tabbing through links/inputs. Consuming Tab in egui prevents webview from seeing it; passing it through breaks egui focus. *Mitigation*: implement focus-ownership model — when the focused tile contains a webview (`get_webview_for_node(active_tile_node).is_some()`), set `consumed = false` for Tab events in `headed_window.rs` so they pass through to Servo's input handler. When egui controls have focus (toolbar, address bar, graph view), consume Tab normally. The determination happens in `handle_winit_window_event()` before the `consumed` flag is checked.
- **AccessKit / accessibility forwarding**: Servo does not currently expose its accessibility tree to embedders. Graphshell can only forward egui's own AccessKit tree (toolbar, graph view labels, tab bar). Webview content accessibility is blocked on Servo providing an embedder API for it. *Status*: known limitation, noted at `headed_window.rs:872`. No graphshell-side work until Servo exposes the API.

**Comparison**: Servoshell still uses `UserInterfaceCommand` for all navigation dispatch -- no per-webview direct calls. Phase D brings graphshell past servoshell's model to direct webview targeting, matching Firefox's explicit-BrowsingContext-targeting pattern.

**Success criteria:**

- Single rendering path (tile runtime only, no fullscreen fallback).
- No window-global navigation dispatch. All navigation targets explicit tile/webview.
- No stubbed UI controls (stop button works or is removed).
- Mouse Back/Forward buttons work with per-webview targeting.

**Testable invariants:**

- `grep -rn 'UserInterfaceCommand::Go\|UserInterfaceCommand::Back\|UserInterfaceCommand::Forward\|UserInterfaceCommand::Reload' ports/graphshell/` returns zero hits (only `ReloadAll` remains).
- EGL path (`egl/app.rs`) compiles and functions without `UserInterfaceCommand::{Go,Back,Forward,Reload}`.
- Legacy fallback else branch at `gui.rs:1282` is deleted; `ensure_tiles_tree_root()` has a debug assertion.

## Open Blockers

1. ~~Phase A is the blocker~~ -- **Core implementation complete** (lifecycle intents wired, legacy lifecycle path removed, frame boundary comments added, retry/demote backpressure heuristic added).
2. ~~Delegate ordering~~ -- **Resolved for current model** via deterministic queue-transform tests (`gui.rs`) plus runtime traces (redirect, SPA pushState, hash-change, back/forward burst, `window.open`). Key rule captured in reducer/tests: traversal semantics use `history_changed` index/list as authoritative, not URL callback deltas alone. **Edge traversal model (2026-02-20) further simplifies**: `Vec<Traversal>` records are commutative, eliminating ordering sensitivity for P2P sync.
3. ~~Close-tab policy~~ -- **Resolved for current model**: closing a webview tile demotes its node to `Cold` (does not delete graph node). This is implemented in lifecycle close paths (`gui.rs`, `webview_controller.rs`) and lifecycle reducer behavior (`DemoteNodeToCold` in `app.rs`). Mode-pluggable delete-vs-hide remains a future enhancement.
4. ~~Stop button API~~ -- **Resolved**: `webview.stop()` does not exist in Servo's `WebView` API (verified Feb 16). Decision: remove the stop button in Phase D.
5. **AccessKit / webview accessibility**: Partially blocked on Servo/embedder bridge surface. Graphshell-side handling now records and warns on dropped webview accessibility updates (non-silent failure), but true web-content accessibility exposure still requires upstream API/bridge support.

## Crash Handling Policy (Specified 2026-02-17)

### Current state

- `RunningAppState::notify_crashed(...)` forwards to platform window.
- Graphshell desktop path converts crash events into semantic intents and reducer transitions (`WebViewCrashed` -> demote/unmap + runtime crash metadata).
- Open crashed tiles show a non-blocking crash banner with recovery actions (`Reload`, `Close Tile`).
- Remaining limitation is upstream API surface (for example web-content accessibility bridge), not missing desktop crash-policy wiring.

### Policy goals

1. Web content process crashes must not crash graphshell.
2. Crashes must be visible to the user in the affected tab/tile context.
3. Graph integrity must remain valid (`node` persists; runtime webview state is cleaned up).
4. Recovery path should be explicit (reload/reopen), not implicit infinite retry.

### Required behavior

1. **On `notify_crashed(webview_id, reason, backtrace)`**:
   - close/unmap crashed webview runtime state,
   - demote mapped node lifecycle to `Cold`,
   - record transient crash metadata for UI (reason summary + timestamp + optional backtrace presence),
   - request UI update/repaint.
2. **UI behavior**:
   - if crashed node has an open tile, show a non-blocking crash banner in that tile context:
     - `"Tab crashed"` + actions: `Reload` and `Close Tile`.
   - graph view remains interactive.
3. **Recovery**:
   - `Reload`/reopen uses standard lifecycle intent path (`PromoteNodeToActive` + reconcile),
   - no automatic per-frame recreation loop after crash.
4. **Persistence policy**:
   - crash metadata is runtime-only (not persisted in graph snapshots/log entries).

### Testable invariants

1. Crash callback does not panic and does not terminate app event loop.
2. After crash handling:
   - webview mapping is absent for crashed webview,
   - mapped node lifecycle is `Cold`,
   - node still exists in graph.
3. Reopening crashed node produces a new webview mapping and returns node to `Active`.

### Implementation checklist

- [x] Add crash semantic event/intent path (`window.rs` -> `gui.rs` conversion -> `app.rs` reducer).
- [x] Add runtime crash metadata store in GUI/app state.
- [x] Add tile-level crash banner/actions.
- [x] Add unit tests for reducer-side crash transitions and reopen flow.

## Delegate Ordering Validation Protocol

Use this protocol to resolve Open Blocker #2 with reproducible evidence.

### Setup

1. Run graphshell with delegate tracing enabled:
   - Windows PowerShell: `$env:GRAPHSHELL_TRACE_DELEGATE_EVENTS=1; cargo run -p graphshell`
2. Ensure log level includes `debug` for `graph_event_trace` lines.
3. Capture logs to a file for each scenario.

### Scenario Matrix

1. Redirect chain:
   - Navigate to a URL that issues 2+ HTTP redirects.
   - Expected trace characteristic: multiple `url_changed` events, then stable title/history updates.
2. SPA pushState:
   - Open a SPA that changes route without full reload.
   - Expected: URL/title/history ordering may differ from full navigation; confirm reducer final state is correct.
3. Hash-only navigation:
   - Trigger `#fragment` changes on same document.
   - Expected: URL changes may occur without meaningful traversal record creation (intra-node URL updates).
4. Back/Forward traversal:
   - Perform back/forward repeatedly on a tab with known history.
   - Expected: `history_changed` reflects traversal direction and index movement.
5. `window.open` new tab:
   - Trigger a child webview creation.
   - Expected: `create_new` appears, child receives subsequent URL/title/history callbacks.

### Acceptance Criteria

1. For each scenario, callback traces are explainable by current event routing (`running_app_state.rs` -> `window.rs` -> `gui.rs` intent conversion).
2. No unexplained ordering causes structural graph regressions (wrong node mapping, duplicate node creation, missing expected edge updates).
3. Any unstable ordering pattern is documented and either:
   - normalized in event preprocessing/reducer rules, or
   - explicitly tolerated with rationale and tests.

### Initial Trace Snapshot (2026-02-17)

Collected with:
- `GRAPHSHELL_TRACE_DELEGATE_EVENTS=1`
- `RUST_LOG=graphshell=debug`
- `-z -x --tracing-filter debug`

Artifacts:
- Redirect trace: `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_redirect.log`
- SPA script trace (file URL): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_spa_pushstate.log`
- Hash script trace (file URL): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_hash_change.log`
- SPA script trace (HTTP): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_spa_pushstate_http.log`
- Hash script trace (HTTP): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_hash_change_http.log`
- Back/forward burst trace (HTTP): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_back_forward_burst_http.log`
- `window.open` trace (HTTP): `ports/graphshell/design_docs/graphshell_docs/implementation_strategy/delegate_trace_window_open_http.log`

Observed ordering in these runs:
- Redirect case (`https://httpbin.org/redirect/2`): `url_changed` -> `history_changed` (final URL observed as `https://httpbin.org/get`).
- Local file script cases: initial `url_changed` + `history_changed`, then title callbacks (`title_present=false` then `title_present=true`).
- SPA pushState over HTTP: initial `url_changed/history_changed`, then same-frame `url_changed/history_changed` for `?step=0`, followed by `url_changed/history_changed` for `?step=1`, with title updates interleaved.
- Hash-change over HTTP: initial `url_changed/history_changed`, then title updates only (no subsequent `url_changed`/`history_changed` for fragment change in this run).
- Back/forward burst over HTTP: history index changed (`2 -> 1 -> 2`) while URL callback values remained at `?step=2`, so traversal record creation should rely on `history_changed` index/list callbacks (to determine `HistoryBack` vs `HistoryForward` trigger), not URL changes alone.
- `window.open` over HTTP: `create_new` event emitted before child URL/history callbacks; child first appeared as `about:blank` (initial URL absent), followed by child `url_changed/history_changed` events.

Interpretation:
- Delegate queue ordering is behaving deterministically in sampled runs.
- SPA/hash HTTP traces now confirm key runtime behavior patterns needed by reducer logic.
- Back/forward and `window.open` scenarios are now captured with trace artifacts.
- Important nuance from back/forward burst trace: callback payload may advance history index without carrying the expected traversed URL string; reducer logic should treat `history_changed` index/list as authoritative for traversal semantics and not infer direction solely from consecutive `url_changed` values.

---

## Edge Traversal Model Benefits for This Architecture (Added 2026-02-20)

The **edge traversal model** (`2026-02-20_edge_traversal_model_research.md`) fundamentally simplifies Phase B implementation and future P2P sync (cross-reference: `2026-02-11_p2p_collaboration_plan.md`).

### Key Change Summary

**Old model (pre-2026-02-20)**:
```rust
pub enum EdgeType {
    Hyperlink,    // new tab from parent
    History,      // back/forward navigation
    UserGrouped,  // explicit user connection
}
// Edge weight in petgraph: EdgeType (singleton enum value)
```

**New model (2026-02-20)**:
```rust
struct EdgePayload {
    user_asserted: bool,              // true for explicit user connections
    traversals: Vec<Traversal>,       // append-only, commutative
}

struct Traversal {
    from_url: String,       // snapshot at navigation time
    to_url: String,         // snapshot at navigation time
    timestamp: u64,         // Unix milliseconds
    trigger: NavigationTrigger,  // Back/Forward/ClickedLink/TypedUrl/...
}
```

### Architectural Benefits

1. **Eliminates "when to create edge" complexity**: The old model had to decide when a history entry becomes an edge vs. just updating an existing edge. With `Vec<Traversal>`, every navigation appends a new record. No state machine required.

2. **Traversals are commutative**: Order-independent append means concurrent navigations from P2P sync merge automatically with no conflicts. Two peers can add different traversals to the same edge simultaneously — both survive. This is a **major win for P2P sync** (Phase 1-5 in the collaboration plan).

3. **Preserves temporal frequency data**: The old model silently discarded repeat navigations (`!has_history_edge` guard). The new model captures every traversal with timestamp, enabling:
   - "Show me most-traveled paths" queries
   - Heatmap visualization (edge thickness = traversal count)
   - Temporal filtering ("only show edges traversed this week")

4. **Trigger metadata enables filtering**: `NavigationTrigger::HistoryBack` vs `ClickedLink` vs `TypedUrl` allows UI to show "only back/forward edges" or "only clicked links" without losing data.

5. **Reducer complexity stays bounded**: Adding a traversal record is a simple append-to-vec operation. The reducer doesn't need complex "merge edge types" logic when the same node pair gets navigated via different triggers.

### Implementation Mapping for Phase B

**Old Phase B logic** (complex):
- Read `history_index` delta to infer direction
- Check if edge already exists with `EdgeType::History`
- If exists, skip (data loss)
- If not exists, create edge with `History` type
- Separate code path for `EdgeType::Hyperlink` (new tab from parent)

**New Phase B logic** (simplified):
- Read `history_index` delta to determine `NavigationTrigger` (Back/Forward/Normal)
- Resolve old/new URLs to `NodeKey`s
- Check if edge exists between those keys (direction-agnostic)
- If exists, append `Traversal` to `edge.payload.traversals`
- If not exists, create edge with `user_asserted: false` and first `Traversal` record
- **No type checking, no conditional skipping, no data loss**

### Persistence Impact

**Old `LogEntry`** (pre-traversal model):
```rust
AddEdge { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType }
RemoveEdge { from_node_id: Uuid, to_node_id: Uuid, edge_type: PersistedEdgeType }
```

**New `LogEntry`** (traversal model):
```rust
AddTraversal {
    from_node_id: Uuid,
    to_node_id: Uuid,
    timestamp: u64,
    trigger: PersistedNavigationTrigger,
    from_url_snapshot: String,  // URL at navigation time
    to_url_snapshot: String,
}
AssertEdge { from_node_id: Uuid, to_node_id: Uuid }  // user-grouped edges
```

The `AddEdge` variant with `edge_type` becomes obsolete. Backward compatibility: old `AddEdge` logs deserialize as `AssertEdge` (user-grouped) or are converted to `AddTraversal` based on `edge_type` value during log replay.

### Cross-References and Migration Path

**Blocked on**: Edge traversal model implementation (tracked in `2026-02-20_edge_traversal_impl_plan.md`, not yet started).

**Phase B can proceed with old model**: Phase B's delegate-driven semantics work with the current `EdgeType` enum. The traversal model is a **data model refactor**, not a navigation routing change. Phase B implementation can land first, then migrate to traversal records in a follow-up phase.

**When to migrate Phase B to traversal model**:
1. After edge traversal model is implemented (petgraph edge weights changed, persistence layer updated).
2. Update `WebViewHistoryChanged` handler to call `add_traversal()` instead of `add_edge_if_not_exists()`.
3. Update `WebViewCreated` handler (new tab from parent) to append traversal with `NavigationTrigger::NewTabFromParent`.
4. Remove `has_history_edge` guard logic (no longer needed).

**Testable invariants after migration**:
- `EdgePayload.traversals.len() >= 0` for all edges.
- User-asserted edges (`user_asserted: true`) may have zero traversals (created by explicit "group" command).
- Navigation-created edges have at least one traversal.
- Traversal timestamps are monotonic per edge (within local clock precision; P2P sync may have out-of-order timestamps from different peers).

---

## Servoshell Scaffold Relationship

Graphshell is a thin fork of servoshell, not a plugin. Understanding the inheritance boundary clarifies what each phase can safely change.

### Unchanged files (~25)

`lib.rs`, `main.rs`, `build.rs`, `backtrace.rs`, `crash_handler.rs`, `panic_hook.rs`, `parser.rs`, `prefs.rs`, `resources.rs`, `test.rs`, `webdriver.rs`, `desktop/accelerated_gl_media.rs`, `desktop/cli.rs`, `desktop/dialog.rs`, `desktop/event_loop.rs`, `desktop/gamepad.rs`, `desktop/geometry.rs`, `desktop/headless_window.rs`, `desktop/keyutils.rs`, `desktop/mod.rs`, `desktop/protocols/*`, `desktop/tracing.rs`, `desktop/webxr.rs`, `egl/*`, `platform/macos/*`.

These provide Servo engine lifecycle, cross-platform windowing (winit), input event routing, rendering context abstraction (OpenGL), webdriver/testing harness, and gamepad support — all for free.

### Surgically extended files (4 touch points)

| File | What graphshell adds | Lines added |
| ---- | -------------------- | ----------- |
| `window.rs` | `GraphSemanticEvent` enum, `pending_graph_events` queue, `create_toplevel_webview_with_context()` for per-tile rendering | ~60 |
| `running_app_state.rs` | Hooks 4 delegate callbacks to push `GraphSemanticEvent`s instead of just `set_needs_update()` | ~20 |
| `headed_window.rs` | Graph-view input routing (T/P/C/Home/Escape bypass webview), multi-webview mouse targeting via `webview_at_point()`, per-tile focus | ~100 |
| `desktop/app.rs` | Graph store init, tile system, persistence recovery, per-tile rendering contexts | 3x larger |

### Wholly new modules (9)

`app.rs` (reducer + intent system), `graph/` (petgraph model + egui adapter), `persistence/` (fjall + redb + rkyv), `render/` (graph rendering + physics integration), `input/` (keyboard actions), `search.rs` (nucleo fuzzy matching), `desktop/gui.rs` (3.5x servoshell's — tiles, graph view, search UI, thumbnails, physics panel), `desktop/webview_controller.rs` (lifecycle + reconciliation), `desktop/tile_*.rs` (tile behavior + kinds).

### What this means for the plan

**Benefits**: Phases A-D only modify the 4 extended files and the 9 new modules. The ~25 unchanged files are not touched, so Servo embedder integration (rendering contexts, webdriver, gamepad, crash handling) remains stable.

**Upstream merge risk**: When upstream servoshell changes delegate signatures in `running_app_state.rs`, WebView APIs in `window.rs`, or input handling in `headed_window.rs`, graphshell's extensions break at the 4 touch points. Mitigation: keep the touch points minimal and well-isolated (currently ~200 lines across 4 files). After Phase D deletes `UserInterfaceCommand::{Go,Back,Forward,Reload}`, the `window.rs` touch point simplifies further because `handle_interface_commands()` shrinks to just `ReloadAll`.

**Phase D specifically**: deleting `UserInterfaceCommand` variants modifies `running_app_state.rs` (shared file). This is the one phase that changes the scaffold itself, not just the extensions. The EGL path (`egl/app.rs`, unchanged file) also needs refactoring here — it's the only unchanged file that Phase D promotes to "extended."

## Guardrails (from prior debugging)

- Do not add runtime instrumentation to diagnose deterministic code-structure failures. Read the code instead.
- Do not patch around the command queue model. Replace it.
- Prefer event-driven Servo callbacks over polling when the model says they are the authority.
- Check tile/lifecycle interactions before concluding a single-path fix is sufficient.
- Timebox diagnostics: one round of logging, then move to a testable change.

## Potential Feature (Diagnostic)

### Headed event-loop/window integration test scaffolding

- **Potential feature**: Add OS-window/event-loop integration test scaffolding to drive headed `winit` events and assert end-to-end target routing (Back/Forward/Reload to focused tile webview).
- **Diagnostic value**: **Yes** (high confidence for focus/input regressions that unit tests cannot fully cover).
- **In current phase scope**: **No**.
- **Reason for "No"**: Requires new harness architecture (test window host abstraction + deterministic event playback + CI stability handling), estimated at multi-day to multi-week effort, and is not required to complete Phases A-D acceptance criteria.
- **Current substitute**: Keep unit-level routing harness/tests in `desktop/gui.rs` and resolver-level assertions; use targeted manual validation for headed runtime behavior.
- **Activation trigger**: Promote this to active scope if repeated focus/input regressions occur that pass unit tests but fail in headed runtime.
