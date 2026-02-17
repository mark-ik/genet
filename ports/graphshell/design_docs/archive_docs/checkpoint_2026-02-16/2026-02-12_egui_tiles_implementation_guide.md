# egui_tiles Implementation Guide

**Document Type**: Step-by-step implementation guide with code examples
**Status**: Ready to implement
**Prerequisites**: Read [2026-02-12_servoshell_inheritance_analysis.md](2026-02-12_servoshell_inheritance_analysis.md)

---

## Overview

This guide provides concrete implementation steps for integrating egui_tiles into graphshell, with actual API calls based on egui_tiles 0.14.1 documentation.

### What You're Building

```
┌─────────────────────────────────────┐
│  egui_tiles::Tree<TileKind>        │
│                                     │
│  ┌─────────┬─────────┬─────────┐   │
│  │  Graph  │ WebView │ WebView │   │ ← Tiles managed by egui_tiles
│  │  [●─●]  │┌───────┐│┌───────┐│   │
│  │    │    ││ page  │││ page  ││   │
│  │  [●─●]  │└───────┘│└───────┘│   │
│  └─────────┴─────────┴─────────┘   │
└─────────────────────────────────────┘
```

---

## Phase 1: Add Dependency & Define Types

### Step 1.1: Update Cargo.toml

```toml
[dependencies]
egui_tiles = "0.14.1"  # Compatible with egui 0.33.x
```

**Verify compatibility**:
- graphshell uses egui 0.33.3
- egui_tiles 0.14.1 requires egui 0.33.0+
- ✅ Compatible

### Step 1.2: Define TileKind Enum

Create `ports/graphshell/desktop/tile_kind.rs`:

```rust
use crate::graph::NodeKey;

/// The kinds of panes (tiles) in graphshell
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TileKind {
    /// The force-directed spatial graph view
    Graph,
    
    /// A webview displaying a specific node's web content
    WebView(NodeKey),
}
```

**Key decisions**:
- `WebView(NodeKey)` ties each webview tile to a graph node
- NodeKey is the stable identifier from petgraph
- Implementing serde makes the entire tile tree serializable (optional but recommended)

### Step 1.3: Create Behavior Struct

Create `ports/graphshell/desktop/tile_behavior.rs`:

```rust
use egui::{Ui, WidgetText, Response, Id};
use egui_tiles::{Behavior, Tiles, TileId, UiResponse, TabState};
use crate::graph::GraphBrowserApp;
use crate::desktop::tile_kind::TileKind;

pub struct GraphshellTileBehavior<'a> {
    /// Reference to graph state (source of truth)
    pub graph_app: &'a mut GraphBrowserApp,
    
    /// Other dependencies will be added here (RunningAppState, etc.)
}

impl<'a> Behavior<TileKind> for GraphshellTileBehavior<'a> {
    // Methods implemented in later phases
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        pane: &mut TileKind,
    ) -> UiResponse {
        match pane {
            TileKind::Graph => {
                ui.label("Graph rendering goes here");
                UiResponse::None
            }
            TileKind::WebView(node_key) => {
                ui.label(format!("WebView for node {:?}", node_key));
                UiResponse::None
            }
        }
    }

    fn tab_title_for_pane(&mut self, pane: &TileKind) -> WidgetText {
        match pane {
            TileKind::Graph => "Graph".into(),
            TileKind::WebView(node_key) => {
                // Query graph for node title
                if let Some(node) = self.graph_app.graph.get_node(*node_key) {
                    node.title.clone().into()
                } else {
                    format!("Node {:?}", node_key).into()
                }
            }
        }
    }
}
```

---

## Phase 2: Minimal Tree Construction

### Step 2.1: Initialize Tree in Gui

Modify `ports/graphshell/desktop/gui.rs`:

```rust
use egui_tiles::{Tree, Tiles};
use crate::desktop::tile_kind::TileKind;

pub struct Gui {
    // NEW: egui_tiles state
    tiles_tree: Tree<TileKind>,
    
    // KEEP: Existing state
    pub graph_app: GraphBrowserApp,
    // ... other fields ...
}

impl Gui {
    pub fn new(/* ... */) -> Self {
        // Create initial tiles
        let mut tiles = Tiles::default();
        
        // Insert the graph as the root tile
        let graph_tile_id = tiles.insert_pane(TileKind::Graph);
        
        // Create tree with unique ID
        let tree = Tree::new("graphshell_tree", graph_tile_id, tiles);
        
        Self {
            tiles_tree: tree,
            graph_app: GraphBrowserApp::new(/* ... */),
            // ... initialize other fields ...
        }
    }
}
```

**Tree construction API** (from egui_tiles docs):

```rust
// Pattern 1: Single root pane
let root_id = tiles.insert_pane(TileKind::Graph);
let tree = Tree::new("my_tree", root_id, tiles);

// Pattern 2: Multiple tiles with tabs
let graph_id = tiles.insert_pane(TileKind::Graph);
let webview_id = tiles.insert_pane(TileKind::WebView(some_node_key));
let tab_container_id = tiles.insert_tab_tile(vec![graph_id, webview_id]);
let tree = Tree::new("my_tree", tab_container_id, tiles);

// Pattern 3: Horizontal split
let left_id = tiles.insert_pane(TileKind::Graph);
let right_id = tiles.insert_pane(TileKind::WebView(some_node_key));
let horizontal_id = tiles.insert_horizontal_tile(vec![left_id, right_id]);
let tree = Tree::new("my_tree", horizontal_id, tiles);
```

### Step 2.2: Replace Update Method with Tree UI

Replace the monolithic `update()` method:

```rust
impl Gui {
    pub(crate) fn update(
        &mut self,
        state: &RunningAppState,
        window: &ServoShellWindow,
        headed_window: &headed_window::HeadedWindow,
    ) {
        egui::CentralPanel::default().show(&headed_window.ctx, |ui| {
            // Create behavior with borrowed dependencies
            let mut behavior = GraphshellTileBehavior {
                graph_app: &mut self.graph_app,
                // Add other dependencies as needed
            };
            
            // egui_tiles handles ALL layout management
            self.tiles_tree.ui(&mut behavior, ui);
        });
        
        // Handle keyboard shortcuts, physics updates, etc. here
        // (outside the egui rendering)
    }
}
```

**What this replaces**:
- ❌ Old: 350+ lines of view branching, toolbar rendering, tab bar logic
- ✅ New: ~10 lines — egui_tiles handles it all

---

## Phase 3: Implement Graph Pane Rendering

### Step 3.1: Wire Graph Rendering

Update `pane_ui()` in `GraphshellTileBehavior`:

```rust
impl<'a> Behavior<TileKind> for GraphshellTileBehavior<'a> {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        tile_id: TileId,
        pane: &mut TileKind,
    ) -> UiResponse {
        match pane {
            TileKind::Graph => {
                // Use existing render::render_graph function
                render::render_graph(&headed_window.ctx, &mut self.graph_app);
                
                // IMPORTANT: Handle egui_graphs events here
                // (Double-click → create webview tile)
                self.handle_graph_events(tile_id);
                
                UiResponse::None
            }
            TileKind::WebView(node_key) => {
                // Phase 4
                UiResponse::None
            }
        }
    }
}
```

### Step 3.2: Handle Double-Click Events

Add event handling to create webview tiles:

```rust
impl<'a> GraphshellTileBehavior<'a> {
    fn handle_graph_events(&mut self, graph_tile_id: TileId) {
        // Check for egui_graphs events (from existing code)
        let events = self.graph_app.egui_state.events.borrow_mut();
        
        for event in events.iter() {
            if let egui_graphs::Event::NodeDoubleClick(node_key) = event {
                // User double-clicked a node in the graph
                self.open_or_focus_webview_tile(*node_key);
            }
        }
    }
    
    fn open_or_focus_webview_tile(&mut self, node_key: NodeKey) {
        // Check if a webview tile already exists for this node
        let existing_tile = self.find_webview_tile(node_key);
        
        if let Some(tile_id) = existing_tile {
            // Tile exists → focus it (make it active tab)
            self.tiles_tree.make_active(|tid, tile| {
                tid == tile_id
            });
        } else {
            // Tile doesn't exist → create it
            self.create_webview_tile(node_key);
        }
    }
    
    fn find_webview_tile(&self, node_key: NodeKey) -> Option<TileId> {
        // Search through all tiles to find WebView(node_key)
        for (tile_id, tile) in self.tiles_tree.tiles.tiles.iter() {
            if let egui_tiles::Tile::Pane(TileKind::WebView(key)) = tile {
                if *key == node_key {
                    return Some(*tile_id);
                }
            }
        }
        None
    }
    
    fn create_webview_tile(&mut self, node_key: NodeKey) {
        // Insert new webview pane
        let webview_tile_id = self.tiles_tree.tiles.insert_pane(
            TileKind::WebView(node_key)
        );
        
        // Add to the root container
        // (This will split the view horizontally by default)
        if let Some(root_id) = self.tiles_tree.root() {
            // Convert root to horizontal container if not already
            // For simplicity, add as new tab for now:
            
            // Get or create a tabs container at root
            let tabs_container = self.get_or_create_tabs_container(root_id);
            
            // Add child to tabs
            if let Some(egui_tiles::Tile::Container(
                egui_tiles::Container::Tabs(tabs)
            )) = self.tiles_tree.tiles.get_mut(tabs_container) {
                tabs.add_child(webview_tile_id);
                tabs.set_active(webview_tile_id);
            }
        }
    }
}
```

**Tile insertion patterns** (from egui_tiles API):

```rust
// Insert as pane
let tile_id = tiles.insert_pane(TileKind::WebView(node_key));

// Insert into existing tabs container
if let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(parent_id) {
    tabs.add_child(tile_id);
    tabs.set_active(tile_id);  // Make it the active tab
}

// Insert into horizontal/vertical layout
tiles.insert_horizontal_tile(vec![existing_id, new_id]);

// Move tile to different container
tree.move_tile_to_container(
    tile_id,
    target_container_id,
    insertion_index,
    false  // reflow_grid
);
```

---

## Phase 4: Per-Tile WebView Rendering

### Step 4.1: Add Rendering Context Management

Update `Gui` struct:

```rust
use std::rc::Rc;
use servo::webview::OffscreenRenderingContext;

pub struct Gui {
    tiles_tree: Tree<TileKind>,
    
    // NEW: One rendering context per visible webview tile
    rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    
    // EXISTING: Favicon cache (now per-tile)
    favicon_textures: HashMap<WebViewId, (TextureHandle, SizedTexture)>,
    
    graph_app: GraphBrowserApp,
    // ... other fields ...
}
```

### Step 4.2: Render WebView in Pane

Update `pane_ui()` for WebView tiles:

```rust
impl<'a> Behavior<TileKind> for GraphshellTileBehavior<'a> {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        tile_id: TileId,
        pane: &mut TileKind,
    ) -> UiResponse {
        match pane {
            TileKind::Graph => {
                // Phase 3 implementation
                render::render_graph(&headed_window.ctx, &mut self.graph_app);
                self.handle_graph_events(tile_id);
                UiResponse::None
            }
            
            TileKind::WebView(node_key) => {
                // Get or create rendering context for this tile
                let context = self.rendering_contexts
                    .entry(*node_key)
                    .or_insert_with(|| {
                        // Create new OffscreenRenderingContext
                        Rc::new(
                            self.window.rendering_context().offscreen_context()
                        )
                    });
                
                // Get webview from graph
                let webview = self.get_webview_for_node(*node_key);
                
                if let Some(webview) = webview {
                    // Tell webview to paint to its rendering context
                    webview.paint(context.clone());
                    
                    // Register egui PaintCallback to composite the texture
                    let callback = egui::PaintCallback {
                        rect: ui.max_rect(),
                        callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                            // Use render_to_parent_callback pattern
                            // (existing code from gui.rs browser_tab function)
                            context.render_to_parent_callback(painter.gl());
                        })),
                    };
                    
                    ui.painter().add(callback);
                }
                
                UiResponse::None
            }
        }
    }
}
```

**Key API calls**:

```rust
// Create offscreen context per tile (Servo API)
let context = window.rendering_context().offscreen_context();

// Can call multiple times - each returns independent context
let context1 = window.rendering_context().offscreen_context();
let context2 = window.rendering_context().offscreen_context();  // Different context!

// Paint webview to specific context
webview.paint(Rc::new(context));

// Composite into egui using PaintCallback
context.render_to_parent_callback(gl);
```

### Step 4.3: WebView Lifecycle Management

```rust
impl Gui {
    fn get_or_create_webview_for_node(&mut self, node_key: NodeKey) -> Option<WebView> {
        // Check if webview already exists
        if let Some(webview_id) = self.node_to_webview.get(&node_key) {
            return self.window.webviews().get(*webview_id).cloned();
        }
        
        // Create new webview for this node
        let node = self.graph_app.graph.get_node(node_key)?;
        let webview = self.window.create_webview(node.url.clone())?;
        
        // Track mapping
        let webview_id = webview.id();
        self.node_to_webview.insert(node_key, webview_id);
        self.webview_to_node.insert(webview_id, node_key);
        
        Some(webview)
    }
    
    fn cleanup_closed_tiles(&mut self) {
        // Called after tree.ui() to remove rendering contexts
        // for tiles that no longer exist
        
        let active_nodes: HashSet<NodeKey> = self.tiles_tree
            .tiles
            .iter()
            .filter_map(|(_, tile)| {
                if let Tile::Pane(TileKind::WebView(key)) = tile {
                    Some(*key)
                } else {</None
                }
            })
            .collect();
        
        // Remove contexts for nodes no longer in tree
        self.rendering_contexts.retain(|key, _| active_nodes.contains(key));
    }
}
```

---

## Phase 5: Tab UI Integration

### Step 5.1: Implement tab_ui() with Existing browser_tab()

```rust
impl<'a> Behavior<TileKind> for GraphshellTileBehavior<'a> {
    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<TileKind>,
        ui: &mut Ui,
        id: Id,
        tile_id: TileId,
        state: &TabState,
    ) -> Response {
        // Get the pane to determine title and favicon
        let Some(tile) = tiles.get(tile_id) else {
            return ui.label("Missing tile").response;
        };
        
        match tile {
            egui_tiles::Tile::Pane(TileKind::Graph) => {
                // Simple label for graph tab
                let response = ui.selectable_label(state.active, "🕸 Graph");
                
                // Make draggable
                response.interact(Sense::click_and_drag())
            }
            
            egui_tiles::Tile::Pane(TileKind::WebView(node_key)) => {
                // Reuse existing browser_tab() function!
                // (from gui.rs lines 242-327)
                
                let node = self.graph_app.graph.get_node(*node_key);
                let webview = self.get_webview_for_node(*node_key);
                let favicon = self.favicon_textures.get(&webview.id());
                
                browser_tab(
                    ui,
                    self.window,
                    webview,
                    favicon.cloned(),
                    state.active,
                )
            }
            
            _ => ui.label("Container").response,
        }
    }
    
    fn is_tab_closable(&self, _tiles: &Tiles<TileKind>, tile_id: TileId) -> bool {
        // Graph tab not closable, webview tabs are
        if let Some(Tile::Pane(TileKind::Graph)) = _tiles.get(tile_id) {
            false
        } else {
            true
        }
    }
    
    fn on_tab_close(&mut self, tiles: &mut Tiles<TileKind>, tile_id: TileId) -> bool {
        // Clean up webview when tab closes
        if let Some(Tile::Pane(TileKind::WebView(node_key))) = tiles.get(tile_id) {
            // Destroy webview
            if let Some(webview_id) = self.node_to_webview.remove(node_key) {
                self.window.destroy_webview(webview_id);
                self.webview_to_node.remove(&webview_id);
            }
            
            // Remove rendering context
            self.rendering_contexts.remove(node_key);
        }
        
        true  // Confirm close
    }
}
```

### Step 5.2: Favicon Loading Per-Tile

```rust
impl Gui {
    fn update_favicons(&mut self, ctx: &egui::Context) {
        // Get all active webview tiles
        for (tile_id, tile) in self.tiles_tree.tiles.iter() {
            if let egui_tiles::Tile::Pane(TileKind::WebView(node_key)) = tile {
                if let Some(webview_id) = self.node_to_webview.get(node_key) {
                    // Reuse existing load_pending_favicons logic
                    // (from gui.rs lines 826-857)
                    
                    if !self.favicon_textures.contains_key(webview_id) {
                        load_favicon_for_webview(
                            ctx,
                            self.window,
                            *webview_id,
                            &mut self.favicon_textures,
                        );
                    }
                }
            }
        }
    }
}
```

---

## Phase 6: Navigation Integration

### Step 6.1: Link Clicks Create New Tiles

```rust
impl Gui {
    fn handle_webview_navigation(&mut self, webview_id: WebViewId, new_url: String) {
        // Called when a webview navigates (from sync_webviews_to_graph)
        
        // Get the source node
        let Some(source_node_key) = self.webview_to_node.get(&webview_id) else {
            return;
        };
        
        // Check if URL already has a node
        if let Some(target_node_key) = self.graph_app.graph.url_to_node(&new_url) {
            // Node exists → create/focus its tile
            self.open_or_focus_webview_tile(target_node_key);
            
            // Create edge in graph
            self.graph_app.graph.add_edge(
                *source_node_key,
                target_node_key,
                EdgeType::Hyperlink,
            );
        } else {
            // New URL → create node + tile
            let new_node_key = self.graph_app.graph.add_node(Node {
                url: new_url.clone(),
                title: "[Loading...]".to_string(),
                // ... other node properties ...
            });
            
            // Create edge
            self.graph_app.graph.add_edge(
                *source_node_key,
                new_node_key,
                EdgeType::Hyperlink,
            );
            
            // Create webview tile
            self.create_webview_tile(new_node_key);
        }
    }
}
```

---

## Phase 7: Layout Persistence

### Step 7.1: Serialize Tree State

egui_tiles Tree is serializable if TileKind implements serde:

```rust
// Tree automatically persists with egui if you use egui::Context::data()
impl Gui {
    pub fn save_layout(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.insert_persisted(
                egui::Id::new("graphshell_tree"),
                self.tiles_tree.clone(),
            );
        });
    }
    
    pub fn load_layout(&mut self, ctx: &egui::Context) {
        if let Some(tree) = ctx.data(|data| {
            data.get_persisted::<Tree<TileKind>>(egui::Id::new("graphshell_tree"))
        }) {
            self.tiles_tree = tree;
        }
    }
}
```

**Or** serialize to your existing persistence system:

```rust
// Using rkyv (graphshell's existing serialization)
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TileTreeSnapshot {
    // Flatten tree to reproducible format
    pub root_layout: LayoutKind,
    pub webview_node_keys: Vec<NodeKey>,
}

impl TileTreeSnapshot {
    pub fn from_tree(tree: &Tree<TileKind>) -> Self {
        // Extract structure
        // ...
    }
    
    pub fn restore_into_tree(&self, graph: &Graph) -> Tree<TileKind> {
        // Rebuild tree from snapshot
        // ...
    }
}
```

---

## Complete Implementation Checklist

### Phase 1: Setup ✅
- [ ] Add `egui_tiles = "0.14.1"` to Cargo.toml
- [ ] Create `tile_kind.rs` with `TileKind` enum
- [ ] Create `tile_behavior.rs` with `GraphshellTileBehavior` struct
- [ ] Implement minimal `pane_ui()` and `tab_title_for_pane()`

### Phase 2: Basic Tree ✅
- [ ] Initialize `Tree<TileKind>` in `Gui::new()`
- [ ] Replace `update()` method body with `tree.ui(&mut behavior, ui)`
- [ ] Verify graph renders in single tile

### Phase 3: Graph Events ✅
- [ ] Implement `handle_graph_events()` for double-click detection
- [ ] Implement `open_or_focus_webview_tile()` to create/focus tiles
- [ ] Implement `find_webview_tile()` to check for existing tiles
- [ ] Test: Double-click node → new tile appears

### Phase 4: WebView Rendering ✅
- [ ] Add `rendering_contexts: HashMap<NodeKey, Rc<OffscreenRenderingContext>>` to `Gui`
- [ ] Implement per-tile context creation in `pane_ui()` for WebView
- [ ] Wire `webview.paint(context)` + `render_to_parent_callback`
- [ ] Implement `cleanup_closed_tiles()` for context cleanup
- [ ] Test: Multiple webviews render simultaneously

### Phase 5: Tab UI ✅
- [ ] Implement full `tab_ui()` using existing `browser_tab()` function
- [ ] Implement `is_tab_closable()` (graph = false, webview = true)
- [ ] Implement `on_tab_close()` to clean up webviews
- [ ] Adapt favicon loading to work per-tile
- [ ] Test: Tabs show titles and favicons, close button works

### Phase 6: Navigation ✅
- [ ] Implement `handle_webview_navigation()` for link clicks
- [ ] Wire to existing `sync_webviews_to_graph` mechanism
- [ ] Create edges in graph when navigation occurs
- [ ] Test: Click link → new tile + edge created

### Phase 7: Persistence ✅
- [ ] Implement `save_layout()` / `load_layout()`
- [ ] Integrate with existing fjall/redb persistence
- [ ] Test: Close app, reopen → layout restored

---

## API Reference Summary

### Tree Construction

```rust
use egui_tiles::{Tree, Tiles, TileId};

let mut tiles = Tiles::default();

// Insert panes
let id1 = tiles.insert_pane(TileKind::Graph);
let id2 = tiles.insert_pane(TileKind::WebView(key));

// Insert containers
let tabs_id = tiles.insert_tab_tile(vec![id1, id2]);
let horiz_id = tiles.insert_horizontal_tile(vec![id1, id2]);
let vert_id = tiles.insert_vertical_tile(vec![id1, id2]);
let grid_id = tiles.insert_grid_tile(vec![id1, id2, id3, id4]);

// Create tree
let tree = Tree::new("unique_id", root_id, tiles);
```

### Behavior Methods (All Optional Except 2)

```rust
impl Behavior<TileKind> for MyBehavior {
    // REQUIRED
    fn pane_ui(&mut self, ui: &mut Ui, tile_id: TileId, pane: &mut TileKind) -> UiResponse;
    fn tab_title_for_pane(&mut self, pane: &TileKind) -> WidgetText;
    
    // OPTIONAL (with sensible defaults)
    fn tab_ui(&mut self, ...) -> Response { /* default button */ }
    fn is_tab_closable(&self, ...) -> bool { true }
    fn on_tab_close(&mut self, ...) -> bool { true }
    fn tab_bar_height(&self, style: &Style) -> f32 { 24.0 }
    fn gap_width(&self, style: &Style) -> f32 { 1.0 }
    fn top_bar_right_ui(&mut self, ...) { /* add buttons */ }
    fn simplification_options(&self) -> SimplificationOptions { /* ... */ }
    // ... many more customization points ...
}
```

### Tree Operations

```rust
// Render
tree.ui(&mut behavior, ui);

// Query
let root_id = tree.root();
let active_tiles = tree.active_tiles();
let tile = tree.tiles.get(tile_id);

// Modify
tree.tiles.insert_pane(TileKind::WebView(key));
tree.tiles.remove(tile_id);
tree.move_tile_to_container(tile_id, container_id, index, false);
tree.make_active(|tid, tile| tid == target_id);

// Cleanup
tree.simplify(&options);
tree.gc(&mut behavior);
```

### Event Handling

```rust
// In pane_ui(), check for drag start:
if ui.button("Drag me").drag_started() {
    return UiResponse::DragStarted;
}

// egui_tiles handles the rest of drag-and-drop automatically
```

---

## Migration Order

1. **Phase 1-2 first** → Get basic tree rendering working
2. **Phase 3** → Wire graph events (most complex logic)
3. **Phase 4** → Add webview rendering (verify Servo API)
4. **Phase 5-6** → Polish (tabs, navigation)
5. **Phase 7** → Persistence (nice-to-have)

**Estimated effort**: 3-5 days for phases 1-4, 1-2 days for phases 5-7.

---

## Troubleshooting

### Multiple OffscreenRenderingContext Issues

**Symptom**: Webviews render to wrong tiles or crash.

**Solution**: Verify each tile has its own `Rc<OffscreenRenderingContext>`. Don't share contexts between tiles.

```rust
// WRONG
let context = window.rendering_context().offscreen_context();
for webview in webviews {
    webview.paint(context.clone());  // Sharing same context!
}

// CORRECT
for (node_key, webview) in webviews {
    let context = self.rendering_contexts.entry(node_key)
        .or_insert_with(|| Rc::new(window.rendering_context().offscreen_context()));
    webview.paint(context.clone());
}
```

### Tree Not Updating

**Symptom**: Tiles don't appear after `insert_pane()`.

**Solution**: Call `tree.ui()` again. The tree only updates during `ui()`.

### Tabs Not Showing Titles

**Symptom**: Empty tabs.

**Solution**: Implement `tab_title_for_pane()` — there's no default.

### Performance Issues

**Symptom**: Laggy with many webviews.

**Solution**: 
1. Limit visible offscreen contexts (destroy when tile not visible)
2. Implement `tree.set_visible(tile_id, false)` for background tiles
3. Use thermal states (Active/Warm/Cold) like the archived zoom plan suggested

---

## Next Steps

1. **Start with Phase 1** → Add dependency and types
2. **Create minimal proof-of-concept** → Graph in one tile, one webview in another
3. **Iterate** → Add features phase-by-phase
4. **Test continuously** → Each phase should be runnable

**Success Criteria for MVP**:
- ✅ Graph visible in one pane
- ✅ Double-click node → webview tile opens
- ✅ Multiple webviews render simultaneously
- ✅ Can drag tiles to rearrange layout
