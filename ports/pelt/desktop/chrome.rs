/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Browser chrome as Cambium views (V2): an omnibar + back/forward toolbar over
//! a **second** `ScriptedDom`, separate from the content document.
//!
//! The chrome is a styled, view-driven `ScriptedDom` rendered through the same
//! GPU-free path V4's scripted documents use — "chrome is a document, in the trusted
//! ring." The separate-roots discipline is the trust boundary: content can never reach
//! the chrome's tree, and the chrome talks to the content root only by emitting a
//! [`ChromeIntent`] the shell applies. That seam is deliberately extensible (a
//! `Custom` arm) so a later sanctioned mod tier (CSS theming today; Lua-via-piccolo /
//! JS mods later) can extend the chrome without a contract change. The default
//! stylesheet ([`Chrome`] carries it) is the theming seam: a user theme layers over it.
//!
//! This module is the GPU-free foundation (state, history, the view, render-to-scene).
//! The windowed two-root shell (compositing the strip beside the content, routing
//! input by side) is the integration step on top.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, KeyEvent, PointerClick, TextField,
    TextInput, el, lens, on_click, text_field_typed,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_render::scene_from_scripted_dom;
use genet_scripted_dom::{NodeId, ScriptedDom};
use netrender::Scene;

/// Which side of the window the chrome strip occupies. A horizontal strip (top/bottom)
/// is the full window width by its thickness; a vertical strip (left/right) is its
/// thickness by the full height.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StripSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl StripSide {
    /// A top/bottom strip runs horizontally (width × thickness).
    pub fn is_horizontal(self) -> bool {
        matches!(self, StripSide::Top | StripSide::Bottom)
    }
}

/// An intent the chrome emits for the shell to apply — the only channel from the chrome
/// root to the content root. The built-in navigation set, plus a `Custom` arm a
/// sanctioned mod can emit (the extensibility hook; the shell or another mod routes it).
#[derive(Clone, Debug, PartialEq)]
pub enum ChromeIntent {
    /// Load a URL into the content root (an omnibar submit).
    Navigate(String),
    /// Step back / forward in history.
    Back,
    Forward,
    /// Reload / stop the content root.
    Reload,
    Stop,
    /// Focus the omnibar (a keyboard shortcut target).
    FocusOmnibar,
    /// Zoom the content by a step.
    Zoom(f32),
    /// Open a settings page (a namespaced [`SettingsRef`](genet_host_api::tile::SettingsRef)).
    OpenSettings(String),
    /// A mod-defined intent (name, payload) — the extensibility escape hatch.
    Custom(String, String),
}

/// The chrome's app state: the omnibar edit buffer, the navigation history, and the
/// queue of intents the view handlers raise for the shell to drain.
pub struct ChromeState {
    /// The omnibar edit buffer (a real `TextInput`, full editing + IME).
    pub omnibar: TextInput,
    /// Visited URLs; `pos` indexes the current one.
    history: Vec<String>,
    pos: usize,
    /// Intents queued by view handlers (button clicks), drained by [`Chrome::take_intents`].
    pending: Vec<ChromeIntent>,
}

impl ChromeState {
    /// Start at `url` (the initial content URL), with it as the sole history entry.
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            omnibar: TextInput::new(url.clone()),
            history: vec![url],
            pos: 0,
            pending: Vec::new(),
        }
    }

    /// The current history URL.
    pub fn current(&self) -> &str {
        &self.history[self.pos]
    }

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.history.len()
    }

    /// Navigate to a new URL: drop any forward history, push, advance, and reset the
    /// omnibar to the new URL.
    pub fn navigate(&mut self, url: impl Into<String>) {
        let url = url.into();
        self.history.truncate(self.pos + 1);
        self.history.push(url.clone());
        self.pos = self.history.len() - 1;
        self.omnibar = TextInput::new(url);
    }

    /// Step back; returns whether it moved (false at the start of history).
    pub fn back(&mut self) -> bool {
        if self.can_back() {
            self.pos -= 1;
            self.sync_omnibar();
            true
        } else {
            false
        }
    }

    /// Step forward; returns whether it moved (false at the end of history).
    pub fn forward(&mut self) -> bool {
        if self.can_forward() {
            self.pos += 1;
            self.sync_omnibar();
            true
        } else {
            false
        }
    }

    fn sync_omnibar(&mut self) {
        self.omnibar = TextInput::new(self.current().to_string());
    }

    fn queue(&mut self, intent: ChromeIntent) {
        self.pending.push(intent);
    }
}

/// The erased chrome view type, so the toolbar's concrete `El<…>` tuple need not be
/// spelled (it grows as the chrome does). Mirrors meerkat's `ChromeView`.
pub type ChromeView = Box<dyn AnyView<ChromeState, (), GenetCtx, GenetElement>>;
type ChromeLogic = fn(&ChromeState) -> ChromeView;

fn go_back(c: &mut ChromeState, _: PointerClick) {
    c.queue(ChromeIntent::Back);
}

fn go_forward(c: &mut ChromeState, _: PointerClick) {
    c.queue(ChromeIntent::Forward);
}

/// The chrome toolbar as genet DOM: back / forward buttons and an editable omnibar
/// (`text_field` lensed onto [`ChromeState::omnibar`]). A spent direction carries a
/// `disabled` class the default sheet greys (the handler is already a no-op at the
/// history edge).
fn chrome_view(c: &ChromeState) -> ChromeView {
    let back_class = if c.can_back() { "nav" } else { "nav disabled" };
    let fwd_class = if c.can_forward() {
        "nav"
    } else {
        "nav disabled"
    };
    let back = on_click(
        el::<_, ChromeState, ()>("button", "back").attr("class", back_class),
        go_back as fn(&mut ChromeState, PointerClick),
    );
    let forward = on_click(
        el::<_, ChromeState, ()>("button", "forward").attr("class", fwd_class),
        go_forward as fn(&mut ChromeState, PointerClick),
    );
    // The omnibar text_field, lensed onto `ChromeState::omnibar`. `text_field_typed`
    // names its concrete view so the `lens` projection is a plain `fn` pointer.
    let make: fn(&mut TextInput) -> TextField = |t: &mut TextInput| text_field_typed(t);
    let to_omnibar: fn(&mut ChromeState) -> &mut TextInput = |c: &mut ChromeState| &mut c.omnibar;
    let omnibar = lens(make, to_omnibar);
    let toolbar =
        el::<_, ChromeState, ()>("div", (back, forward, omnibar)).attr("class", "toolbar");
    Box::new(toolbar)
}

/// The default chrome stylesheet — the theming seam. A user theme layers over (or
/// replaces) this; the chrome is CSS-styled DOM, so theming is "add CSS," not a
/// refactor. (Structural display defaults + a dark toolbar.)
const DEFAULT_CHROME_CSS: &str = "\
    div, button, span { display: block; } \
    head, style, script, title, meta, link, base { display: none; } \
    .toolbar { display: flex; align-items: center; background: #2b2b33; padding: 6px; } \
    button { padding: 4px 10px; margin-right: 6px; background: #444444; color: #eeeeee; } \
    button.disabled { color: #888888; }";

/// A view-driven chrome strip: a [`GenetAppRunner`] over its own `ScriptedDom`, plus
/// the strip placement and the stylesheets it renders with.
pub struct Chrome {
    runner: GenetAppRunner<ChromeState, ChromeLogic, ChromeView, ()>,
    side: StripSide,
    thickness: u32,
    sheets: Vec<String>,
}

impl Chrome {
    /// Build the chrome for an initial content `url`, placed on `side` with `thickness`
    /// px (the strip's height for top/bottom, width for left/right).
    pub fn new(url: impl Into<String>, side: StripSide, thickness: u32) -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::new(dom, chrome_view as ChromeLogic, ChromeState::new(url));
        Self {
            runner,
            side,
            thickness,
            sheets: vec![DEFAULT_CHROME_CSS.to_string()],
        }
    }

    pub fn side(&self) -> StripSide {
        self.side
    }

    pub fn thickness(&self) -> u32 {
        self.thickness
    }

    /// The chrome's app state (for the shell to read history / the omnibar).
    pub fn state(&self) -> &ChromeState {
        self.runner.state()
    }

    /// Append a user theme stylesheet (the theming seam). Later sheets cascade over the
    /// default, so a theme can recolor / relayout the chrome without touching this code.
    pub fn add_stylesheet(&mut self, css: impl Into<String>) {
        self.sheets.push(css.into());
    }

    /// Render the chrome strip to a [`Scene`] at `width`×`height` (the shell sizes it
    /// from [`side`](Self::side) + [`thickness`](Self::thickness)).
    pub fn frame(&self, width: u32, height: u32) -> Scene {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        scene_from_scripted_dom(
            &dom,
            &sheets,
            width.max(1),
            height.max(1),
            None,
            &ScrollOffsets::default(),
        )
    }

    /// Drain the intents the view handlers queued and apply the navigational ones to
    /// the chrome's own history (so [`state().current()`](ChromeState::current) is the
    /// URL to display). Returns the raw intents so the shell can act on them too
    /// (loading the resolved URL into the content root, handling Reload/Zoom/Custom…).
    pub fn take_intents(&mut self) -> Vec<ChromeIntent> {
        let mut intents = Vec::new();
        self.runner
            .update(|s| intents = std::mem::take(&mut s.pending));
        for intent in &intents {
            match intent {
                ChromeIntent::Back => self.runner.update(|s| {
                    s.back();
                }),
                ChromeIntent::Forward => self.runner.update(|s| {
                    s.forward();
                }),
                ChromeIntent::Navigate(url) => {
                    let url = url.clone();
                    self.runner.update(move |s| s.navigate(url));
                },
                _ => {},
            }
        }
        intents
    }

    /// Submit the omnibar: queue a `Navigate` to its current text (the shell calls this
    /// when Enter is pressed with the omnibar focused).
    pub fn submit_omnibar(&mut self) {
        let url = self.runner.state().omnibar.text().to_string();
        self.runner
            .update(|s| s.queue(ChromeIntent::Navigate(url.clone())));
    }

    /// Queue a navigation to `url` (a content link click the shell resolved to a URL).
    /// Mirrors [`submit_omnibar`](Self::submit_omnibar) with an explicit target, so a
    /// followed link walks the same history + omnibar path as a typed URL.
    pub fn navigate_to(&mut self, url: impl Into<String>) {
        let url = url.into();
        self.runner
            .update(move |s| s.queue(ChromeIntent::Navigate(url.clone())));
    }

    /// Paste `text` into the omnibar at the caret, replacing any selection. The shell
    /// reads the OS clipboard and calls this on Ctrl/Cmd+V while the omnibar is focused.
    pub fn paste(&mut self, text: &str) {
        let text = text.to_string();
        self.runner.update(move |s| s.omnibar.insert_str(&text));
    }

    /// The shared chrome DOM handle (for the shell's hit-testing).
    pub fn dom(&self) -> DomHandle {
        self.runner.dom()
    }

    /// Dispatch a native click that hit chrome node `target` (the shell resolves the
    /// point → node), routing it to the toolbar's handlers.
    pub fn dispatch_click(&mut self, target: NodeId, event: PointerClick) {
        self.runner.dispatch_click(target, event);
    }

    /// Dispatch a key to the focused chrome element (the omnibar, when focused) — the
    /// shell routes keystrokes here while the chrome holds focus.
    pub fn dispatch_key(&mut self, event: KeyEvent) {
        self.runner.dispatch_key(event);
    }

    /// The focused chrome node, if any. The shell reads this to decide whether
    /// keystrokes go to the chrome (omnibar editing) or the content (scroll keys).
    pub fn focused(&self) -> Option<NodeId> {
        self.runner.focus()
    }

    /// Hit-test the chrome DOM at strip-local `(x, y)`, laying it out at the strip
    /// size, so the shell can resolve a click in the strip to a chrome node.
    pub fn hit_test(&self, x: f32, y: f32, width: u32, height: u32) -> Option<NodeId> {
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let dom = self.runner.dom();
        let dom = dom.borrow();
        // `&*dom` (not `&dom`): `IncrementalLayout::new` is generic over `D: LayoutDom`,
        // so it must see `&ScriptedDom`, not `&Ref<ScriptedDom>` (no deref coercion into
        // a generic param).
        let session =
            IncrementalLayout::new(&*dom, &sheets, width.max(1) as f32, height.max(1) as f32);
        session.hit_test(&*dom, x, y, &ScrollOffsets::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// History walks: navigate pushes + advances, back/forward move and gate at the
    /// edges, and a fresh navigate truncates forward history.
    #[test]
    fn history_walks() {
        let mut s = ChromeState::new("a");
        assert_eq!(s.current(), "a");
        assert!(!s.can_back() && !s.can_forward());

        s.navigate("b");
        s.navigate("c");
        assert_eq!(s.current(), "c");
        assert!(s.can_back() && !s.can_forward());

        assert!(s.back());
        assert_eq!(s.current(), "b");
        assert!(s.back());
        assert_eq!(s.current(), "a");
        assert!(!s.back(), "gated at the start");

        assert!(s.forward());
        assert_eq!(s.current(), "b");

        // Navigating from the middle drops the forward entry ("c").
        s.navigate("d");
        assert_eq!(s.current(), "d");
        assert!(!s.can_forward());
        assert_eq!(s.omnibar.text(), "d", "omnibar reflects the new URL");
    }

    /// The chrome renders its toolbar to a scene with painted text (the omnibar URL +
    /// the button labels) — the GPU-free render path works.
    #[test]
    fn chrome_renders_toolbar_text() {
        let chrome = Chrome::new("example.org", StripSide::Top, 40);
        let scene = chrome.frame(800, 40);
        assert!(
            scene
                .ops
                .iter()
                .any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "the toolbar paints text (buttons + omnibar URL)",
        );
    }

    /// Clicking the back button queues a Back intent, and `take_intents` applies it to
    /// history — the on_click → intent → history flow, GPU-free (no layout hit-test
    /// needed: the handler is found by routing to the button node).
    #[test]
    fn back_button_click_steps_history() {
        let mut chrome = Chrome::new("a", StripSide::Top, 40);
        // Build some history so Back has somewhere to go.
        chrome.runner_navigate("b");
        assert_eq!(chrome.state().current(), "b");

        // Find the "back" button node in the chrome DOM and dispatch a click to it.
        let back_node = find_button(&chrome, "back").expect("back button exists");
        chrome.dispatch_click(back_node, PointerClick::at((0.0, 0.0)));

        let intents = chrome.take_intents();
        assert!(
            intents.contains(&ChromeIntent::Back),
            "click queued Back: {intents:?}"
        );
        assert_eq!(chrome.state().current(), "a", "Back stepped history");
    }

    /// A followed content link (`navigate_to`) queues a Navigate that `take_intents`
    /// applies to history + the omnibar — the same path a typed URL walks.
    #[test]
    fn navigate_to_advances_history_and_omnibar() {
        let mut chrome = Chrome::new("a.html", StripSide::Top, 40);
        chrome.navigate_to("b.html");
        let intents = chrome.take_intents();
        assert!(
            intents.contains(&ChromeIntent::Navigate("b.html".to_string())),
            "navigate_to queued a Navigate: {intents:?}",
        );
        assert_eq!(
            chrome.state().current(),
            "b.html",
            "history advanced to the link target"
        );
        assert_eq!(
            chrome.state().omnibar.text(),
            "b.html",
            "the omnibar shows the new URL"
        );
    }

    /// Pasting inserts text into the omnibar at the caret (the shell supplies the
    /// clipboard string; the insertion itself is host-independent and testable).
    #[test]
    fn paste_inserts_into_omnibar() {
        let mut chrome = Chrome::new("a.html", StripSide::Top, 40);
        let before = chrome.state().omnibar.text().to_string();
        chrome.paste("ZZ");
        let after = chrome.state().omnibar.text();
        assert!(
            after.contains("ZZ"),
            "the pasted text is in the omnibar: {after}"
        );
        assert!(
            after.len() > before.len(),
            "the omnibar grew by the pasted text"
        );
    }

    impl Chrome {
        /// Test helper: navigate directly (the shell would do this on an omnibar submit).
        fn runner_navigate(&mut self, url: &str) {
            let url = url.to_string();
            self.runner.update(move |s| s.navigate(url));
        }
    }

    /// Walk the chrome DOM for a `<button>` whose text is `label`.
    fn find_button(chrome: &Chrome, label: &str) -> Option<NodeId> {
        use layout_dom_api::LayoutDom;
        let dom = chrome.dom();
        let dom = dom.borrow();
        let mut stack = vec![dom.document()];
        while let Some(node) = stack.pop() {
            if dom
                .element_name(node)
                .is_some_and(|q| q.local.as_ref() == "button")
            {
                let text: String = dom
                    .dom_children(node)
                    .filter_map(|c| dom.text(c).map(str::to_string))
                    .collect();
                if text == label {
                    return Some(node);
                }
            }
            for child in dom.dom_children(node) {
                stack.push(child);
            }
        }
        None
    }
}
