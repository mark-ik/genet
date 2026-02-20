# GraphShell iOS Port Plan (Archived 2026-02-20)

## Status: Deferred

**Rationale for deferral**: The Cross-Platform Sync and Extension Plan (2026-02-20) supersedes this with a lighter-weight approach: iOS users connect as sync clients (native list/edit UI), not full Graphshell ports. WKWebView preview functionality from Phase 1-2 is preserved as an optional enhancement layer in the sync architecture. This document remains useful as reference for:
- RendererId abstraction pattern (Phase 0) — reusable if iOS native rendering is desired later
- Cargo.toml cfg gates for platform-specific dependencies
- Delegate callback patterns

## Context (Original)

GraphShell currently supports desktop (Windows/macOS/Linux) via Servo and EGL-based platforms (Android/OHOS) via the `egl/` module. iOS is blocked for Servo due to Apple's App Store rule prohibiting custom JIT-enabled JavaScript engines (SpiderMonkey). However, using Apple's own WKWebView satisfies the rule: you are delegating JS execution to the platform engine.

The graph layer (petgraph, physics, persistence) is already platform-agnostic. The egui graph canvas renders via Metal through winit's iOS backend. The only Servo-specific surface is web content rendering in detail view. Replacing it with WKWebView (via the `wry` crate) is the iOS path.

This plan follows the architecture sketched in `2026-02-18_universal_node_content_model.md` (§3.2, §10 step 10), and the prior Tao+Wry discussion from `checkpoint_2026-01-29/COMPREHENSIVE_SYNTHESIS.md`.

---

## Phase 0 Reuse in Sync Client Architecture

The RendererId abstraction and iOS cfg gates from Phase 0 remain useful for the sync client approach if native content preview is desired. See 2026-02-20_cross_platform_sync_and_extension_plan.md §Platform-Specific Layering for context.

---

## Critical Files

| File | Role |
|---|---|
| `ports/graphshell/lib.rs` | Platform cfg gates — iOS must be added as exclusion from desktop |
| `ports/graphshell/app.rs` | `GraphBrowserApp` + `GraphIntent` — contains `servo::WebViewId` coupling |
| `ports/graphshell/window.rs` | `GraphSemanticEvent` — uses `WebViewId` in event variants |
| `ports/graphshell/Cargo.toml` | Dependency sections per target; needs iOS target section |
| `ports/graphshell/desktop/webview_controller.rs` | Servo webview lifecycle (must wrap behind abstraction) |
| `ports/graphshell/desktop/tile_compositor.rs` | Servo-specific OpenGL compositing loop |
| `ports/graphshell/desktop/tile_render_pass.rs` | Drives tile compositor — holds `OffscreenRenderingContext` |
| `ports/graphshell/desktop/tile_runtime.rs` | Servo `WebViewId` in webview tile management |
| `ports/graphshell/desktop/thumbnail_pipeline.rs` | Thumbnail capture via Servo rendering |

---

## Phase 0 — Platform Abstraction (Prerequisite) ✅ COMPLETE

**Goal:** Make `app.rs` and `window.rs` compile without `servo::WebViewId` as a direct dependency. This is the critical seam that makes iOS (and any future non-Servo renderer) possible.

**Completed 2026-02-19.** `cargo check -p graphshell` clean (warnings only, no errors).

### What was planned vs. what was done

The plan originally proposed a newtype `RendererId(u64)` with a conversion `From<WebViewId>` requiring a registry in the desktop layer. During implementation, a simpler approach was found: **a type alias on desktop**.

```rust
// app.rs (implemented)
#[cfg(not(target_os = "ios"))]
use servo::WebViewId;

#[cfg(not(target_os = "ios"))]
pub type RendererId = WebViewId;   // alias — zero cost, no conversion anywhere

#[cfg(target_os = "ios")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RendererId(u64);        // standalone opaque type for iOS
```

Because `RendererId = WebViewId` on desktop, the 107 `WebViewId` references across 16 desktop files required **zero changes** — the alias makes them transparent. Only the intent boundary changed.

### Changes made

**`app.rs`**

- `RendererId` type defined (alias on desktop, struct on iOS)
- `GraphBrowserApp` fields: `webview_to_node: HashMap<RendererId, NodeKey>`, `node_to_webview: HashMap<NodeKey, RendererId>`
- `GraphIntent` variants: `MapWebviewToNode`, `UnmapWebview`, `WebViewCreated`, `WebViewUrlChanged`, `WebViewHistoryChanged`, `WebViewScrollChanged`, `WebViewTitleChanged`, `WebViewCrashed` — field types changed to `RendererId`
- Methods: `map_webview_to_node`, `unmap_webview`, `get_node_for_webview`, `get_webview_for_node`, `webview_node_mappings` — signatures use `RendererId`
- `test_webview_id()` returns `RendererId`; iOS branch uses an atomic counter, so none of the 30+ callsites needed gating

**`window.rs`**

- Imports `RendererId` from `crate::app`
- `GraphSemanticEvent` variant fields changed to `RendererId` (construction sites unchanged — pass `WebViewId` which equals `RendererId` on desktop)

**`Cargo.toml`**

- Desktop dependency section updated from `not(any(android, ohos))` to `not(any(android, ohos, ios))` so Servo and egui are not pulled in for iOS targets.

**Unchanged** (type alias made them transparent):

- All 16 `desktop/` files (`gui.rs`, `tile_compositor.rs`, `semantic_event_pipeline.rs`, etc.)
- No registry, no conversion helpers, no `.into()` calls needed

---

## Phase 1 — iOS Build Target

**Goal:** `cargo check --target aarch64-apple-ios` reaches a clean compile (stubs okay).

### 1.1 — Update cfg gates in `lib.rs`

Current:
```rust
#[cfg(not(any(target_os = "android", target_env = "ohos")))]
pub(crate) mod desktop;
#[cfg(any(target_os = "android", target_env = "ohos"))]
mod egl;
```

Updated:
```rust
#[cfg(not(any(target_os = "android", target_env = "ohos", target_os = "ios")))]
pub(crate) mod desktop;
#[cfg(any(target_os = "android", target_env = "ohos"))]
mod egl;
#[cfg(target_os = "ios")]
mod ios;
```

Also update `pub fn main()` guard and any other desktop-gated items.

### 1.2 — Stub `ios/mod.rs`

```rust
pub fn main() {
    unimplemented!("iOS entry point not yet implemented")
}
```

### 1.3 — `Cargo.toml` — iOS dependency section

```toml
# Exclude Servo from iOS (uses WKWebView instead)
[target.'cfg(not(any(target_os = "android", target_env = "ohos", target_os = "ios")))'.dependencies]
# (Move existing desktop-only deps here, replacing the current not(android/ohos) section)
# egui, egui_graphs, petgraph, fjall, redb, rkyv, etc.

[target.'cfg(target_os = "ios")'.dependencies]
wry = { version = "0.47", default-features = false, features = ["metal"] }
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["std"] }
objc2-ui-kit = { version = "0.3", features = ["UIApplication", "UIViewController", "UIView", "UIWindow"] }
egui = "0.33"
egui-winit = { version = "0.33", features = [] }
egui_glow = "0.33"
egui_graphs = { version = "0.29", features = ["events"] }
egui_tiles = { version = "0.14", features = ["serde"] }
petgraph = { version = "0.8", features = ["serde-1"] }
fjall = "3"
redb = "3"
rkyv = { version = "0.8", features = ["std"] }
winit = { workspace = true }
```

Note: `wry` 0.47 supports iOS natively via WKWebView. No GStreamer, no SpiderMonkey, no surfman needed.

### 1.4 — Crate type for iOS framework

```toml
[lib]
name = "graphshell"
path = "lib.rs"
# iOS requires staticlib for embedding in Xcode; desktop uses rlib
crate-type = ["rlib", "staticlib"]
```

---

## Phase 2 — WKWebView Renderer via wry

**Goal:** iOS detail view shows web pages using WKWebView positioned as a native UIView overlay above the egui Metal surface.

### 2.1 — `ios/wry_renderer.rs`

```rust
pub(crate) struct WryRendererState {
    /// Map from RendererId to wry WebView instance
    webviews: HashMap<RendererId, wry::WebView>,
    next_id: u64,
    /// UIView parent handle for positioning overlays
    parent_view: raw_window_handle::RawWindowHandle,
}

impl WryRendererState {
    pub fn create_webview(&mut self, url: &str, node: NodeKey) -> RendererId { ... }
    pub fn destroy_webview(&mut self, id: RendererId) { ... }
    pub fn navigate(&mut self, id: RendererId, url: &str) { ... }
    /// Position the WKWebView UIView to match tile rect (overlay model)
    pub fn set_rect(&mut self, id: RendererId, rect: egui::Rect, scale: f32) { ... }
    pub fn title_for(&self, id: RendererId) -> Option<String> { ... }
    pub fn url_for(&self, id: RendererId) -> Option<String> { ... }
}
```

**Overlay positioning**: `wry` creates each `WebView` with a `UIView` in a specified parent `UIView`. The iOS shell (Phase 3) provides the root `UIView` as parent. `set_rect()` calls `setFrame:` on the `WKWebView`'s `UIView` using `objc2` to position it over the tile area in the egui Metal layer.

**Event callbacks**: wry's navigation and title-change handlers emit `RendererId`-keyed events into a `crossbeam_channel::Sender<GraphSemanticEvent>` that the main loop drains into `GraphIntent`s.

### 2.2 — iOS-side `GraphSemanticEvent` emission

The wry callbacks cannot call `GraphBrowserApp` directly (wrong thread/ownership model). Use the same `crossbeam_channel` pattern as the desktop `semantic_event_pipeline.rs`. Add to `ios/event_bridge.rs`:

```rust
pub(crate) fn make_semantic_event_channel() -> (Sender<GraphSemanticEvent>, Receiver<GraphSemanticEvent>)
```

### 2.3 — Tile rect sync

In the iOS main loop frame tick, after the egui graph canvas renders, call:
```rust
for (node_key, tile_rect) in active_webview_tile_rects(&tiles_tree) {
    if let Some(renderer_id) = app.get_webview_for_node(node_key) {
        wry_state.set_rect(renderer_id, tile_rect, scale_factor);
    }
}
```

This is the iOS equivalent of `tile_compositor.rs`'s composite pass — instead of blitting OpenGL textures, it repositions native UIViews.

---

## Phase 3 — iOS Event Loop + Touch UI

**Goal:** egui graph canvas renders on real iOS hardware with touch gestures; toolbar is touch-adapted.

### 3.1 — `ios/app.rs` — winit iOS event loop

```rust
pub fn start() {
    let event_loop = winit::event_loop::EventLoop::new().unwrap();
    let window = winit::window::WindowBuilder::new()
        .with_title("GraphShell")
        .build(&event_loop).unwrap();

    let (painter, egui_ctx) = setup_egui_metal(&window);
    let store = GraphStore::open(data_dir()).expect("persistence");
    let mut graph_app = GraphBrowserApp::new();
    let mut wry_state = WryRendererState::new(window.raw_window_handle());
    let (sem_tx, sem_rx) = make_semantic_event_channel();

    event_loop.run(move |event, elwt| {
        handle_winit_event(event, elwt, &mut graph_app, &mut wry_state, &sem_rx,
                           &painter, &egui_ctx, &mut tiles_tree);
    });
}
```

### 3.2 — `ios/touch_input.rs` — graph touch gestures

Map winit iOS touch/gesture events to `GraphAction` (the existing testable enum from `input/mod.rs`):

| Gesture | Graph action |
|---|---|
| Tap on node | `SelectNode` |
| Tap on empty space | `ClearSelection` |
| Double-tap on node | `OpenNodeInDetail` (switch to detail view) |
| Long-press on node | Radial palette (future) |
| Pinch on graph canvas | Zoom (egui_graphs camera) |
| Two-finger pan | Pan graph canvas |
| Swipe right in detail view | Navigate back |
| Swipe left | Navigate forward |

The `apply_actions()` function in `input/mod.rs` is already platform-agnostic — reuse directly.

### 3.3 — `ios/toolbar_ios.rs` — bottom toolbar

iOS convention: navigation controls at the bottom (thumb reach). Use egui `TopBottomPanel::bottom()`:

```
[ ← ] [ → ] [ ▼ URL bar ▼ ] [ ⬡ Graph ] [ + ]
```

- URL bar: tap to activate, shows keyboard
- Graph toggle: switch graph ↔ detail view (existing `is_graph_view` bool)
- Back/forward: call `wry_state.navigate()` with `go_back()`/`go_forward()` — wry exposes these

### 3.4 — Keyboard avoidance

winit on iOS emits keyboard-show events. When the software keyboard appears, shrink the egui viewport to avoid occlusion. Read `safe_area_insets` via `objc2-ui-kit` `UIWindow.safeAreaInsets`.

---

## Phase 4 — Xcode Wrapper + Distribution

**Goal:** A real `.ipa` that runs on an iPhone.

### 4.1 — Xcode project structure

```
GraphShell.xcodeproj/
├── GraphShell/
│   ├── AppDelegate.swift      # UIApplication entry; calls graphshell_start()
│   ├── ViewController.swift   # UIViewController; owns the Metal view
│   └── Info.plist
└── Frameworks/
    └── graphshell.a           # Built by cargo
```

Swift entry:
```swift
@_silgen_name("graphshell_start")
func graphshell_start()

class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(_ app: UIApplication, didFinishLaunching ...) -> Bool {
        DispatchQueue.global().async { graphshell_start() }
        return true
    }
}
```

`graphshell_start` is an `extern "C"` fn in `ios/app.rs`.

### 4.2 — Build script

```bash
#!/bin/bash
cargo build --release --target aarch64-apple-ios
cargo build --release --target aarch64-apple-ios-sim  # simulator (M1)

xcodebuild -project GraphShell.xcodeproj \
           -scheme GraphShell \
           -configuration Release \
           -archivePath build/GraphShell.xcarchive \
           archive
```

### 4.3 — Required: Apple Developer account

- $99/year Apple Developer Program membership
- Provisioning profile (development for device testing, distribution for App Store)
- App ID: `com.graphshell.app`
- **No App Store review concern for WKWebView** — this is the required Apple-sanctioned path

---

## Effort Summary

| Phase | Scope | Est. effort |
|---|---|---|
| **0 — Platform abstraction** | Replace `servo::WebViewId` with `RendererId` across ~10 files | 1 week |
| **1 — iOS build target** | cfg gates, Cargo.toml, stub module, `cargo check` passes | 3-4 days |
| **2 — WKWebView renderer** | wry integration, overlay positioning, event bridge | 2 weeks |
| **3 — Touch UI** | winit touch gestures, iOS toolbar, keyboard avoidance | 1.5 weeks |
| **4 — Xcode wrapper** | Swift shim, build script, signing | 3-4 days |
| **Total** | | **~6 weeks** |

Phase 0 also benefits desktop: once `RendererId` exists, adding WebView2 (Windows compat fallback), WebKitGTK (Linux), or PDF/image renderers follows the same pattern without touching `app.rs`.

---

## Verification

1. **After Phase 0**: `cargo test` — all 137 existing tests pass; `cargo build` for Windows unchanged.
2. **After Phase 1**: `cargo check --target aarch64-apple-ios` exits 0.
3. **After Phase 2**: On macOS simulator, URLs load in WKWebView overlay; `GraphSemanticEvent::UrlChanged` fires; graph node title/URL updates correctly.
4. **After Phase 3**: Pinch-to-zoom works on graph canvas; tap selects node; toolbar navigates back/forward.
5. **After Phase 4**: App runs on physical iPhone; graph persists across restarts (fjall + redb in iOS app sandbox).

---

## Key Dependencies

- `wry` 0.47+ — Tauri's cross-platform webview crate; WKWebView on Apple platforms
- `objc2-ui-kit` — UIView frame manipulation for overlay positioning
- `winit` (already a dep) — iOS event loop via CAMetalLayer/UIKit backend
- Apple Developer account ($99/yr) — required for device testing and distribution
