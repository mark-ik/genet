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

use pelt_core::tile::{ContentSource, DocumentRef, SplitAxis, Tile, TileBranch, TileId, TileTree};

use crate::{StaticViewerOutcome, WindowingMode};

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
        _ => TileTree::split(
            SplitAxis::Row,
            vec![
                TileBranch::new(0.5, TileTree::stack(vec![tile(0, 1), tile(1, 2)], 0)),
                TileBranch::new(0.5, TileTree::single(tile(2, 3))),
            ],
        ),
    }
}

/// Run the tile viewer for the content `urls`. Headless returns immediately; headed
/// opens the window.
pub fn run_tile_viewer(
    urls: Vec<String>,
    windowing: WindowingMode,
) -> Result<StaticViewerOutcome, String> {
    let tree = tree_from_urls(&urls);
    match windowing {
        WindowingMode::Headless => Ok(StaticViewerOutcome {
            url: urls.first().cloned().unwrap_or_default(),
            created_window: false,
            redraws: 0,
        }),
        WindowingMode::Headed => windowed::run(tree, urls),
    }
}

mod windowed {
    use std::sync::Arc;

    use netrender::external_texture::ExternalTexturePlacement;
    use netrender::{ColorLoad, NetrenderOptions};
    use pelt_core::tile::TileTree;
    use serval_winit_host::{wheel_delta_from_winit, SurfaceHost};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    use super::StaticViewerOutcome;
    use crate::tile_shell::TileShell;

    type Rect = (f32, f32, f32, f32);

    pub(super) fn run(tree: TileTree, urls: Vec<String>) -> Result<StaticViewerOutcome, String> {
        let shell = TileShell::new(tree);
        let event_loop =
            EventLoop::new().map_err(|error| format!("could not create event loop: {error}"))?;
        let mut app = TileApp::new(shell, urls);
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
        redraws: u32,
    }

    impl TileApp {
        fn new(shell: TileShell, urls: Vec<String>) -> Self {
            Self {
                shell,
                first_url: urls.into_iter().next().unwrap_or_default(),
                window: None,
                host: None,
                width: 1100,
                height: 750,
                redraws: 0,
            }
        }

        fn outcome(&self) -> StaticViewerOutcome {
            StaticViewerOutcome {
                url: self.first_url.clone(),
                created_window: self.window.is_some(),
                redraws: self.redraws,
            }
        }

        fn render(&mut self) {
            let (win_w, win_h) = (self.width.max(1), self.height.max(1));
            self.shell.resize(win_w, win_h);
            let frame = self.shell.frame();

            let Some(host) = self.host.as_ref() else { return };
            // The frame (tab bars + content backgrounds) is the bottom layer; each
            // tile's document composites over its content rect.
            let (_ft, frame_view) = host.rasterize(
                &frame.frame_scene,
                win_w,
                win_h,
                ColorLoad::Clear(wgpu::Color { r: 0.13, g: 0.13, b: 0.16, a: 1.0 }),
            );
            let tile_layers: Vec<(wgpu::Texture, wgpu::TextureView, Rect)> = frame
                .tiles
                .iter()
                .map(|layer| {
                    let (w, h) = (layer.rect.2.max(1.0) as u32, layer.rect.3.max(1.0) as u32);
                    let (tex, view) =
                        host.rasterize(&layer.scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
                    (tex, view, layer.rect)
                })
                .collect();

            let Some(swap) = host.acquire() else { return };
            let target = swap.texture.create_view(&wgpu::TextureViewDescriptor::default());
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
                    placement(*rect),
                );
            }
            // The drag ghost composites last (over everything), on a transparent clear so
            // only its box shows. `_gt` holds the texture alive until present.
            if let Some(ghost) = frame.ghost.as_ref() {
                let (gw, gh) = (ghost.rect.2.max(1.0) as u32, ghost.rect.3.max(1.0) as u32);
                let (_gt, gview) =
                    host.rasterize(&ghost.scene, gw, gh, ColorLoad::Clear(wgpu::Color::TRANSPARENT));
                renderer.compose_external_texture(
                    &gview,
                    &target,
                    host.format(),
                    win_w,
                    win_h,
                    placement(ghost.rect),
                );
            }
            swap.present();
            self.redraws += 1;
        }

        fn request_redraw(&self) {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }
    }

    fn placement(r: Rect) -> ExternalTexturePlacement {
        ExternalTexturePlacement::new([r.0, r.1, r.0 + r.2, r.1 + r.3])
    }

    impl ApplicationHandler for TileApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title("Pelt — tiles")
                .with_inner_size(PhysicalSize::new(self.width, self.height));
            let window = match event_loop.create_window(attributes) {
                Ok(window) => Arc::new(window),
                Err(err) => {
                    eprintln!("[pelt-tiles] could not create window: {err}");
                    event_loop.exit();
                    return;
                }
            };
            let size = window.inner_size();
            self.width = size.width.max(1);
            self.height = size.height.max(1);
            self.shell.resize(self.width, self.height);
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
                }
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
                    self.shell.resize(self.width, self.height);
                    self.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if self.shell.pointer_move(position.x as f32, position.y as f32) {
                        self.request_redraw();
                    }
                }
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
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (dx, dy) = wheel_delta_from_winit(delta);
                    if self.shell.wheel(dx, dy) {
                        self.request_redraw();
                    }
                }
                WindowEvent::RedrawRequested => self.render(),
                _ => {}
            }
        }
    }
}
