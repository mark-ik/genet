/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `pelt-live-counter`: Stage 1b-window of
//! `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`.
//!
//! The visible payoff of the headless Stages 1a/1b/2a/2b/3: a real on-screen
//! winit window running an [`xilem_serval`] demo, rendered by serval and
//! presented through netrender. The window shows a big count number, a clickable
//! `[ + ]` button, and (Stage 3, the form-control slice) a typeable text field —
//! a [`text_field`] lensed onto the app state. A background timer bumps the count
//! ~1/s so the number climbs on its own; clicking `[ + ]` bumps it too; clicking
//! the field focuses it and typing edits it — proving the full input loop
//! (pointer *and* keyboard) on screen.
//!
//! The spine (the same one the headless probe asserts on, now driven by a
//! window):
//!
//! ```text
//! app state --(ServalAppRunner)--> ScriptedDom diff
//!           --(scene_from_scripted_dom: cascade -> layout -> paint emit)--> netrender::Scene
//!           --(Renderer::render_vello)--> Rgba8Unorm texture
//!           --(Renderer::compose_external_texture)--> wgpu::Surface backbuffer --> present
//! ```
//!
//! # The present path
//!
//! netrender's vello rasterizer writes into an `Rgba8Unorm` texture (it binds
//! the target as a storage texture), but a winit surface backbuffer is
//! typically `Bgra8UnormSrgb`. A raw `copy_texture_to_texture` requires
//! matching formats, so present is *not* a copy: it is a blit. netrender
//! already ships exactly that blit — [`Renderer::compose_external_texture`]
//! samples a source texture and draws it into a target view of any
//! `target_format` (the same zero-copy pass pelt-viewer uses for `<img>`
//! overlays). We point it at the surface's backbuffer view, so the bin adds no
//! GPU code of its own.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};
use serval_scripted_dom::ScriptedDom;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};
use xilem_serval::{
    El, Key, KeyEvent, Lens, Modifiers, NamedKey, OnClick, PointerClick, ServalAppRunner, TextField,
    TextInput, el, lens, on_click, text_field_typed,
};

use accesskit_winit::{Adapter, Event as AkEvent, WindowEvent as AkWindowEvent};
use pelt_live::{accesskit_tree, fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

// ── App state + view ───────────────────────────────────────────────────────

/// The app state: a counter plus an editable text field. The counter is the
/// Stage 1b probe; `field` (a [`TextInput`] — buffer + caret) is the Stage 3
/// form-control slice — a `text_field` lensed onto it edits it as you type, with
/// ←/→ moving the caret.
struct Demo {
    count: u32,
    field: TextInput,
}

/// The concrete demo view type: `<div>` holding the count `<p>`, the `+`
/// `<button>` (an `on_click` that increments the count), a `<label>` prompt, and
/// a `text_field` lensed onto `Demo::text`. Every handler is a non-capturing
/// closure that coerces to a `fn` pointer, so the whole type is nameable (no
/// boxing). The lensed field carries the reusable [`TextField`] type bridged
/// onto `Demo` by `xilem_core`'s `Lens`.
type DemoView = El<
    (
        El<String, Demo, ()>,
        OnClick<El<&'static str, Demo, ()>, Demo, (), fn(&mut Demo, PointerClick)>,
        El<&'static str, Demo, ()>,
        // `Lens<CF, V, F, ParentState, ChildState, Action, Context>`: the field
        // component (`fn(&mut TextInput) -> TextField`), the inner view
        // (`TextField`), the projection (`fn(&mut Demo) -> &mut TextInput`), then
        // the parent/child state, action, and context types.
        Lens<
            fn(&mut TextInput) -> TextField,
            TextField,
            fn(&mut Demo) -> &mut TextInput,
            Demo,
            TextInput,
            (),
            xilem_serval::ServalCtx,
        >,
    ),
    Demo,
    (),
>;

fn demo_view(s: &Demo) -> DemoView {
    let increment: fn(&mut Demo, PointerClick) = |s: &mut Demo, _ev| s.count += 1;
    // `text_field_typed` is `text_field` with its concrete return type named, so
    // the `Lens<…>` in `DemoView` can be spelled. A thin `|t| text_field_typed(t)`
    // adapter bridges its `&str` argument to the `Fn(&mut ChildState) -> View`
    // shape `lens` expects. Both the adapter and the lens projection are `fn`
    // pointers so `DemoView` stays nameable (no boxing).
    let make_field: fn(&mut TextInput) -> TextField = |t: &mut TextInput| text_field_typed(t);
    let to_field: fn(&mut Demo) -> &mut TextInput = |d: &mut Demo| &mut d.field;
    el::<_, Demo, ()>(
        "div",
        (
            el::<_, Demo, ()>("p", s.count.to_string()),
            on_click(el::<_, Demo, ()>("button", "+"), increment),
            el::<_, Demo, ()>("label", "Click the field below, then type (←/→ move the caret):"),
            lens(make_field, to_field),
        ),
    )
}

/// The author stylesheet. Block boxes so layout reaches every element; a large
/// font on the `<p>` makes the count visibly big; the `<button>` gets a little
/// padding/colour so the `[ + ]` target reads as a button; the `<input>` field
/// gets a light background and padding so it reads as a typeable box. Kept
/// minimal and within what serval's cascade supports. The page background is the
/// white clear in [`App::render`] (the runner attaches the `<div>` directly
/// under the document root — there is no `<body>` element to style).
const SHEET: &[&str] = &[
    "div, p, button, label, input { display: block; }",
    "p { font-size: 96px; color: rgb(30, 30, 50); }",
    "button { font-size: 48px; color: rgb(255, 255, 255); \
        background-color: rgb(60, 120, 220); padding: 12px; }",
    "label { font-size: 28px; color: rgb(60, 60, 80); padding: 8px; }",
    "input { font-size: 40px; color: rgb(20, 20, 20); \
        background-color: rgb(235, 238, 245); padding: 12px; }",
];

// ── winit user event ───────────────────────────────────────────────────────

/// Events injected into the loop from off the main thread / from accesskit.
///
/// `Tick` is the ~1Hz timer (a background thread sleeps 1s and sends it through
/// an [`EventLoopProxy`], so the timer lives off the event loop without a
/// busy-poll). `Accessibility` carries an [`accesskit_winit::Event`]: the
/// adapter's deferred-event model delivers a11y requests (initial-tree, action,
/// deactivation) as user events, which is why [`UserEvent`] implements
/// `From<accesskit_winit::Event>` (the bound `Adapter::with_event_loop_proxy`
/// requires). Not `Copy`/`Clone`: the a11y event isn't.
#[derive(Debug)]
enum UserEvent {
    Tick,
    Accessibility(AkEvent),
}

impl From<AkEvent> for UserEvent {
    fn from(event: AkEvent) -> Self {
        UserEvent::Accessibility(event)
    }
}

// ── winit → serval key mapping ───────────────────────────────────────────────

/// Map a winit logical key to the serval-native [`KeyEvent`], or `None` for a
/// key with no text and no named mapping (skipped).
///
/// `Key::Character(s)` carries the text the key produced and maps straight to
/// [`Key::Character`]. The named keys the editing foundation cares about
/// ([`NamedKey`]) map one-to-one; in particular **Space maps to
/// [`NamedKey::Space`]** (not `Character(" ")`) per the Stage 3b convention, and
/// **Backspace maps to [`NamedKey::Backspace`]** so the field's edit handler can
/// pop a char. Any other named key becomes [`NamedKey::Other`] (a real event the
/// field currently ignores). `Dead`/`Unidentified` keys produce no text and have
/// no mapping, so they are skipped.
fn key_event_from_winit(key: &WinitKey, mods: Modifiers) -> Option<KeyEvent> {
    let mapped = match key {
        WinitKey::Character(s) => Key::Character(s.to_string()),
        WinitKey::Named(named) => Key::Named(match named {
            WinitNamedKey::Backspace => NamedKey::Backspace,
            WinitNamedKey::Enter => NamedKey::Enter,
            WinitNamedKey::Tab => NamedKey::Tab,
            WinitNamedKey::Escape => NamedKey::Escape,
            WinitNamedKey::Space => NamedKey::Space,
            WinitNamedKey::ArrowLeft => NamedKey::ArrowLeft,
            WinitNamedKey::ArrowRight => NamedKey::ArrowRight,
            WinitNamedKey::ArrowUp => NamedKey::ArrowUp,
            WinitNamedKey::ArrowDown => NamedKey::ArrowDown,
            WinitNamedKey::Delete => NamedKey::Delete,
            WinitNamedKey::Home => NamedKey::Home,
            WinitNamedKey::End => NamedKey::End,
            _ => NamedKey::Other,
        }),
        // No text, no named mapping: nothing to route.
        WinitKey::Dead(_) | WinitKey::Unidentified(_) => return None,
    };
    Some(KeyEvent::with_mods(mapped, mods))
}

// ── GPU state (created on resume) ────────────────────────────────────────────

/// wgpu/netrender state, built once a window exists. Held together so the
/// surface, its config, and the renderer share one lifetime.
struct Gpu {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

// ── The application ─────────────────────────────────────────────────────────

/// Logic alias: `demo_view` as the runner's logic closure type.
type Logic = fn(&Demo) -> DemoView;

struct App {
    /// The shared document the runner mutates and the render path reads.
    dom: Rc<RefCell<ScriptedDom>>,
    runner: ServalAppRunner<Demo, Logic, DemoView>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// The accesskit screen-reader adapter (created on resume, once a window
    /// exists). `update_if_active` no-ops until a screen reader activates a11y.
    adapter: Option<Adapter>,
    /// A proxy clone for building the adapter on resume (it delivers a11y
    /// requests back as `UserEvent::Accessibility`).
    proxy: EventLoopProxy<UserEvent>,
    /// Last cursor position in physical pixels (window space == content space:
    /// the surface fills the window, so window coords are layout coords).
    cursor: (f32, f32),
    /// Current keyboard modifiers (tracked from `ModifiersChanged`), folded into
    /// each `KeyEvent` — so `Shift+Tab` reverses focus traversal.
    modifiers: Modifiers,
    width: u32,
    height: u32,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::new(
            dom.clone(),
            demo_view as Logic,
            Demo {
                count: 0,
                field: TextInput::default(),
            },
        );
        Self {
            dom,
            runner,
            window: None,
            gpu: None,
            adapter: None,
            proxy,
            cursor: (0.0, 0.0),
            modifiers: Modifiers::default(),
            width: 800,
            height: 600,
        }
    }

    /// Push the current accessibility tree to the adapter. Builds it eagerly
    /// from the live DOM + a fresh layout (`fragments`) + the runner's focus,
    /// then hands it to `update_if_active`, which only does work when a screen
    /// reader is active. (The spare layout pass when inactive is acceptable for
    /// a demo; a real host would gate on activation.)
    fn push_a11y(&mut self) {
        let (w, h) = (self.width.max(1), self.height.max(1));
        let dom = self.dom.borrow();
        let fragments = fragments_from_scripted_dom(&dom, SHEET, w, h);
        let tree = accesskit_tree(&dom, &fragments, self.runner.focus());
        drop(dom);
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.update_if_active(|| tree);
        }
    }

    /// Render the current DOM and present it to the surface backbuffer.
    ///
    /// 1. `scene_from_scripted_dom` runs the serval engine (cascade → layout →
    ///    paint emit) over the live `ScriptedDom` into a `netrender::Scene`.
    /// 2. `render_vello` rasterizes the scene into an `Rgba8Unorm` texture.
    /// 3. `compose_external_texture` blits that texture onto the surface's
    ///    (sRGB BGRA) backbuffer — the format-bridging present.
    fn render(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else { return };
        let (w, h) = (self.width.max(1), self.height.max(1));

        // 1. Engine pipeline → Scene. When a field is focused, paint its caret:
        //    the focused element + the byte offset of the field's char-index caret.
        let caret = self.runner.focus().map(|node| {
            let field = &self.runner.state().field;
            let byte_offset = field
                .text()
                .char_indices()
                .nth(field.caret())
                .map(|(b, _)| b)
                .unwrap_or(field.text().len());
            (node, byte_offset)
        });
        let scene: Scene = scene_from_scripted_dom(&self.dom.borrow(), SHEET, w, h, caret);

        // 2. Render the scene into a fresh Rgba8Unorm target. vello binds this
        //    as a storage texture (STORAGE_BINDING) and also reads it back via
        //    sampling for the present blit (TEXTURE_BINDING).
        let device = &gpu.renderer.wgpu_device.core.device;
        let content = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pelt-live-counter content"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let content_view = content.create_view(&wgpu::TextureViewDescriptor {
            label: Some("pelt-live-counter content view"),
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        gpu.renderer.render_vello(
            &scene,
            &content_view,
            ColorLoad::Clear(wgpu::Color::WHITE),
        );

        // 3. Acquire the surface backbuffer and blit the content onto it. The
        //    blit pass uses `LoadOp::Load`, so it draws over whatever is in the
        //    backbuffer; the full-viewport draw covers it entirely (the scene's
        //    body background paints the whole viewport), so no separate clear
        //    is needed.
        let frame = match gpu.surface.get_current_texture() {
            // Both Success and Suboptimal carry a usable frame; present it.
            // (Suboptimal just means a reconfigure would be more optimal, which
            // the next Resized handles.)
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // The surface needs reconfiguring (e.g. a resize raced us).
                gpu.surface.configure(device, &gpu.surface_config);
                return;
            },
            // Timeout / Occluded / Validation: skip this frame, try again.
            other => {
                eprintln!("[pelt-live-counter] surface acquire skipped: {other:?}");
                return;
            },
        };
        let target_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        gpu.renderer.compose_external_texture(
            &content_view,
            &target_view,
            gpu.surface_config.format,
            w,
            h,
            ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]),
        );

        // `compose_external_texture` already submitted its encoder (it owns the
        // device + queue internally), so the blit is queued; present the frame.
        frame.present();
    }

    /// Reconfigure the surface for `(width, height)` and request a redraw.
    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.surface_config.width = self.width;
            gpu.surface_config.height = self.height;
            gpu.surface
                .configure(&gpu.renderer.wgpu_device.core.device, &gpu.surface_config);
        }
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // 1. Window — created *invisible*: the accesskit adapter must be built
        //    before the window is first shown (it panics otherwise), so we show
        //    it at the end of resume, after the adapter exists.
        let attributes = Window::default_attributes()
            .with_title("Pelt Live — xilem-serval counter")
            .with_inner_size(PhysicalSize::new(self.width, self.height))
            .with_visible(false);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create pelt-live-counter window"),
        );
        let size = window.inner_size();
        self.width = size.width.max(1);
        self.height = size.height.max(1);

        // 1b. AccessKit adapter, while the window is still invisible. The
        //     deferred-event model: a11y requests arrive as
        //     `UserEvent::Accessibility` via the proxy; we answer them (and push
        //     tree updates on state changes) through `push_a11y`.
        self.adapter = Some(Adapter::with_event_loop_proxy(
            event_loop,
            &window,
            self.proxy.clone(),
        ));

        // 2. wgpu handles via netrender::boot (standalone instance/adapter/
        //    device/queue), then the netrender renderer over them.
        let handles = match netrender::boot() {
            Ok(handles) => handles,
            Err(err) => {
                eprintln!("[pelt-live-counter] netrender wgpu boot failed: {err}");
                event_loop.exit();
                return;
            },
        };

        // 3. Surface over the window, on the booted instance. The window is
        //    Arc-held so the surface can be `'static`.
        let surface = match handles.instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("[pelt-live-counter] create_surface failed: {err}");
                event_loop.exit();
                return;
            },
        };

        // 4. Surface configuration. Prefer an sRGB format from the adapter's
        //    supported set (the typical desktop backbuffer is Bgra8UnormSrgb);
        //    `compose_external_texture` builds its blit pipeline for whatever
        //    format we pick, so any supported format works.
        let caps = surface.get_capabilities(&handles.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: self.width,
            height: self.height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };

        let renderer = match netrender::create_netrender_instance(
            handles,
            NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            },
        ) {
            Ok(renderer) => renderer,
            Err(err) => {
                eprintln!("[pelt-live-counter] netrender init failed: {err:?}");
                event_loop.exit();
                return;
            },
        };
        surface.configure(&renderer.wgpu_device.core.device, &surface_config);

        self.gpu = Some(Gpu {
            surface,
            surface_config,
            renderer,
        });
        // The adapter exists now, so it is safe to show the window.
        window.set_visible(true);
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Tick => {
                // Timer tick: bump the count through the runner (state → DOM
                // diff), then push the updated a11y tree and redraw.
                self.runner.update(|s| s.count += 1);
                self.push_a11y();
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },
            UserEvent::Accessibility(event) => match event.window_event {
                // A screen reader activated (or re-requested): hand it the tree.
                AkWindowEvent::InitialTreeRequested => self.push_a11y(),
                // SR-initiated actions (activate / focus a node) are a follow-up:
                // mapping `ActionRequest` -> `dispatch_click`/`set_focus` wants a
                // screen reader in the loop to verify, so for now the tree is
                // read-only (perceivable, not actuable via a11y).
                AkWindowEvent::ActionRequested(_) => {},
                AkWindowEvent::AccessibilityDeactivated => {},
            },
        }
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

        // Let the accesskit adapter observe every window event (focus, resize,
        // etc.) before we handle it. Borrows the adapter + window fields (disjoint).
        if let (Some(adapter), Some(window)) = (self.adapter.as_mut(), self.window.as_ref()) {
            adapter.process_event(window, &event);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => self.resize(size.width, size.height),

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
            },

            WindowEvent::ModifiersChanged(mods) => {
                // Track modifiers so each KeyEvent carries them (Shift+Tab, …).
                let s = mods.state();
                self.modifiers = Modifiers {
                    shift: s.shift_key(),
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                };
            },

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Click input: hit-test the cursor through serval's existing
                // query, then dispatch a PointerClick to the hit node. If the
                // hit lands on (or under) the `[ + ]` button, its handler bumps
                // the count and the runner rebuilds.
                let (x, y) = self.cursor;
                let hit = hit_test_node(&self.dom.borrow(), SHEET, self.width, self.height, x, y);
                if let Some(node) = hit {
                    self.runner
                        .dispatch_click(node, PointerClick { local: (x, y) });
                    self.push_a11y();
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            },

            WindowEvent::KeyboardInput { event, .. } => {
                // Keyboard input: only presses type (include auto-repeat so a
                // held key keeps typing); releases do nothing. Map winit's
                // logical key to the serval `KeyEvent` and dispatch it to the
                // focused node — which `dispatch_click` set to the text field
                // when it was clicked. Keys with no text and no named mapping
                // (e.g. dead keys) are skipped.
                if event.state == ElementState::Pressed {
                    if let Some(key_event) = key_event_from_winit(&event.logical_key, self.modifiers) {
                        self.runner.dispatch_key(key_event);
                        self.push_a11y();
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                }
            },

            WindowEvent::RedrawRequested => {
                self.render();
                // Continuous loop: keep redrawing so the timer-driven climb is
                // always reflected promptly (and the window stays responsive).
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            },

            _ => {},
        }
    }
}

fn main() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build event loop");

    // The ~1Hz timer: a background thread sleeps 1s and sends a Tick through
    // the proxy. It runs for the program's lifetime; send errors mean the loop
    // has exited, at which point the thread ends.
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if proxy.send_event(UserEvent::Tick).is_err() {
                break;
            }
        }
    });

    let mut app = App::new(event_loop.create_proxy());
    event_loop
        .run_app(&mut app)
        .expect("pelt-live-counter event loop failed");
}
