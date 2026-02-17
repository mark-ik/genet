# Architecture and Navigation Plan (2026-02-16)

Consolidated from prior research and plans. Originals archived in `archive_docs/checkpoint_2026-02-16/`.

## Decided Model

**Semantic parity, not structural parity.** Three authority domains:

| Domain | Authoritative For | Examples |
| ------ | ----------------- | -------- |
| **Graph** | Node identity (UUID), lifecycle, edge semantics | Add/remove node, URL change, Hyperlink/History edges |
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

### Edge glue: `manage_lifecycle()`

`manage_lifecycle()` (`webview_controller.rs:105-203`) is the main bridge between the two worlds. It directly calls both `app` state methods (promote/demote/map/unmap) and Servo/window methods (create/destroy webviews) in a single function, outside the reducer. Lines 142-144 are phase-1 work (state mutation), lines 160-164 are phase-2 work (side effects), lines 169-172 are phase-1 again. Phase A untangles this into intent emission + reconciliation.

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

**Status**: Ready for implementation.

**Migration checklist:**

- [ ] Add 4 lifecycle intent variants to `GraphIntent` enum (`app.rs:113`)
- [ ] Implement handlers in `apply_intent()` (reuse existing `promote_node_to_active`, `demote_node_to_cold`, `map_webview_to_node`, `unmap_webview` at `app.rs:638-714`)
- [ ] Extract reconciliation function from `manage_lifecycle()` (`webview_controller.rs:105-203`)
- [ ] Refactor `manage_lifecycle()` to emit intents instead of calling app methods directly
- [ ] Update frame loop in `gui.rs`: intents -> apply -> reconcile -> render
- [ ] Update `WebViewCreated` handler (`app.rs:380-404`) to use lifecycle intents internally
- [ ] Add failure backpressure: retry counter on nodes, demote after 3 failures
- [ ] Add tests for lifecycle intents and reconciliation
- [ ] Document phase gap invariant in code comments

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
2. **History edge creation (missing behavior)**: `WebViewHistoryChanged` (`app.rs:428-443`) currently stores metadata on the node but does not generate structural History edges. Extend to create History edges from navigation history.
3. Ensure `request_create_new` emits graph-meaningful intent (new node + Hyperlink edge). (Already works -- `WebViewCreated` handler at `app.rs:380-404` does this.)
4. Remove URL-change -> new-node creation path from `sync_to_graph` in `webview_controller.rs`. (Already done -- `sync_to_graph_intents` at line 210 is reconciliation-only.)
5. Decide `sync_to_graph` residual scope: keep as reconciliation (stale mapping cleanup + selection) or fold into event-driven path.
6. Remove `PHASE 0 PROOF` comment and convert fallback `Go` command at `webview_controller.rs:263-290`. The fallback fires when `focused_webview` is None (no tile has a webview). The correct behavior is to emit a graph intent that creates a new node + webview for the URL, not to call `UserInterfaceCommand::Go` on a nonexistent active webview.

**Risks and mitigations:**

- **Delegate ordering under redirects**: If `notify_url_changed` fires multiple times during a redirect chain, each fires a `WebViewUrlChanged` intent. Last URL wins (correct), but rapid fire causes unnecessary persistence log entries. *Mitigation*: debounce URL log writes (only log if URL differs from last logged).
- **History edge semantics**: When does a history entry become an edge? If we create edges for every entry in the Servo history list, we generate O(n) edges per page. *Mitigation — state transition rules*: store two values per node: `prev_history_index` and `prev_history_url` (the URL at the previous index). On each `WebViewHistoryChanged`:
  - Let `old_idx = node.history_index`, `new_idx = incoming index`, `old_url = node.history_entries[old_idx]`, `new_url = incoming entries[new_idx]`.
  - **Back**: `new_idx < old_idx` → emit History edge from `old_url` to `new_url`.
  - **Forward**: `new_idx > old_idx` AND `incoming entries.len() == node.history_entries.len()` (list didn't grow) → emit History edge from `old_url` to `new_url`.
  - **Normal navigation**: `new_idx > old_idx` AND `incoming entries.len() > node.history_entries.len()` → do NOT emit edge (regular page load, not traversal).
  - **Same index**: no edge.
  - Then update stored `history_index` and `history_entries`. Refine empirically during Phase B.
- **SPA transitions**: Single-page apps fire `notify_url_changed` for fragment/pushState changes without real navigation. *Mitigation*: compare old and new URL; skip edge creation for same-origin fragment-only changes.
- **`sync_to_graph` removal timing**: If we remove the reconciliation pass before all its duties are covered by events, we lose stale mapping cleanup. *Mitigation*: keep `sync_to_graph_intents` running during Phase B; only consider removal in Phase D after verifying no regressions.

**Comparison**: Servoshell uses `UserInterfaceCommand::Go/Back/Forward` dispatched to `active_webview()` -- window-global targeting. Graphshell already routes to specific webviews via tile -> node -> webview mapping. Firefox routes to specific `BrowsingContext` via JSActors -- no global dispatch. Phase B aligns graphshell with Firefox's model (explicit targeting) over servoshell's (global dispatch).

**Success criteria:**

- Same-tab navigation updates node URL without creating a new node.
- New-tab action creates exactly one node and one Hyperlink edge.
- History callbacks create History edges on back/forward (not on every page load).
- No node creation from polling path.
- `PHASE 0 PROOF` comment and `UserInterfaceCommand::Go` fallback removed.

**Testable invariants:**

- `grep -rn 'add_node_and_sync' webview_controller.rs` returns zero hits (no URL-polling structural node creation).
- Unit test: `WebViewHistoryChanged` with decreasing `history_index` creates a History edge; with increasing index and growing list, it does not.
- `grep -rn 'PHASE 0 PROOF' ports/graphshell/` returns zero hits.

### Phase C: Route lifecycle mutations through GraphIntent

**Dependency**: Phase A boundary implemented, Phase B complete

**Tasks:**

1. Replace direct `manage_lifecycle()` calls in `gui.rs` with intent emission + reconciliation.
2. The 4 lifecycle intents from Phase A are already in the reducer. This phase wires the callers.
3. `manage_lifecycle()` becomes a function that returns `Vec<GraphIntent>` (like `sync_to_graph_intents`), not one that mutates directly.

**Risks and mitigations:**

- **Graph-view teardown ordering**: Currently `manage_lifecycle()` saves active nodes, then destroys webviews, then unmaps, all in one function. If split into intents (`DemoteNodeToCold` x N) + reconciliation (destroy webviews), the save-before-destroy must still happen atomically. *Mitigation*: keep the save logic (`active_webview_nodes`) in the caller before emitting demote intents. Or add a `SaveActiveWebviewNodes` intent.
- **Double lifecycle transition**: If a node is promoted and demoted in the same frame, the intents cancel out. Reconciliation sees Cold node -- correct. *Mitigation*: lifecycle flags are not logged (per atomicity policy), so no persistence noise.
- **Webview creation requires rendering context**: The reconciliation pass needs `OffscreenRenderingContext` and `WindowRenderingContext`. *Mitigation*: reconciliation takes the same parameters as current `manage_lifecycle()`. No new threading required.

**Comparison**: Servoshell mixes state mutation and side effects in `handle_interface_commands()` -- no separation. Verso separates via message channels. Firefox separates via process boundaries (parent decides, child executes). Phase C gives graphshell verso-level separation without message channels.

**Success criteria:**

- No direct `app.promote_node_to_active()` / `app.demote_node_to_cold()` calls outside `apply_intent()`.
- `manage_lifecycle()` returns intents, doesn't mutate app state directly.
- Webview create/destroy still works through reconciliation.

**Testable invariants:**

- `grep -rn 'promote_node_to_active\|demote_node_to_cold' webview_controller.rs gui.rs` returns zero direct calls (all go through `GraphIntent` variants).
- `manage_lifecycle()` signature returns `Vec<GraphIntent>` (compile-time enforced).

### Phase D: Delete legacy paths and close UI loose ends

**Dependency**: Phase C complete

**Tasks:**

1. Delete legacy fullscreen-detail fallback path (`gui.rs:1282-1328` else branch).
2. Delete `UserInterfaceCommand::{Go, Back, Forward, Reload}` variants from the desktop path -- toolbar already routes directly to per-webview calls (`gui.rs:707-767`). **EGL/embedded impact**: `egl/app.rs:386-416` uses these variants for `load_uri()`, `reload()`, `go_back()`, `go_forward()`. These must be refactored to direct webview calls at the same time, or the EGL path breaks. Phase D is **not desktop-only** — it requires equivalent refactoring in `egl/app.rs`.
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

1. ~~Phase A is the blocker~~ -- **Design resolved** (two-phase apply decided, identity invariants documented, lifecycle vocabulary defined). Implementation not yet started.
2. **Delegate ordering**: Need to confirm ordering guarantees between `notify_url_changed`, `notify_page_title_changed`, and `notify_history_changed` under redirects and SPA transitions. Can validate empirically during Phase B.
3. **Close-tab policy**: What does closing a webview tile mean for the graph node? Current behavior: demote to Cold. Design docs suggest mode-dependent (delete vs hide). For now, keep current Cold demotion; defer mode-pluggable policy to later.
4. ~~Stop button API~~ -- **Resolved**: `webview.stop()` does not exist in Servo's `WebView` API (verified Feb 16). Decision: remove the stop button in Phase D.
5. **AccessKit / webview accessibility**: Blocked on Servo exposing an accessibility tree API to embedders. No graphshell-side work possible until then. Noted as known limitation in Phase D.

## Guardrails (from prior debugging)

- Do not add runtime instrumentation to diagnose deterministic code-structure failures. Read the code instead.
- Do not patch around the command queue model. Replace it.
- Prefer event-driven Servo callbacks over polling when the model says they are the authority.
- Check tile/lifecycle interactions before concluding a single-path fix is sufficient.
- Timebox diagnostics: one round of logging, then move to a testable change.
