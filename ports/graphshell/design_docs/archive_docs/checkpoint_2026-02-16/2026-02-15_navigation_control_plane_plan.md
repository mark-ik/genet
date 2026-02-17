# Navigation Control Plane Plan (Servo Host vs Graphshell Semantics)

**Date**: February 15, 2026  
**Status**: Draft (validation-first, no-legacy policy compliant)  
**Audience**: Graphshell architecture and runtime maintainers

## Purpose

Document the core architectural subject from recent debugging work:

1. Why omnibar/search/navigation regressions keep recurring.
2. Which constraints are validated vs still assumptions.
3. How to remove servoshell control-path leakage without indefinite patching.

This project has no user/legacy compatibility obligations. Choose correctness and deletion over compatibility shims.

## Problem Statement

Graphshell is currently implemented inside `ports/graphshell`, and runtime control paths still inherit servoshell assumptions:

1. Window-global navigation targets (`active_webview()`).
2. Heuristic synchronization between tile-active and window-active webview.
3. Lifecycle paths that destroy/recreate webviews and use mixed creation contexts.

These assumptions conflict with Graphshell's intended multi-pane spatial browsing model.

## Validated Constraints (Code-Backed)

### 1) Window-global command dispatch is still active

Navigation commands route through `UserInterfaceCommand` and execute on `active_webview()`:

- `Go` -> `active_webview().load(...)`
- `Back` -> `active_webview().go_back(1)`
- `Forward` -> `active_webview().go_forward(1)`
- `Reload` -> `active_webview().reload()`

Reference: `ports/graphshell/window.rs` (`handle_interface_commands`).

### 2) Two independent activation systems exist

- Tile model: `tiles_tree.active_tiles()` / `active_webview_tile_node(...)`
- Window model: `webview_collection.active_id()` / `active_webview()`

These are synchronized heuristically (including per-frame activation).

References:

- `ports/graphshell/desktop/gui.rs` (`active_webview_tile_node`, `active_webview_tile_rects`)
- `ports/graphshell/window.rs` (`active_webview`, `activate_webview`)

### 3) Lifecycle destroy/recreate path is real

Switching to graph context can close all webviews and later recreate them.

Reference: `ports/graphshell/desktop/webview_controller.rs` (`manage_lifecycle`).

### 4) Mixed creation paths are real

Two code paths create webviews with different context sources:

1. `create_toplevel_webview(...)` (default window context path in lifecycle restore)
2. `create_toplevel_webview_with_context(...)` (per-tile offscreen context path)

References:

- `ports/graphshell/desktop/webview_controller.rs`
- `ports/graphshell/desktop/gui.rs` (`ensure_webview_for_node`)

### 5) Direct per-webview navigation is possible in Servo API usage

Direct `webview.load(url)` by explicit `WebViewId` is already used elsewhere.

Reference: `ports/graphshell/running_app_state.rs` (`handle_webdriver_load_url`).

## Proven Failure Mechanisms (Deterministic from Code Structure)

### Mechanism 1: Wrong-target dispatch for omnibar navigation

**Code path:**
1. User types URL in omnibar and presses Enter
2. `handle_address_bar_submit` → `queue_user_interface_command(Go(url))` ([webview_controller.rs:206-208](ports/graphshell/desktop/webview_controller.rs#L206-L208))
3. `handle_interface_commands` → `active_webview().load(url)` ([window.rs:419-420](ports/graphshell/window.rs#L419-L420))
4. `active_webview()` determined by `active_tile_rects.first()` ([gui.rs:1202-1205](ports/graphshell/desktop/gui.rs#L1202-L1205))

**Why it fails:**
- When omnibar has focus, omnibar itself is NOT a tile
- `active_tile_rects` contains webview tiles behind the unfocused omnibar input
- Command executes against whichever tile happens to be first in rect list
- This is wrong-target execution, not timing-dependent or race condition

**No runtime tracing needed:** The logic structure guarantees wrong-target when omnibar is focused.

### Mechanism 2: Dual rendering context creation paths

**Two paths create webviews with different OpenGL contexts:**

1. **Lifecycle restore path:** `manage_lifecycle` → `create_toplevel_webview(state, url)` ([webview_controller.rs:137](ports/graphshell/desktop/webview_controller.rs#L137))
   - Uses window's default rendering context
   
2. **Tile ensure path:** `ensure_webview_for_node` → `create_toplevel_webview_with_context(state, url, render_context)` ([gui.rs:1500](ports/graphshell/desktop/gui.rs#L1500))
   - Uses per-tile offscreen rendering context

**Why it fails:**
- Same `WebViewId` can be created with different rendering contexts depending on path
- Servo's compositor expects one context per webview for lifetime
- Switching contexts mid-lifecycle = OpenGL/graphics API undefined behavior
- Manifests as blank content, failed compositing, or rendering corruption

**No render logs needed:** The code structure creates the undefined behavior deterministically.

## Validation Methodology: Proof Over Instrumentation

For both mechanisms above, **adding runtime instrumentation is unnecessary** because:

1. **Code structure proves causation** - logic guarantees the failure modes exist
2. **Deductive analysis > empirical symptom tracking** - we know what's broken from reading code
3. **10-line fixes prove/disprove faster than 100-line logging infrastructure**

### Validation Approach: Phase 0 Proof Patches

Instead of instrumenting to measure symptoms, implement minimal fixes to prove mechanisms:

**Proof 1: Direct webview targeting for omnibar**
```rust
// In handle_address_bar_submit, bypass command queue:
if let Some(node_key) = active_webview_tile_node(&graph_app.tiles_tree)
    && let Some(wv_id) = graph_app.get_webview_for_node(node_key)  
    && let Some(webview) = window.webview_by_id(wv_id)
{
    webview.load(url);  // DIRECT CALL - no queue, no wrong-target
}
```

**Test:** Does omnibar Enter now navigate correctly? If yes, wrong-target dispatch was the blocker.

**Proof 2: Unified rendering context creation**
```rust
// In manage_lifecycle, use same context path as ensure_webview:
let render_context = graph_app.get_or_create_rendering_context_for_node(node_key);
let webview = window.create_toplevel_webview_with_context(state, url, render_context);
```

**Test:** Does blank content disappear? If yes, dual context paths were the blocker.

These are **deterministic fixes that prove causation**, not symptom measurements.

---

## Update After Proof Attempts

The proof patches did not resolve the observed navigation failures in practice. This means the mechanisms above remain hypotheses, not confirmed root causes. The current conclusion is to prioritize the event-driven delegate model (especially `notify_url_changed`) and remove polling-based node creation before further host-level decontamination. See NAVIGATION_NEXT_STEPS_OPTIONS.md for the updated decision framing.

## Decision Framework

### Option A: Keep patching around current command model

- Lowest immediate churn.
- High recurring bug tax.
- Rejected as long-term approach.

### Option B: Control-plane decontamination inside current host (recommended)

- Keep platform/event/render host runtime for now.
- Remove servoshell navigation assumptions from Graphshell paths.
- Make all navigation/search explicitly target tile-resolved `WebViewId`/`NodeId`.

### Option C: Full custom host rewrite now

- Architecturally cleanest end-state.
- Highest short-term risk/cost (windowing/input/render lifecycle parity).
- Better as follow-on once semantics are explicit and test-covered.

## Why Direct Calls Are Correct (Command Queue Discipline Analysis)

### Servo's Default Mode is Single-Process

Servo runs in **single-process, multi-threaded mode by default**. Multiprocess mode (`-M` flag) is optional for testing isolation.

**Evidence:**
- [event_loop.rs:116-119](components/constellation/event_loop.rs#L116-L119): `if opts::get().multiprocess { spawn_in_process() } else { spawn_in_thread() }`
- [generic_channel.rs:554-558](components/shared/base/generic_channel.rs#L554-L558): Channels use `crossbeam_channel` in single-process, IPC only when `-M` enabled

### In Single-Process Mode (Default)

- Constellation is a **thread**, not a process
- `webview.load(url)` → `constellation_proxy.send()` → **crossbeam_channel** (in-memory MPSC)
- No serialization, no IPC, no process boundaries
- Thread safety already guaranteed by crossbeam

### Command Queue Provides No Additional Ordering

**Direct call:**
```rust
webview.load(url);  // → constellation_proxy.send() → crossbeam_channel
```

**Queue-based:**
```rust
queue.push(Go(url));  // → Later: active_webview().load(url) → constellation_proxy.send() → crossbeam_channel
```

**Same crossbeam_channel, same ordering, same thread safety.** The embedder command queue is just indirection with no concurrency benefit.

### When Queue IS Useful

Command queue exists for **embedder-level window coordination** (servoshell multi-window model):
- `ReloadAll` - coordinate multiple webviews at window level
- Window-level state consistency before Servo operations

For **independent per-webview operations** (omnibar navigating specific tile), queue provides:
- ❌ No thread safety benefit (crossbeam already handles it)
- ❌ No ordering benefit (single-webview operation, no coordination needed)
- ✅ Only latency + wrong-target risk

### Conclusion: Direct Calls Are Correct for Tile-Specific Operations

Bypassing the command queue for tile-targeted navigation is:
- **Simpler** - caller has webview reference, just calls method
- **Faster** - no queue allocation, no dispatch match, no defensive cloning  
- **Safer** - eliminates wrong-target bugs from `active_webview()` routing
- **Correct** - Servo's threading model already provides all necessary guarantees

Keep queue only for multi-window coordination (`ReloadAll`), bypass for tile-specific operations.

## Recommended Plan (No-legacy, Validation-First)

### Phase 0: Proof patches (validate mechanisms)

**Implementation:**

1. **Omnibar direct targeting:**
   - Modify `handle_address_bar_submit` in `webview_controller.rs`
   - Resolve `active_webview_tile_node` → `get_webview_for_node` → `webview_by_id`
   - Call `webview.load(url)` directly, bypass `queue_user_interface_command(Go)`
   
2. **Unified webview creation context:**
   - Modify `manage_lifecycle` in `webview_controller.rs`
   - Use `get_or_create_rendering_context_for_node` + `create_toplevel_webview_with_context`
   - Match context creation logic from `ensure_webview_for_node`

**Success criteria:**

1. Omnibar Enter navigates correct tile (not wrong-target or no-op)
2. Page content renders visibly after navigation (not blank)
3. No new regressions in tile switching or multi-window operations

### Phase 1: Delete window-global navigation dispatch (~100 lines)

**If Phase 0 succeeds, this is the right next step** (not extraction, not more patching):

1. **Delete `UserInterfaceCommand::{Go, Back, Forward, Reload}` variants**
   - These are servoshell single-active-webview patterns
   - Graphshell UI should target explicit tile/webview instead
   
2. **Remove `handle_interface_commands` dispatch logic for these commands**
   - Keep only `ReloadAll` for multi-window coordination
   - All tile-specific navigation uses direct `webview.method()` calls
   
3. **Update UI handlers to resolve tile → webview directly**
   - Omnibar: already done in Phase 0
   - Keyboard shortcuts (Back/Forward): resolve active tile first
   - Context menus: already have tile context
   
**Estimated deletion:** ~100 lines of command enum + dispatch + queue management

### Phase 2: Remove heuristic per-frame activation

### Phase 0 Validation (Proof of Mechanism)

**Manual testing (sufficient for proof):**
1. Launch graphshell with multi-tile layout
2. Type URL in omnibar while tile B is focused
3. Press Enter
4. **Expected:** Tile B navigates to URL (not tile A, not no-op)
5. Verify content renders visibly (not blank)

**Pass criteria:** Issue "omnibar Enter does nothing" and "content blank" are fixed.

### Phase 1 Validation (Deletion Safety)

**Integration tests (before accepting deletion):**
1. Omnibar URL submit navigates correct tile
2. Key Insights from Analysis

### 1. Deterministic Failure > Plausible Failure

The omnibar and rendering failures are **proven by code structure**, not "plausible pending trace evidence."

- Wrong-target dispatch: logic guarantees wrong webview when omnibar focused
- Dual context: code creates same WebViewId with different OpenGL contexts
- No need to instrument/measure what code analysis already proves

### 2. Proof Patches > Instrumentation

**10-line direct call fixes prove mechanisms faster than 100-line logging infrastructure.**

Runtime instrumentation gives symptom data (how often bug X occurs), not solution data (is this the right fix). For deterministic failures, implement minimal fix and test - that's the validation.

### 3. Single-Process Default Matters

Servo's default mode is **single-process multi-threaded**, not multiprocess. The embedder command queue provides **window-level coordination**, not thread safety (crossbeam_channel already handles that).

Direct `webview.load()` calls are simpler, faster, and eliminate wrong-target bugs with no concurrency cost.

### 4. Deletion > Extraction (for navigation fixes)

Extraction is valuable for **testing isolation** and **future porting**, but not a **prerequisite for working navigation**.

- Phase 1 deletion (~100 lines) fixes navigation immediately
- Extraction can be deferred until testing/porting needs justify the work
- "Working software first, then optional architectural refinement"

### 5. Servo API Already Supports Multi-Pane

The constraint is **not in Servo's embedder API** - `WebView::load(self.id(), url)` explicitly targets by WebViewId.

The constraint is **graphshell's inherited servoshell dispatch layer** - `handle_interface_commands` routes to `active_webview())`.

Servo supports graphshell's multi-pane model natively; graphshell's adapter code just needs to stop routing through single-active patterns.

## Immediate Next Actions

1. **Implement Phase 0 proof patches** (direct targeting + unified context)
2. **Manual test with repro cases** (omnibar Enter, content rendering)
3. **If Phase 0 succeeds:** Proceed directly to Phase 1 deletion (not extraction)
4. **Add integration tests** before accepting Phase 1 deletion
5. **Re-evaluate extraction** after Phase 1-2 complete (may be unnecessary)
- Tile switching doesn't break activation
- Dialogs (DevTools, search) still route input correctly
- Window focus transitions work
- Painting/compositing remains stable

### Phase 2 Validation (Activation Cleanup)

**Focus behavior tests:**
1. Click between tiles → verify keyboard input routes to clicked tile
2. Open dialog → verify dialog receives input, not background tile  
3. Window focus lost/regained → verify active tile remains correct
4. DevTools open → verify DevTools receives input when focused

**Pass criteria:** No input routing regressions, frame-by-frame activation removed
   
3. **Test focus-dependent behaviors:**
   - Keyboard input routing
   - Dialog interactions
   - DevTools focus

### Phase 3 (Optional): Extract graphshell-core IF justified

**After Phase 0-2 complete, re-evaluate whether extraction is necessary:**

Arguments FOR extraction:
- Testing isolation (unit test graph semantics without Servo runtime)
- Future porting to different embedders  
- Clear ownership boundary (graph logic vs host adapter)

Arguments AGAINST extraction (do simpler thing):
- Phase 1-2 already remove servoshell assumptions from navigation
- Keeping code in-place reduces refactor risk
- Can extract later if testing/porting needs materialize

**Decision criteria:** If navigation works correctly after Phase 1-2, extraction becomes "nice to have" not "prerequisite for working software."

## Required Validation Artifacts

Before accepting each phase:

1. Deterministic integration tests for:
   - Omnibar URL submit in graph mode and webview mode.
   - `@query` search selection behavior (including no-match feedback).
   - Back/Forward/Reload against targeted tile.
2. Runtime trace for submit and execution target identity during repro.
3. Regression checklist for tile switching, dialogs, focus, and painting.

## Non-Goals

1. Preserving legacy servoshell command semantics for Graphshell UX.
2. Incremental compatibility shims for non-existent users.
3. Deferring control-plane cleanup in favor of feature-by-feature patches.

## Immediate Next Actions

1. Implement Phase 0 proof patch for omnibar direct targeted load.
2. Unify webview creation context strategy.
3. Add/extend integration tests around navigation target resolution.
4. If Phase 0 passes, proceed directly to Phase 1 deletion work.

