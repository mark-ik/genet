# Architecture Reconciliation (2026-02-12)

## The Contradiction

Two incompatible architectural proposals exist in the design docs:

### Proposal A: Continuous Zoom Model
**Document**: `2026-02-12_unified_architecture_plan.md`
**Core Idea**: Single continuous spatial canvas where you zoom into nodes to see their web content rendered as textures ON the node surface.

```
Far zoom:  [●]──[●]──[●]    (nodes as icons)
Mid zoom:  [preview]         (thumbnail visible)
Near zoom: [┌────────┐]      (webview fills viewport, rendered on node)
           [│ content │]
           [└────────┘]
```

**Key Features**:
- Thermal states (Active/Warm/Cold) based on camera distance
- Webviews rendered to textures composited onto node geometry
- No separate "views" — just zoom levels
- Nodes own their `Option<WebView>` directly

**Problem**: This was NOT what the user proposed. This was my interpretation without verification.

### Proposal B: egui_tiles Tiling
**Document**: `2026-02-12_servoshell_inheritance_analysis.md`
**Core Idea**: Use egui_tiles for layout management. Graph is one pane/tile, webviews are separate panes/tiles that can be arranged/docked.

```
┌─────────────┬─────────────┐
│   Graph     │   WebView   │
│   [●]──[●]  │  ┌────┐     │
│      │      │  │page│     │
│   [●]──[●]  │  └────┘     │
├─────────────┼─────────────┤
│  WebView    │  WebView    │
│  ┌────┐     │  ┌────┐     │
│  │page│     │  │page│     │
└─────────────┴─────────────┘
```

**Key Features**:
- Graph and webviews are sibling tiles in egui_tiles layout
- Drag-and-drop tiling/docking comes free
- Tab management automatic
- Multiple simultaneous webviews in separate panes
- Each webview tile gets its own `OffscreenRenderingContext`

**Alignment**: This matches what the user actually asked for ("viewports containing webviews within the context of the graph view").

### Historical Context

From archived docs (`2026-02-10_crate_refactor_plan.md`):
- "Replace exclusive view toggle with `egui_tiles` tiled layout"
- "egui_tiles enables: graph AND detail view (simultaneous)"

From `README.md`:
- "Split view: egui_tiles for graph + detail simultaneously (Phase 2)"

From `ARCHITECTURAL_OVERVIEW.md`:
- "Split view (egui_tiles)" listed as future work

**Conclusion**: egui_tiles has been the plan all along.

## Why Proposal A Doesn't Match the Request

The user said:
> "I guess what I think makes sense is to use egui_tiles to organize viewports containing webviews within the context of the graph view. those viewports would reuse servoshell's tab ui although everything would be wired through the graph ultimately."

This explicitly describes:
1. ✅ egui_tiles for organization
2. ✅ Viewports (separate panes/tiles) containing webviews
3. ✅ Reusing servoshell's tab UI
4. ✅ Graph as source of truth (wiring)

Proposal A (continuous zoom) does NONE of these. It:
- ❌ Doesn't use egui_tiles
- ❌ Doesn't have separate viewport panes — just zooming into nodes
- ❌ No opportunity to reuse servoshell's tab UI (no separate tabs/panes)
- ❌ Fundamentally different interaction model

## Technical Feasibility Check

### Proposal A (Zoom) Challenges
- **Rendering webviews as textures on nodes**: Possible but requires exposing `OffscreenRenderingContext::texture_id` (currently private) or implementing custom `RenderingContext` that exposes it
- **Smooth zoom UX**: Every node at "warm" zoom needs a webview → resource intensive
- **Interaction**: How do you click links on a zoomed node while also dragging the graph? Input routing becomes complex

### Proposal B (egui_tiles) Verified
- ✅ egui_tiles 0.14.x compatible with egui 0.33.3 (confirmed)
- ✅ Multiple `OffscreenRenderingContext`s supported by Servo API (verified in source)
- ✅ `render_to_parent_callback()` pattern works per-tile (servoshell already uses it)
- ✅ `browser_tab()` widget directly reusable in `Behavior::tab_ui()`
- ✅ Graph can be one pane while webviews are other panes

## Reconciled Architecture: Proposal B (egui_tiles)

**Decision**: Pursue Proposal B. It matches:
- The user's explicit request
- Historical documentation direction
- Technical feasibility analysis
- Minimal divergence from existing codebase

### Disposition of Proposal A

`2026-02-12_unified_architecture_plan.md` should be:
- **Archived or marked as "Alternative approach — not pursued"**
- Or replaced with an egui_tiles-aligned plan

The continuous zoom approach might be interesting future work, but it's not what was requested and not what the historical docs point toward.

## Updated Architecture: egui_tiles Integration

### Structure

```rust
pub struct Gui {
    tiles_tree: egui_tiles::Tree<TileKind>,
    tile_behavior: GraphshellTileBehavior,
    
    // One rendering context per visible webview tile
    rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    
    // Favicon cache remains per-webview
    favicon_textures: HashMap<WebViewId, (TextureHandle, SizedTexture)>,
    
    // Graph state
    graph_app: GraphBrowserApp,
    
    // ... other state ...
}

pub enum TileKind {
    Graph,                    // The force-directed spatial canvas
    WebView(NodeKey),         // A webview displaying this node's content
}

impl egui_tiles::Behavior<TileKind> for GraphshellTileBehavior {
    fn tab_title_for_pane(&mut self, pane: &TileKind) -> WidgetText {
        match pane {
            TileKind::Graph => "Graph".into(),
            TileKind::WebView(node_key) => {
                // Query graph_app.graph.get_node(node_key) for title
            }
        }
    }

    fn pane_ui(&mut self, ui: &mut Ui, tile_id: TileId, pane: &mut TileKind) -> UiResponse {
        match pane {
            TileKind::Graph => {
                // Existing render::render_graph(ctx, graph_app)
            }
            TileKind::WebView(node_key) => {
                // Get or create OffscreenRenderingContext for this node
                // Get webview from graph_app via node_key
                // Call webview.paint()
                // Register egui PaintCallback with render_to_parent_callback()
            }
        }
    }

    fn tab_ui(&mut self, tiles, ui, id, tile_id, state) -> Response {
        // Reuse existing browser_tab() function
        // Pull data from graph_app.graph instead of WebViewCollection
    }
}
```

### Migration Path (Aligned with Proposal B)

1. **Add egui_tiles dependency** (Cargo.toml)
2. **Define TileKind enum** (Graph + WebView variants)
3. **Implement minimal Behavior**:
   - `pane_ui()` dispatches to graph rendering or webview rendering
   - `tab_title_for_pane()` queries graph for node title
4. **Per-tile rendering contexts**:
   - Create one `OffscreenRenderingContext` per visible WebView tile
   - Wire `paint()` + `render_to_parent_callback()` per tile
5. **Graph-driven tile management**:
   - Clicking a graph node creates/focuses its WebView tile
   - Navigation in a webview creates new node + new tile
   - Closing a tile updates graph state

### What Changes from Current Code

| Current | egui_tiles |
|---------|------------|
| `View` enum (Graph/Detail) exclusive toggle | `egui_tiles::Tree` with multiple simultaneous tiles |
| Monolithic `update()` with view branching | `Behavior::pane_ui()` dispatch per tile |
| Single shared `OffscreenRenderingContext` | One context per visible webview tile |
| Tab bar only in detail view | Tab bar per container, managed by egui_tiles |
| `webview_controller.rs` manage_lifecycle | Tile creation/destruction via egui_tiles API |

### What Stays the Same

- **All Servo integration**: `running_app_state.rs`, `window.rs`
- **Graph data structures**: `graph/mod.rs`, `GraphBrowserApp`
- **Persistence**: fjall + redb + rkyv unchanged
- **Physics**: Worker thread, KD-tree, all intact
- **Platform layer**: `headed_window.rs`, `platform/` unchanged

## Action Items

1. **Archive or deprecate** `2026-02-12_unified_architecture_plan.md`
2. **Update INDEX.md** to list the new docs under implementation_strategy
3. **Rename** `2026-02-12_servoshell_inheritance_analysis.md` to something like `2026-02-12_egui_tiles_architecture.md` (since it's now the official plan)
4. **Create a spike**: Minimal egui_tiles integration to prove the concept

## Why This Matters

Without reconciliation, future work could go in the wrong direction. The continuous zoom idea (Proposal A) is architecturally interesting but:
- Not what was requested
- More complex to implement
- Doesn't align with historical documentation
- Requires more Servo API changes

The egui_tiles approach (Proposal B):
- Matches the explicit user request
- Aligns with historical docs
- Technically verified as feasible
- Reuses more existing code
- Clearer migration path from current codebase
