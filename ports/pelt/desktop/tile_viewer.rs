/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The on-screen tile viewer (V5): a window split into tiles, each showing a document.
//!
//! A thin winit + present wrapper over [`TileShell`](crate::tile_shell::TileShell): the
//! window translates winit events into the shell's semantic pointer/wheel methods and
//! composites the frame the shell renders. All the interaction logic (drags, drops,
//! routing) lives in the host-agnostic shell, so the same brain drives the window here
//! and a headless test/driver elsewhere.

use genet_host_api::tile::{
    ContentSource, DocumentRef, SplitAxis, Tile, TileBranch, TileId, TileTree,
};

use crate::{StaticViewerOutcome, WindowingMode};

/// Configuration for a headed tile-viewer run. It parallels [`StaticViewerConfig`]
/// without pretending the heterogeneous tile sessions are one static document.
pub struct TileViewerConfig {
    pub urls: Vec<String>,
    pub windowing: WindowingMode,
    pub size: Option<(u32, u32)>,
    pub frames: Option<u32>,
}

impl TileViewerConfig {
    pub fn new(urls: Vec<String>, windowing: WindowingMode) -> Self {
        Self {
            urls,
            windowing,
            size: None,
            frames: None,
        }
    }

    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width.max(1), height.max(1)));
        self
    }

    pub fn with_frame_limit(mut self, frames: u32) -> Self {
        self.frames = Some(frames.max(1));
        self
    }
}

/// Build a demo tile tree from content URLs: one tile is a single document, two are a
/// side-by-side row split, and three or more put the first two in a tab-stack beside a
/// single tile (so the demo shows a split, tabs, and content compositing at once).
fn tree_from_urls(urls: &[String]) -> TileTree {
    let tile = |index: usize, id: u64| Tile {
        id: TileId(id),
        title: crate::tile_surface::tile_title(&urls[index]),
        content: ContentSource::Document(DocumentRef(urls[index].clone())),
        accent: None,
    };
    match urls.len() {
        0 => TileTree::single(Tile {
            id: TileId(1),
            title: "blank".into(),
            content: ContentSource::Document(DocumentRef("about:blank".into())),
            accent: None,
        }),
        1 => TileTree::single(tile(0, 1)),
        2 => TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::single(tile(0, 1))),
                TileBranch::new(0.5, TileTree::single(tile(1, 2))),
            ],
        ),
        _ => {
            // The demo tree is a fixed shape, so anything past the third URL has
            // nowhere to go. Say so: a document that silently never appears reads
            // as a rendering failure rather than as a limit of this profile.
            if urls.len() > 3 {
                eprintln!(
                    "[pelt-tiles] the demo tree holds three documents; ignoring {}",
                    urls[3..].join(", ")
                );
            }
            TileTree::split(
                SplitAxis::Row,
                vec![
                    TileBranch::new(0.5, TileTree::stack(vec![tile(0, 1), tile(1, 2)], 0)),
                    TileBranch::new(0.5, TileTree::single(tile(2, 3))),
                ],
            )
        },
    }
}

/// Run the tile viewer for the content `urls`. Headless returns immediately; headed
/// opens the window.
pub fn run_tile_viewer(
    urls: Vec<String>,
    windowing: WindowingMode,
) -> Result<StaticViewerOutcome, String> {
    run_tile_viewer_with_config(TileViewerConfig::new(urls, windowing))
}

/// Run the tile viewer with an explicit physical size and optional deterministic
/// frame limit for capture/CI profiles.
pub fn run_tile_viewer_with_config(
    config: TileViewerConfig,
) -> Result<StaticViewerOutcome, String> {
    let tree = tree_from_urls(&config.urls);
    match config.windowing {
        WindowingMode::Headless => Ok(StaticViewerOutcome {
            url: config.urls.first().cloned().unwrap_or_default(),
            created_window: false,
            redraws: 0,
            size: (0, 0),
        }),
        WindowingMode::Headed => windowed::run(tree, config),
    }
}

mod windowed {
    use std::collections::HashMap;
    use std::sync::Arc;

    use accesskit::{Action, NodeId as AccessNodeId};
    use genet_host_api::tile::TileTree;
    use genet_scripted_dom::NodeId;
    use genet_winit_host::{AccessKitBridge, BridgeStatus, SurfaceHost, wheel_delta_from_winit};
    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions};
    use winit::application::ApplicationHandler;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    use super::{StaticViewerOutcome, TileViewerConfig};
    use crate::tile_shell::TileShell;

    type Rect = (f32, f32, f32, f32);

    pub(super) fn run(
        tree: TileTree,
        config: TileViewerConfig,
    ) -> Result<StaticViewerOutcome, String> {
        let shell = TileShell::new(tree);
        let event_loop =
            EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = TileApp::new(shell, config);
        event_loop
            .run_app(&mut app)
            .map_err(|error| format!("tile event loop failed: {error}"))?;
        Ok(app.outcome())
    }

    /// The window over a [`TileShell`]: winit events translate to shell input, and the
    /// shell's frame is composited (the frame layer plus one document layer per tile).
    struct TileApp {
        shell: TileShell,
        first_url: String,
        window: Option<Arc<Window>>,
        host: Option<SurfaceHost>,
        width: u32,
        height: u32,
        scale_factor: f32,
        redraws: u32,
        frames: Option<u32>,
        /// OS accessibility bridge: the frame DOM pelt renders is projected to an
        /// AccessKit tree each frame and pushed here, so a screen reader reads the
        /// tab bars and the status bar's leaves. `None` until the window exists.
        a11y: Option<AccessKitBridge>,
        /// Actionable node ids from the last projection, so a screen reader's
        /// `Click` routes back to the same handler a tab press takes.
        a11y_route: HashMap<AccessNodeId, NodeId>,
    }

    impl TileApp {
        fn new(shell: TileShell, config: TileViewerConfig) -> Self {
            Self {
                shell,
                first_url: config.urls.into_iter().next().unwrap_or_default(),
                window: None,
                host: None,
                width: config.size.map_or(1100, |size| size.0),
                height: config.size.map_or(750, |size| size.1),
                scale_factor: 1.0,
                redraws: 0,
                frames: config.frames,
                a11y: None,
                a11y_route: HashMap::new(),
            }
        }

        /// Apply the screen-reader actions the OS queued since the last frame. The
        /// route map is from the previous projection, which is what the OS acted
        /// against.
        fn pump_a11y_actions(&mut self) {
            let requests = match self.a11y.as_mut() {
                Some(bridge) => bridge.drain_actions(),
                None => return,
            };
            for request in requests {
                if request.action == Action::Click {
                    if let Some(&node) = self.a11y_route.get(&request.target_node) {
                        self.shell.activate(node);
                    }
                }
            }
        }

        /// Project the current frame into the OS accessibility tree, installing the
        /// adapter on the first laid-out frame. Best-effort: a platform without an
        /// adapter simply goes without.
        fn sync_a11y(&mut self) {
            if self.a11y.is_none() {
                return;
            }
            let (tree, route) = self.shell.a11y_tree();
            self.a11y_route = route.into_iter().collect();
            let (Some(bridge), Some(window)) = (self.a11y.as_mut(), self.window.as_ref()) else {
                return;
            };
            match bridge.status() {
                BridgeStatus::Installed => bridge.update(tree),
                BridgeStatus::Unavailable => {
                    let _ = bridge.install(window, tree);
                },
            }
        }

        fn outcome(&self) -> StaticViewerOutcome {
            StaticViewerOutcome {
                url: self.first_url.clone(),
                created_window: self.window.is_some(),
                redraws: self.redraws,
                size: if self.window.is_some() {
                    (self.width, self.height)
                } else {
                    (0, 0)
                },
            }
        }

        fn window_title(&self) -> String {
            crate::static_viewer::pelt_window_title(self.shell.primary_document_title().as_deref())
        }

        fn render(&mut self, event_loop: &ActiveEventLoop) {
            // A screen reader's request lands on the previous frame's tree; apply it
            // first so its effect shows in the frame we are about to build.
            self.pump_a11y_actions();
            // Time the whole frame (produce + rasterize + compose) and feed the
            // status bar's meter for the NEXT frame: real measured wall time.
            let frame_t0 = std::time::Instant::now();
            let (win_w, win_h) = (self.width.max(1), self.height.max(1));
            let (logical_w, logical_h) = (
                crate::static_viewer::logical_extent(win_w, self.scale_factor),
                crate::static_viewer::logical_extent(win_h, self.scale_factor),
            );
            self.shell.resize(logical_w, logical_h);
            let frame = self.shell.frame();
            self.sync_a11y();

            let Some(host) = self.host.as_ref() else {
                return;
            };
            // The frame (tab bars + content backgrounds) is the bottom layer; each
            // tile's document composites over its content rect.
            let (_ft, frame_view) = host.rasterize_scaled(
                &frame.frame_scene,
                win_w,
                win_h,
                ColorLoad::Clear(wgpu::Color {
                    r: 0.13,
                    g: 0.13,
                    b: 0.16,
                    a: 1.0,
                }),
                self.scale_factor,
            );
            let tile_layers: Vec<(wgpu::Texture, wgpu::TextureView, Rect)> = frame
                .tiles
                .iter()
                .map(|layer| {
                    let (w, h) = (
                        physical_extent(layer.rect.2, self.scale_factor),
                        physical_extent(layer.rect.3, self.scale_factor),
                    );
                    let (tex, view) = host.rasterize_scaled(
                        &layer.scene,
                        w,
                        h,
                        ColorLoad::Clear(wgpu::Color::WHITE),
                        self.scale_factor,
                    );
                    (tex, view, layer.rect)
                })
                .collect();

            let Some(swap) = host.acquire() else { return };
            let target = swap
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let renderer = host.renderer();
            renderer.compose_external_texture(
                &frame_view,
                &target,
                host.format(),
                win_w,
                win_h,
                ExternalTexturePlacement::new([0.0, 0.0, win_w as f32, win_h as f32]),
            );
            for (_tex, view, rect) in &tile_layers {
                renderer.compose_external_texture(
                    view,
                    &target,
                    host.format(),
                    win_w,
                    win_h,
                    placement(*rect, self.scale_factor),
                );
            }
            // The drag ghost composites last (over everything), on a transparent clear so
            // only its box shows. `_gt` holds the texture alive until present.
            if let Some(ghost) = frame.ghost.as_ref() {
                let (gw, gh) = (
                    physical_extent(ghost.rect.2, self.scale_factor),
                    physical_extent(ghost.rect.3, self.scale_factor),
                );
                let (_gt, gview) = host.rasterize_scaled(
                    &ghost.scene,
                    gw,
                    gh,
                    ColorLoad::Clear(wgpu::Color::TRANSPARENT),
                    self.scale_factor,
                );
                renderer.compose_external_texture(
                    &gview,
                    &target,
                    host.format(),
                    win_w,
                    win_h,
                    placement(ghost.rect, self.scale_factor),
                );
            }
            swap.present();
            self.redraws += 1;
            self.shell
                .note_frame_millis(frame_t0.elapsed().as_secs_f32() * 1000.0);
            if self.frames.is_some_and(|limit| self.redraws >= limit) {
                event_loop.exit();
            }
        }

        fn request_redraw(&self) {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    fn physical_extent(logical: f32, scale_factor: f32) -> u32 {
        ((logical.max(1.0) * scale_factor.max(1.0)).round() as u32).max(1)
    }

    fn placement(r: Rect, scale_factor: f32) -> ExternalTexturePlacement {
        ExternalTexturePlacement::new([
            r.0 * scale_factor,
            r.1 * scale_factor,
            (r.0 + r.2) * scale_factor,
            (r.1 + r.3) * scale_factor,
        ])
    }

    impl ApplicationHandler for TileApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = crate::static_viewer::pelt_window_attributes(
                self.window_title(),
                self.width,
                self.height,
            )
            // Hidden until the first frame has installed the accessibility
            // adapter: `accesskit_windows` subclasses the window and must do
            // so before it is shown, while the adapter itself cannot exist
            // until there is a laid-out tree to hand it. Showing here panics
            // on Windows. It also means the window never appears unpainted.
            .with_visible(false);
            let window = match event_loop.create_window(attributes) {
                Ok(window) => Arc::new(window),
                Err(err) => {
                    eprintln!("[pelt-tiles] could not create window: {err}");
                    event_loop.exit();
                    return;
                },
            };
            let size = window.inner_size();
            self.width = size.width.max(1);
            self.height = size.height.max(1);
            self.scale_factor = window.scale_factor() as f32;
            window.set_title(&self.window_title());
            self.shell.resize(
                crate::static_viewer::logical_extent(self.width, self.scale_factor),
                crate::static_viewer::logical_extent(self.height, self.scale_factor),
            );
            // A screen-reader action wakes the loop so the next frame drains and
            // routes it. The adapter installs on the first laid-out frame, in
            // `sync_a11y`, once there is a tree to hand it.
            let wake_window = window.clone();
            let mut bridge = AccessKitBridge::new(move || wake_window.request_redraw());
            // `accesskit_windows` subclasses the window and must do so before it
            // is first shown. The tree here is whatever the shell has before any
            // frame has run; `sync_a11y` replaces it every frame after.
            let (tree, route) = self.shell.a11y_tree();
            self.a11y_route = route.into_iter().collect();
            let _ = bridge.install(&window, tree);
            self.a11y = Some(bridge);
            window.set_visible(true);
            let options = NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            };
            match SurfaceHost::boot(window.clone(), self.width, self.height, options) {
                Ok(host) => self.host = Some(host),
                Err(err) => {
                    eprintln!("[pelt-tiles] {err}");
                    event_loop.exit();
                    return;
                },
            }
            window.request_redraw();
            self.window = Some(window);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
                return;
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    self.width = size.width.max(1);
                    self.height = size.height.max(1);
                    if let Some(host) = self.host.as_mut() {
                        host.resize(self.width, self.height);
                    }
                    self.shell.resize(
                        crate::static_viewer::logical_extent(self.width, self.scale_factor),
                        crate::static_viewer::logical_extent(self.height, self.scale_factor),
                    );
                    self.request_redraw();
                },
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    self.scale_factor = scale_factor as f32;
                    if let Some(window) = self.window.as_ref() {
                        let size = window.inner_size();
                        self.width = size.width.max(1);
                        self.height = size.height.max(1);
                    }
                    if let Some(host) = self.host.as_mut() {
                        host.resize(self.width, self.height);
                    }
                    self.shell.resize(
                        crate::static_viewer::logical_extent(self.width, self.scale_factor),
                        crate::static_viewer::logical_extent(self.height, self.scale_factor),
                    );
                    self.request_redraw();
                },
                WindowEvent::CursorMoved { position, .. } => {
                    if self.shell.pointer_move(
                        crate::static_viewer::logical_position(
                            position.x as f32,
                            self.scale_factor,
                        ),
                        crate::static_viewer::logical_position(
                            position.y as f32,
                            self.scale_factor,
                        ),
                    ) {
                        self.request_redraw();
                    }
                },
                WindowEvent::MouseInput { state, button, .. } => {
                    if button != MouseButton::Left {
                        return;
                    }
                    let changed = match state {
                        ElementState::Pressed => self.shell.pointer_down(),
                        ElementState::Released => self.shell.pointer_up(),
                    };
                    if changed {
                        self.request_redraw();
                    }
                },
                WindowEvent::MouseWheel { delta, .. } => {
                    let (dx, dy) = wheel_delta_from_winit(delta);
                    if self
                        .shell
                        .wheel(dx / self.scale_factor, dy / self.scale_factor)
                    {
                        self.request_redraw();
                    }
                },
                WindowEvent::RedrawRequested => self.render(event_loop),
                _ => {},
            }
        }
    }
}
