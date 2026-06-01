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

use layout_dom_api::{LayoutDom, LocalName, Namespace};
use serval_layout::ScrollOffsets;
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions, Renderer, Scene};
use serval_scripted_dom::{NodeId, ScriptedDom};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};
use xilem_serval::{
    AnyView, El, Key, KeyEvent, Lens, Modifiers, NamedKey, OnClick, Placement, PointerClick,
    PointerEvent, PointerPhase, SelectState, ServalCtx, ServalElement, ServalAppRunner, Slider,
    TextField, TextInput, anchor_point, el, lens, on_click, overlay_at, select, slider,
    text_field_typed,
};

use accesskit_winit::{Adapter, Event as AkEvent, WindowEvent as AkWindowEvent};
use pelt_live::{
    accesskit_tree, caret_screen_rect, fragments_from_scripted_dom, hit_test_node,
    scene_from_scripted_dom, TextCursor,
};

// ── App state + view ───────────────────────────────────────────────────────

/// The app state: a counter plus an editable text field. The counter is the
/// Stage 1b probe; `field` (a [`TextInput`] — buffer + caret) is the Stage 3
/// form-control slice — a `text_field` lensed onto it edits it as you type, with
/// ←/→ moving the caret.
/// The colour options the demo's `select` dropdown offers.
const COLOURS: &[&str] = &["red", "green", "blue"];

struct Demo {
    count: u32,
    field: TextInput,
    /// The `select` dropdown's state (which colour, open/closed). Composed onto
    /// the view via `lens`, like `field`.
    colour: SelectState,
    /// The slider's value (0..1), drag-set via the pointer-drag foundation.
    volume: Slider,
    /// Whether the `[ + ]` button's popup overlay is showing. Toggled on each
    /// click of the button (which also still bumps the count).
    popup_open: bool,
    /// The popup's top-left, in the root `<div>`'s coordinate space — the
    /// `Below`-anchor of the button, recomputed by the host from the button's
    /// laid-out rect after each click (the button is static, so it settles
    /// immediately). The view reads it to place the overlay.
    popup_anchor: (f32, f32),
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
        // The `[ + ]` button's popup overlay — shown when `popup_open`, placed
        // last so it paints over everything before it (the paint walk has no
        // z-index). `Option<V>` is a `ViewSequence`, so this is the conditional
        // child slot.
        Option<El<&'static str, Demo, ()>>,
        // Erased (`Box<dyn AnyView>`) children — concrete types unnameable
        // (closures / `Vec`): the scrollable box (`overflow: scroll`,
        // wheel-scrolled), the `select` dropdown, and the drag slider.
        AnyDemoView,
        AnyDemoView,
        AnyDemoView,
    ),
    Demo,
    (),
>;

/// An erased child view over `Demo` — used for controls whose concrete type
/// can't be named in [`DemoView`].
type AnyDemoView = Box<dyn AnyView<Demo, (), ServalCtx, ServalElement>>;

fn demo_view(s: &Demo) -> DemoView {
    // Clicking `[ + ]` bumps the count *and* toggles its popup overlay, so a
    // click both advances the counter and shows/hides the anchored popup.
    let increment: fn(&mut Demo, PointerClick) = |s: &mut Demo, _ev| {
        s.count += 1;
        s.popup_open = !s.popup_open;
    };
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
            // The popup: an overlay anchored below the button (its `(x, y)` is
            // the host-computed `popup_anchor`), styled by the `.popup` class.
            s.popup_open.then(|| {
                overlay_at::<_, Demo, ()>(s.popup_anchor.0, s.popup_anchor.1, "more")
                    .attr("class", "popup")
            }),
            // The colour dropdown, lensed onto `Demo::colour`, boxed as an erased
            // view. Self-positions its option list (top: 100%), so unlike the
            // popup it needs no host anchor.
            // The scrollable box: eight lines exceeding its fixed height. The
            // host wheel-scrolls it; emit clips + translates the content. Placed
            // before the select so the select's dropdown (which opens over this
            // area) paints on top — the paint walk has no z-index, so stacking
            // is document order.
            Box::new(
                el::<_, Demo, ()>(
                    "div",
                    (1..=8)
                        .map(|i| {
                            // Clickable: now that hit-testing is clip + scroll
                            // aware, clicking a *scrolled* line logs its true
                            // index (and clicks at the scroller's level no longer
                            // leak onto the controls below).
                            on_click(
                                el::<_, Demo, ()>("p", format!("Scrollable line {i}")),
                                move |_: &mut Demo, _| tracing::info!(line = i, "line clicked"),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .attr("class", "scroller"),
            ) as AnyDemoView,
            // The colour dropdown, lensed onto `Demo::colour`, boxed as an erased
            // view. Self-positions its option list (top: 100%); z-index Tier 1
            // paints that (absolute) list on top regardless of sibling order.
            Box::new(lens(
                |c: &mut SelectState| select(c, COLOURS),
                |d: &mut Demo| &mut d.colour,
            )) as AnyDemoView,
            // The drag slider, lensed onto `Demo::volume` — press/drag the track
            // to set the value (via the pointer-drag foundation).
            Box::new(lens(
                |v: &mut Slider| slider(v),
                |d: &mut Demo| &mut d.volume,
            )) as AnyDemoView,
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
    // The root <div> is the positioned containing block for the popup overlay
    // (an absolute child resolves against the nearest positioned ancestor). The
    // overlay's own `position: absolute` rides an inline style, which outranks
    // this rule, so only the root is made relative.
    "div { position: relative; }",
    "p { font-size: 96px; color: rgb(30, 30, 50); }",
    "button { font-size: 48px; color: rgb(255, 255, 255); \
        background-color: rgb(60, 120, 220); padding: 12px; }",
    "label { font-size: 28px; color: rgb(60, 60, 80); padding: 8px; }",
    "input { font-size: 40px; color: rgb(20, 20, 20); \
        background-color: rgb(235, 238, 245); padding: 12px; }",
    // The popup overlay box: a small tinted card with padding, drawn on top.
    ".popup { font-size: 28px; color: rgb(40, 40, 40); \
        background-color: rgb(245, 230, 140); padding: 10px; }",
    // The colour dropdown: a clickable box; the option list (top: 100%) sits
    // just below it, each option a tappable row.
    ".select-box { font-size: 28px; color: rgb(20, 20, 20); \
        background-color: rgb(220, 225, 235); padding: 10px; }",
    ".select-list { background-color: rgb(255, 255, 255); }",
    ".select-option { font-size: 24px; color: rgb(30, 30, 40); \
        background-color: rgb(240, 242, 248); padding: 8px; }",
    // The scrollable box: fixed height with `overflow: scroll`; its eight lines
    // are taller than that, so the wheel scrolls them. `.scroller p` overrides
    // the big count-`<p>` font so the lines are list-sized.
    ".scroller { overflow: scroll; height: 140px; \
        background-color: rgb(250, 245, 235); padding: 8px; }",
    ".scroller p { font-size: 28px; color: rgb(60, 50, 40); padding: 4px; margin: 0; }",
    // The drag slider: a grey track holding a blue thumb. The thumb's
    // `position: absolute; left: <value>%` rides an inline style; its width/
    // height/colour come from here.
    ".slider-track { height: 28px; background-color: rgb(190, 194, 208); }",
    ".slider-thumb { width: 18px; height: 28px; background-color: rgb(60, 100, 200); }",
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

/// The first element named `tag` in `dom`, in document pre-order. Used to find
/// the `[ + ]` button to anchor its popup. (`pop` makes the walk depth-first;
/// order doesn't matter here — there is one button.)
fn find_element_by_tag(dom: &ScriptedDom, tag: &str) -> Option<NodeId> {
    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        if dom.element_name(id).is_some_and(|q| q.local.as_ref() == tag) {
            return Some(id);
        }
        queue.extend(dom.dom_children(id));
    }
    None
}

/// The first element whose `class` attribute contains `class` (whitespace-
/// separated), in document pre-order. Used to find the scroll container.
fn find_element_by_class(dom: &ScriptedDom, class: &str) -> Option<NodeId> {
    let ns = Namespace::from("");
    let local = LocalName::from("class");
    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        if dom
            .attribute(id, &ns, &local)
            .is_some_and(|c| c.split_whitespace().any(|t| t == class))
        {
            return Some(id);
        }
        queue.extend(dom.dom_children(id));
    }
    None
}

/// The scroll container's node, its box (location + size, root-relative — the
/// scroller is a direct child of the positioned root, so parent-relative ≈
/// absolute here), and its max vertical scroll extent (content beyond the
/// visible box). `None` if there is no `.scroller` or it has no fragment.
struct ScrollBox {
    node: NodeId,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scrollable_y: f32,
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
    /// The system clipboard, for Ctrl/Cmd+C/X/V. `None` if it failed to open.
    clipboard: Option<arboard::Clipboard>,
    /// The scrollable box's current scroll offset (device px). Updated by the
    /// mouse wheel, clamped to the content; applied at render by keying the
    /// scroller's node in the [`ScrollOffsets`] map passed to the scene builder.
    scroll_offset: (f32, f32),
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
                colour: SelectState::new(0),
                volume: Slider::new(0.5),
                popup_open: false,
                popup_anchor: (0.0, 0.0),
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
            clipboard: arboard::Clipboard::new().ok(),
            scroll_offset: (0.0, 0.0),
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

    /// Request a redraw if a window exists (after state-changing input).
    fn redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Handle a Ctrl/Cmd+C/X/V clipboard shortcut on the focused field, returning
    /// `true` if it was one (so the key isn't also treated as text input). No-op
    /// (`false`) without the modifier, without focus, or for other keys.
    fn handle_clipboard_shortcut(&mut self, key: &WinitKey) -> bool {
        if !(self.modifiers.ctrl || self.modifiers.meta) || self.runner.focus().is_none() {
            return false;
        }
        let WinitKey::Character(s) = key else {
            return false;
        };
        match s.as_str() {
            "c" => {
                self.clipboard_copy();
                true
            },
            "x" => {
                self.clipboard_cut();
                true
            },
            "v" => {
                self.clipboard_paste();
                true
            },
            _ => false,
        }
    }

    /// Copy the focused field's selection to the system clipboard.
    fn clipboard_copy(&mut self) {
        let text = self.runner.state().field.selected_text().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(text);
        }
    }

    /// Cut: copy the selection, then delete it. No-op without a selection.
    fn clipboard_cut(&mut self) {
        if !self.runner.state().field.has_selection() {
            return;
        }
        self.clipboard_copy();
        self.runner.update(|d| d.field.backspace()); // deletes the selection
        self.push_a11y();
        self.redraw();
    }

    /// Paste: insert the clipboard text at the caret, replacing any selection.
    fn clipboard_paste(&mut self) {
        let Some(text) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.runner.update(|d| d.field.insert_str(&text));
        self.push_a11y();
        self.redraw();
    }

    /// The popup's `Below`-anchor: the `[ + ]` button's laid-out rect, run
    /// through [`anchor_point`]. The button is a direct child of the root
    /// `<div>`, so its parent-relative fragment rect is already in the overlay's
    /// containing-block (root-`<div>`) space — no origin accumulation needed.
    /// `None` if there is no button / it has no fragment yet.
    fn button_anchor(&self) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let button = find_element_by_tag(&dom, "button")?;
        let frags = fragments_from_scripted_dom(&dom, SHEET, self.width, self.height);
        let r = frags.rect_of(button)?;
        Some(anchor_point(
            (r.location.x, r.location.y, r.size.width, r.size.height),
            (0.0, 0.0),
            Placement::Below,
        ))
    }

    /// Locate the `.scroller` and measure its box + scroll extent from a fresh
    /// layout. `scrollable_y` is how far the content overflows the visible box
    /// (the clamp ceiling for the wheel); `0` when it fits.
    fn scroll_box(&self) -> Option<ScrollBox> {
        let dom = self.dom.borrow();
        let node = find_element_by_class(&dom, "scroller")?;
        let frags = fragments_from_scripted_dom(&dom, SHEET, self.width, self.height);
        let r = frags.rect_of(node)?;
        let inner_h =
            r.size.height - r.padding.top - r.padding.bottom - r.border.top - r.border.bottom;
        Some(ScrollBox {
            node,
            x: r.location.x,
            y: r.location.y,
            w: r.size.width,
            h: r.size.height,
            scrollable_y: (r.content_size.height - inner_h).max(0.0),
        })
    }

    /// The scroller's node keyed to its current offset — the map handed to both
    /// paint (to scroll the content) and hit-testing (to map clicks through the
    /// scroll + clip). Empty when there is no scroller.
    fn scroll_offsets_map(&self) -> ScrollOffsets<NodeId> {
        let mut offsets = ScrollOffsets::default();
        if let Some(node) = find_element_by_class(&self.dom.borrow(), "scroller") {
            offsets.insert(node, self.scroll_offset);
        }
        offsets
    }

    /// A node's laid-out box as `(x, y, w, h)` in root-relative coords (≈
    /// absolute for the demo's top-level elements). Used to turn a window cursor
    /// into an element-local `PointerEvent` for the drag slider. `None` if the
    /// node has no fragment.
    fn element_rect(&self, node: NodeId) -> Option<(f32, f32, f32, f32)> {
        let dom = self.dom.borrow();
        let frags = fragments_from_scripted_dom(&dom, SHEET, self.width, self.height);
        let r = frags.rect_of(node)?;
        Some((r.location.x, r.location.y, r.size.width, r.size.height))
    }

    /// Build a [`PointerEvent`] for `node` at the current cursor: local =
    /// cursor minus the node's origin, size = the node's box. `None` if the node
    /// has no fragment.
    fn pointer_event_for(&self, node: NodeId, phase: PointerPhase) -> Option<PointerEvent> {
        let (rx, ry, rw, rh) = self.element_rect(node)?;
        let (x, y) = self.cursor;
        Some(PointerEvent { phase, local: (x - rx, y - ry), size: (rw, rh) })
    }

    /// Point the IME candidate window at the focused field's caret (IME T3): the
    /// caret's screen rect from `caret_screen_rect`, reported via
    /// `set_ime_cursor_area`. No-op without a focused field. Called after input
    /// that moves focus or the caret (and on commit), so the candidate popup
    /// tracks the cursor.
    fn update_ime_cursor_area(&self) {
        let (Some(window), Some(node)) = (self.window.as_ref(), self.runner.focus()) else {
            return;
        };
        // Caret byte within the field's *rendered* text (after any preedit), so
        // the candidate window sits after the composing run.
        let caret_byte = self.runner.state().field.caret_byte_in_render();
        if let Some((x, y, w, h)) =
            caret_screen_rect(&self.dom.borrow(), SHEET, self.width, self.height, node, caret_byte)
        {
            window.set_ime_cursor_area(
                PhysicalPosition::new(x, y),
                PhysicalSize::new(w.max(1.0), h.max(1.0)),
            );
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

        // 1. Engine pipeline → Scene. When a field is focused, paint its caret
        //    and selection: the focused element plus the field's caret / selection
        //    converted from char indices to byte offsets.
        let cursor = self.runner.focus().map(|node| {
            let field = &self.runner.state().field;
            let byte_of = |char_idx: usize| {
                field
                    .text()
                    .char_indices()
                    .nth(char_idx)
                    .map(|(b, _)| b)
                    .unwrap_or(field.text().len())
            };
            let selection = field
                .has_selection()
                .then(|| {
                    let (s, e) = field.selection();
                    (byte_of(s), byte_of(e))
                });
            // Caret byte within the field's rendered text — after any IME preedit
            // spliced at the caret (so the painted caret sits after the composing
            // run). With no preedit this equals `byte_of(field.caret())`.
            TextCursor { node, caret: field.caret_byte_in_render(), selection }
        });
        // Key the scroller's node with its current offset so emit scrolls it.
        let scroll_offsets = self.scroll_offsets_map();
        let scene: Scene =
            scene_from_scripted_dom(&self.dom.borrow(), SHEET, w, h, cursor, &scroll_offsets);

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

        // Allow the platform IME so composed input (CJK, transliteration, dead-key
        // accents) is delivered as `WindowEvent::Ime` — `Commit` text inserts into
        // the focused field (IME T1). `set_ime_cursor_area` (T3) follows the caret.
        window.set_ime_allowed(true);

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
                // Drive an in-progress pointer drag: route a Move to the captured
                // element (local coords measured from its rect + the new cursor).
                if let Some(node) = self.runner.pointer_capture() {
                    if let Some(ev) = self.pointer_event_for(node, PointerPhase::Move) {
                        self.runner.dispatch_pointer_move(ev);
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                }
            },

            WindowEvent::MouseWheel { delta, .. } => {
                // Convert to pixels (≈30 px/line) and scroll the box under the
                // cursor. Wheel up (positive) reveals earlier content, so the
                // offset decreases; clamp to the content's scroll extent.
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 30.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if let Some(sb) = self.scroll_box() {
                    let (cx, cy) = self.cursor;
                    let inside =
                        cx >= sb.x && cx <= sb.x + sb.w && cy >= sb.y && cy <= sb.y + sb.h;
                    if inside {
                        let new_y = (self.scroll_offset.1 - dy).clamp(0.0, sb.scrollable_y);
                        if new_y != self.scroll_offset.1 {
                            self.scroll_offset.1 = new_y;
                            tracing::info!(
                                offset_y = new_y,
                                scrollable = sb.scrollable_y,
                                "scroll"
                            );
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    }
                }
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
                // Clip-aware hit-test: pass the scroll offsets so the engine maps
                // the click through scrolled containers and clips to overflow
                // boxes (a click inside a scrolled box finds the scrolled content,
                // and a click at the box's level no longer leaks onto the element
                // below it).
                let offsets = self.scroll_offsets_map();
                let hit =
                    hit_test_node(&self.dom.borrow(), SHEET, self.width, self.height, x, y, &offsets);
                match hit {
                    Some(node) => {
                        let tag = self
                            .dom
                            .borrow()
                            .element_name(node)
                            .map(|q| q.local.to_string());
                        tracing::debug!(x, y, ?node, ?tag, "left click → hit");
                    },
                    None => tracing::debug!(x, y, "left click → miss"),
                }
                // A press on a pointer-drag target (the slider track / thumb)
                // starts a drag instead of a click. Measure the *captured*
                // element (the track) for local coords, not the hit (maybe the
                // thumb child).
                let drag_target = hit.and_then(|h| self.runner.pointer_target(h));
                if let Some(target) = drag_target {
                    if let Some(ev) = self.pointer_event_for(target, PointerPhase::Down) {
                        let vol = ev.local.0 / ev.size.0.max(1.0);
                        self.runner.dispatch_pointer_down(target, ev);
                        tracing::info!(volume = vol.clamp(0.0, 1.0), "pointer down (drag)");
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                } else if let Some(node) = hit {
                    let actions = self
                        .runner
                        .dispatch_click(node, PointerClick::at((x, y)));
                    if !actions.is_empty() {
                        tracing::debug!(n = actions.len(), "click bubbled actions to root");
                    }
                    // The click may have toggled the button's popup. If it is now
                    // open, (re)place it under the button from the freshly
                    // laid-out button rect via `anchor_point`. The button is
                    // static, so this settles on the first open.
                    if self.runner.state().popup_open {
                        if let Some(anchor) = self.button_anchor() {
                            if anchor != self.runner.state().popup_anchor {
                                self.runner.update(|d| d.popup_anchor = anchor);
                            }
                        }
                    }
                    let s = self.runner.state();
                    tracing::info!(
                        count = s.count,
                        popup_open = s.popup_open,
                        colour_selected = s.colour.selected,
                        colour_open = s.colour.open,
                        "click handled"
                    );
                    self.push_a11y();
                    // Focus may have moved (click-to-focus); point the IME at the
                    // newly-focused field's caret.
                    self.update_ime_cursor_area();
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
            },

            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // End an in-progress pointer drag.
                if let Some(node) = self.runner.pointer_capture() {
                    if let Some(ev) = self.pointer_event_for(node, PointerPhase::Up) {
                        self.runner.dispatch_pointer_up(ev);
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
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
                    // Ctrl/Cmd+C/X/V are clipboard shortcuts on the focused field,
                    // intercepted before text input (so "c"/"x"/"v" with the
                    // modifier don't type). Otherwise map + dispatch the key.
                    if self.handle_clipboard_shortcut(&event.logical_key) {
                        // handled as a clipboard op (it did its own redraw).
                    } else if let Some(key_event) =
                        key_event_from_winit(&event.logical_key, self.modifiers)
                    {
                        tracing::debug!(?key_event, focus = ?self.runner.focus(), "key → dispatch");
                        self.runner.dispatch_key(key_event);
                        self.push_a11y();
                        self.update_ime_cursor_area();
                        self.redraw();
                    }
                }
            },

            WindowEvent::Ime(ime) => match ime {
                // IME T2: show the in-progress composition inline at the caret
                // (not yet committed). `set_preedit` makes the field render the
                // composing text spliced at the caret.
                Ime::Preedit(text, _cursor) => {
                    tracing::debug!(preedit = %text, "ime preedit");
                    self.runner.update(|d| d.field.set_preedit(text));
                    self.update_ime_cursor_area();
                    self.redraw();
                },
                // IME T1: composition finished — clear the preedit, then insert
                // the committed text via the focus-routed text path (a `Character`
                // key event). Latin typing still arrives as `KeyboardInput`; CJK /
                // transliteration / dead-key accents commit here.
                Ime::Commit(text) => {
                    tracing::info!(text = %text, "ime commit");
                    self.runner.update(|d| d.field.clear_preedit());
                    if !text.is_empty() {
                        self.runner.dispatch_key(KeyEvent::new(Key::Character(text)));
                    }
                    self.push_a11y();
                    self.update_ime_cursor_area();
                    self.redraw();
                },
                Ime::Enabled => tracing::debug!("ime enabled"),
                Ime::Disabled => {
                    tracing::debug!("ime disabled");
                    self.runner.update(|d| d.field.clear_preedit());
                    self.redraw();
                },
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
    // Runtime diagnostics → stdout (the launcher pipes it to a readable log).
    // Defaults to `pelt_live=info` when `RUST_LOG` is unset, so the interaction
    // trace shows without any env setup; `RUST_LOG=pelt_live=debug` adds detail.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pelt_live=info")),
        )
        .init();
    tracing::info!("pelt-live-counter starting");

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
