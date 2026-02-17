# Servoshell Inheritance Analysis

## What Graphshell Currently Uses from Servoshell

### Core Infrastructure (Keep These)

| Module | Purpose | Status |
|--------|---------|--------|
| **`running_app_state.rs`** | Servo integration, embedder delegate, preference management, WebViewCollection | ✅ **Keep** — Fundamental Servo integration layer |
| **`window.rs`** | `ServoShellWindow` - webview management, window lifecycle, paint coordination | ✅ **Keep** — Core abstraction over Servo WebViews |
| **`parser.rs`** | URL parsing (`location_bar_input_to_url`, `parse_url_or_filename`) | ✅ **Keep** — Utility functions |
| **`desktop/headed_window.rs`** | Platform window integration (winit, rendering context, input dispatch) | ✅ **Keep** — Platform abstraction |
| **`desktop/headless_window.rs`** | Headless window for testing | ✅ **Keep** — Testing infrastructure |
| **`platform/`** | Platform-specific code (macos, windows, linux, android) | ✅ **Keep** — Platform layer |
| **`egl/`** | EGL rendering support | ✅ **Keep** — Alternative rendering path |
| **`prefs.rs`** | CLI argument parsing and preferences | ✅ **Keep** (extend as needed) |
| **`webdriver.rs`** | WebDriver protocol support | ✅ **Keep** — Testing/automation |
| **Panic/crash handlers** | `panic_hook.rs`, `crash_handler.rs`, `backtrace.rs` | ✅ **Keep** — Error handling |
| **`resources.rs`** | Resource file handling | ✅ **Keep** — Asset loading |

### UI Layer (Fork/Replace These)

| Module | Servoshell Purpose | Graphshell Status | Recommendation |
|--------|-------------------|-------------------|----------------|
| **`desktop/gui.rs`** | Traditional browser UI with toolbar, tabs, address bar using egui | 🔶 **Heavily forked** — 794 lines vs servoshell's 682, added graph view branching | 🔀 **REPLACE** with egui_tiles-based layout |
| **`desktop/webview_controller.rs`** | N/A (doesn't exist in servoshell) | ⭐ **Graphshell-specific** — 313 lines of lifecycle management | 🔀 **REFACTOR** for per-tile rendering contexts |

### What `desktop/gui.rs` Includes That You WOULD Reuse

#### The Good Parts (Reusable Components)

```rust
// ✅ KEEP: Tab rendering widget (lines 242-327)
fn browser_tab(
    ui: &mut egui::Ui,
    window: &ServoShellWindow,
    webview: WebView,
    favicon_texture: Option<egui::load::SizedTexture>,
) { ... }
```
**Why keep**: This renders a tab with favicon + title + close button. It's exactly what you'd want in an egui_tiles `Behavior::tab_ui()`. Just needs to be wired to graph data instead of `WebViewCollection` ordering.

```rust
// ✅ KEEP: Favicon loading/caching (lines 826-857)
fn load_pending_favicons(
    ctx: &egui::Context,
    window: &ServoShellWindow,
    graph_app: &mut GraphBrowserApp,
    texture_cache: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
) { ... }

// ✅ KEEP: Image conversion (lines 781-824)
fn embedder_image_to_rgba(image: &Image) -> (usize, usize, Vec<u8>) { ... }
```
**Why keep**: Favicon pipeline already works. Just needs to run per-tile instead of globally.

```rust
// ✅ KEEP: Toolbar buttons (line 234-240)
fn toolbar_button(text: &str) -> egui::Button<'_> { ... }
```
**Why keep**: Minor styling utility, works fine.

#### The Parts to Replace

```rust
// ❌ REPLACE: Monolithic update() method (lines 337-700+)
pub(crate) fn update(
    &mut self,
    state: &RunningAppState,
    window: &ServoShellWindow,
    headed_window: &headed_window::HeadedWindow,
) {
    // 350+ lines of branching logic for:
    // - Drawing toolbar
    // - Drawing tab bar (only in detail view)
    // - Keyboard shortcuts
    // - Address bar handling
    // - Graph view vs detail view rendering
    // - Clear data dialogs
}
```
**Why replace**: This is the monolithic single-view renderer. egui_tiles will manage layout. You only need to implement `Behavior::pane_ui()` for each tile type.

```rust
// ❌ REPLACE: View branching (lines 638-696)
if is_graph_view {
    render::render_graph(ctx, graph_app);
} else {
    // render webview...
}
```
**Why replace**: egui_tiles removes the need for manual view switching. Each pane type renders itself.

```rust
// ❌ REPLACE: Single shared rendering context model
self.rendering_context  // ONE context for ALL webviews
```
**Why replace**: Need one `OffscreenRenderingContext` per visible webview tile for simultaneous rendering.

## Proposed egui_tiles Architecture

### New Structure

```
Gui {
    tiles_tree: egui_tiles::Tree<TileKind>,
    tile_behavior: GraphshellTileBehavior,
    rendering_contexts: HashMap<WebViewId, Rc<OffscreenRenderingContext>>,
    favicon_textures: HashMap<WebViewId, (TextureHandle, SizedTexture)>,
    graph_app: GraphBrowserApp,
}

enum TileKind {
    Graph,                          // The spatial force-directed canvas
    WebView(NodeKey),               // A webview showing a specific node's content
}

impl egui_tiles::Behavior<TileKind> for GraphshellTileBehavior {
    fn tab_title_for_pane(&mut self, pane: &TileKind) -> WidgetText {
        match pane {
            TileKind::Graph => "Graph".into(),
            TileKind::WebView(node_key) => {
                // Pull title from graph_app.graph.get_node(node_key)
            }
        }
    }

    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut TileKind) -> UiResponse {
        match pane {
            TileKind::Graph => {
                // Render the graph (existing render::render_graph)
            }
            TileKind::WebView(node_key) => {
                // Create/get OffscreenRenderingContext for this node's webview
                // Call webview.paint()
                // Register PaintCallback with render_to_parent_callback()
            }
        }
    }

    fn tab_ui(&mut self, tiles, ui, id, tile_id, state) -> Response {
        // Reuse the existing browser_tab() function here
        // Query graph_app for node data instead of WebViewCollection
    }
}
```

### What This Buys You

1. **Drag-and-drop tiling comes free** — users can arrange graph + webview panes however they want
2. **Tab management is automatic** — egui_tiles handles tab bar, active tab, close buttons
3. **Graph and webviews coexist naturally** — no more exclusive view branching
4. **Multiple simultaneous webviews** — each tile gets its own rendering context
5. **Layout persistence** — egui_tiles Tree is serializable

## Migration Path

### Phase 1: Proof of Concept

Create a minimal egui_tiles integration:

1. Define `TileKind` enum (Graph + WebView)
2. Implement `Behavior::pane_ui()` with:
   - Graph: Call existing `render::render_graph()`
   - WebView: Use existing `render_to_parent_callback()` pattern but per-tile
3. Build Tree from graph state on startup
4. See both graph and one webview tile on screen simultaneously

**Success criteria**: Graph canvas + one webview tile visible at the same time, no view toggle.

### Phase 2: Per-Tile Rendering Contexts

1. Create one `OffscreenRenderingContext` per visible webview tile
2. Wire `paint()` + `render_to_parent_callback()` into each WebView tile's `pane_ui()`
3. Test with 2-3 simultaneous webview tiles

**Success criteria**: Multiple webviews render simultaneously without clobbering.

### Phase 3: Tab Integration

1. Implement `tab_ui()` to reuse `browser_tab()` function
2. Wire favicon loading per-tile
3. Support close button → remove tile + webview

**Success criteria**: Tabs look like servoshell tabs, pull data from graph.

### Phase 4: Graph-Driven Layout

1. Rebuild tiles Tree from graph state when nodes are added/removed
2. Clicking a graph node creates/focuses its webview tile
3. Navigation in a webview creates a new node + tile

**Success criteria**: Graph is source of truth for what tiles exist.

## What You DON'T Need to Replace

- **Servo integration** — `running_app_state.rs` is solid
- **Window management** — `window.rs` `ServoShellWindow` abstraction works
- **Platform layer** — `headed_window.rs` + `platform/` are fine
- **Favicon pipeline** — already working, just needs per-tile wiring
- **Address bar widget** — can keep as a floating command palette or per-tile toolbar

## Key Insight

Servoshell's `Gui` is 90% layout management code solving "how to arrange toolbar + tabs + one webview viewport." egui_tiles solves that problem generically. You're replacing ~600 lines of layout code with ~150 lines of `Behavior` implementation, and gaining multi-viewport capability in the process.

The 10% you keep: `browser_tab()` widget, favicon loading, toolbar buttons — the actual rendering functions, not the layout logic.
