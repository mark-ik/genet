/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `pelt-live`: a headless Xilem-on-serval host probe.
//!
//! Stage 1b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`. It pairs
//! [`xilem_serval::ServalAppRunner`] (state → view tree → DOM diff) with a
//! headless render driver ([`render::scene_from_scripted_dom`]: `ScriptedDom` →
//! `netrender::Scene`) and proves the whole spine end to end, offline:
//!
//! ```text
//! app state --(ServalAppRunner)--> ScriptedDom diff --(serval-layout)--> layout
//!                                                    --(paint emit)----> PaintList
//!                                                    --(paint)---------> netrender::Scene
//! ```
//!
//! No window and no input yet (input is Stage 2). The render side is GPU-free —
//! a `pelt` host binary would feed the produced `Scene` to netrender/wgpu, but
//! the probe asserts on the `Scene`/layout directly. This is `xilem-serval`'s
//! consumer; `xilem-serval` itself stays thin (no serval-layout/netrender dep),
//! and this crate carries the engine stack.

pub mod render;

pub use render::{fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};
    use xilem_serval::{
        El, Key, KeyEvent, NamedKey, OnClick, OnKey, PointerClick, ServalAppRunner, ServalCtx,
        ServalElement, View, el, lens, map_action, on_click, on_key,
    };

    use crate::render::{fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

    /// The app state under test.
    struct Counter {
        count: u32,
    }

    /// App logic: `<div id="counter">{count}</div>`. The `id` lets author CSS
    /// target the element; the text child carries the count.
    fn counter_view(s: &Counter) -> El<String, Counter, ()> {
        el::<_, Counter, ()>("div", s.count.to_string()).attr("id", "counter")
    }

    /// The author stylesheet: make the `<div>` a block box so layout reaches it.
    const SHEET: &[&str] = &["div { display: block; }"];

    /// The text data of the (single) text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// End-to-end: tick a counter 0 → 1 → 2 → 3 through the runner, and after
    /// each tick assert (a) the DOM text under the div equals the count and
    /// (b) the serval render path (cascade → layout → paint) reflects the
    /// state — the div is laid out, and the paint list translates to a Scene.
    #[test]
    fn counter_renders_through_serval_end_to_end() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });

        // Tick 0 (initial build) through 3.
        for expected in 0..=3u32 {
            if expected > 0 {
                runner.update(|s| s.count += 1);
            }
            assert_eq!(runner.state().count, expected);

            let dom = runner.dom();
            let dom_ref = dom.borrow();
            let root = runner.root();

            // (a) State change reached the DOM: the text under the div is the
            // current count.
            assert_eq!(
                text_child(&dom_ref, root).as_deref(),
                Some(expected.to_string().as_str()),
                "tick {expected}: DOM text under div must equal the count"
            );

            // (b1) Layout reflects the DOM: the <div> was reached by layout and
            // got a fragment. This is the plan's fallback assertion level and
            // holds regardless of glyph emission.
            let fragments = fragments_from_scripted_dom(&dom_ref, SHEET, 800, 600);
            assert!(
                fragments.rect_of(root).is_some(),
                "tick {expected}: <div> must be laid out (got a fragment)"
            );

            // (b2) Paint emission + Scene translation builds without panic over
            // the live ScriptedDom (paint-over-ScriptedDom works — no fallback
            // needed). The Scene carries the requested viewport.
            let scene = scene_from_scripted_dom(&dom_ref, SHEET, 800, 600);
            assert_eq!(scene.viewport_width, 800);
            assert_eq!(scene.viewport_height, 600);
            // The counter's text paints: emission produces at least one draw op
            // (a glyph run for the digit). Non-empty ops confirm the paint half
            // ran over the live ScriptedDom, not just layout.
            assert!(
                !scene.ops.is_empty(),
                "tick {expected}: paint emission should produce draw ops"
            );
        }
    }

    /// Stage 2a: serval's existing hit-test query, wired over the live
    /// `ScriptedDom`. A point inside the laid-out `<div>` recovers a live node in
    /// its subtree; a point far outside recovers nothing. This is the
    /// `point → NodeId` half of input dispatch (the dispatch walk + handlers are
    /// Stage 2b).
    #[test]
    fn hit_test_recovers_live_node_in_subtree() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });
        let root = runner.root();

        let dom_ref = dom.borrow();
        // (5, 5) is inside the top-left block <div> (full-width, one text line
        // tall), so the hit-test lands on the div or its text child.
        let hit = hit_test_node(&dom_ref, SHEET, 800, 600, 5.0, 5.0)
            .expect("a point inside the div should hit something");
        assert!(
            hit == root || dom_ref.parent(hit) == Some(root),
            "hit {hit:?} should be the div or a child of it (root {root:?})"
        );

        // A point well outside every fragment recovers nothing.
        assert!(
            hit_test_node(&dom_ref, SHEET, 800, 600, 10_000.0, 10_000.0).is_none(),
            "a point outside all fragments should miss"
        );
    }

    // --- MARK: Stage 2b — native click dispatch -------------------------------

    /// The first element in `node`'s subtree (pre-order) whose local name is
    /// `name`. Used to find a handler-bearing node by tag without depending on
    /// layout coordinates (point → node is covered by `hit_test_node`; this is
    /// the structural counterpart the dispatch test wants).
    fn find_element_by_name(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom
                .element_name(node)
                .is_some_and(|q| q.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|c| find_element_by_name(dom, c, name))
    }

    /// The concrete button-counter view type:
    /// `<div>{count}<button>+</button></div>`, the `<button>` carrying an
    /// `on_click` that increments the count. The handler is a non-capturing
    /// closure, so it coerces to a `fn` pointer and the view type is nameable
    /// (the existing tests' type-alias convention). Reuses the module's
    /// [`Counter`] state.
    type ButtonView = El<
        (String, OnClick<El<&'static str, Counter, ()>, Counter, (), fn(&mut Counter, PointerClick)>),
        Counter,
        (),
    >;

    fn button_counter_view(s: &Counter) -> ButtonView {
        let increment: fn(&mut Counter, PointerClick) = |s: &mut Counter, _ev| s.count += 1;
        el::<_, Counter, ()>(
            "div",
            (
                s.count.to_string(),
                on_click(el::<_, Counter, ()>("button", "+"), increment),
            ),
        )
    }

    /// Dispatching a click on the `<button>` fires its handler, bumping the
    /// count, and the rebuild reflects the new count into the DOM text.
    #[test]
    fn click_on_button_increments_and_rebuilds() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::new(dom.clone(), button_counter_view, Counter { count: 0 });
        let root = runner.root();

        // Find the <button> by a DOM walk (not layout coords — Stage 2a covers
        // point → node).
        let button = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, root, "button").expect("a <button> must exist")
        };

        assert_eq!(runner.state().count, 0);
        assert_eq!(text_child(&dom.borrow(), root).as_deref(), Some("0"));

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        // (a) state mutated through the faithful message route.
        assert_eq!(runner.state().count, 1, "handler should have incremented");
        // (b) rebuild reflected the change into the DOM text under the div.
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("1"),
            "rebuild after dispatch should update the DOM text"
        );
    }

    /// A parent `<div>` handler fires when the click is dispatched on the child
    /// `<button>`: the bubble walk (target → root) reaches the div's registered
    /// handler. The button itself has no handler here, so only the parent fires.
    type BubbleView = El<
        (String, El<&'static str, Counter, ()>),
        Counter,
        (),
    >;

    /// `on_click(<div>{count}<button>+</button></div>, parent_handler)` — the
    /// handler is on the *div*, the button is a plain leaf. Dispatching on the
    /// button must bubble up to the div.
    type BubbleRoot =
        OnClick<BubbleView, Counter, (), fn(&mut Counter, PointerClick)>;

    fn bubble_view(s: &Counter) -> BubbleRoot {
        let increment: fn(&mut Counter, PointerClick) = |s: &mut Counter, _ev| s.count += 1;
        on_click(
            el::<_, Counter, ()>(
                "div",
                (
                    s.count.to_string(),
                    el::<_, Counter, ()>("button", "+"),
                ),
            ),
            increment,
        )
    }

    #[test]
    fn click_bubbles_from_child_to_parent_handler() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::new(dom.clone(), bubble_view, Counter { count: 0 });
        let root = runner.root();

        let button = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, root, "button").expect("a <button> must exist")
        };
        // The button is genuinely a descendant of the handler-bearing div.
        assert_ne!(button, root, "button must be the child, not the root div");

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().count,
            1,
            "click on child <button> should bubble to the parent <div>'s handler"
        );
    }

    /// Dispatching on a node with no registered handler (and no handler-bearing
    /// ancestor) leaves the state untouched and runs no rebuild side effect.
    #[test]
    fn click_on_unhandled_node_is_a_noop() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::new(dom.clone(), button_counter_view, Counter { count: 0 });
        let root = runner.root();

        // The text node under the div carries no click handler, and its only
        // ancestors are the div and document, which also carry none.
        let text_node = {
            let dom_ref = dom.borrow();
            dom_ref
                .dom_children(root)
                .find(|&c| dom_ref.kind(c) == NodeKind::Text)
                .expect("the div has a text child")
        };

        runner.dispatch_click(text_node, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().count,
            0,
            "dispatching on an unhandled node must not change state"
        );
    }

    // --- MARK: Stage 3a — component composition -------------------------------

    /// All elements in `node`'s subtree (pre-order) whose local name is `name`.
    /// The plural counterpart of [`find_element_by_name`]: the `lens` test needs
    /// to find *both* counter buttons and address each one independently.
    fn collect_elements_by_name(dom: &ScriptedDom, node: NodeId, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        fn walk(dom: &ScriptedDom, node: NodeId, name: &str, out: &mut Vec<NodeId>) {
            if dom.kind(node) == NodeKind::Element
                && dom
                    .element_name(node)
                    .is_some_and(|q| q.local.as_ref() == name)
            {
                out.push(node);
            }
            for c in dom.dom_children(node) {
                walk(dom, c, name, out);
            }
        }
        walk(dom, node, name, &mut out);
        out
    }

    /// A reusable, independently-stateful component: a `<button>` whose text is
    /// the current count and whose click increments it. Its state is a bare
    /// `u32`, so it knows nothing about whatever larger app embeds it — that is
    /// exactly what makes it reusable.
    ///
    /// This is the canonical `xilem_core::lens` *component* shape
    /// (`Fn(&mut ChildState) -> impl View<ChildState, …>`, as in the upstream
    /// `lens(date_picker, |state| &mut state.date)` example), so it slots into
    /// `lens` directly. (The plan sketched a zero-arg `counter_button()`; the
    /// component must read the count to render its text, so it takes `&mut u32`,
    /// the real `lens` component signature.)
    fn counter_button(
        count: &mut u32,
    ) -> impl View<u32, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, u32, ()>("button", count.to_string()),
            |c: &mut u32, _ev| *c += 1,
        )
    }

    /// The composing app: two independent counters, each a `counter_button`
    /// lensed onto its own field of `App`. `lens` is a stock `xilem_core` view —
    /// it is generic over `Context: ViewPathTracker`, so it drives `ServalCtx`
    /// with no serval-side impl.
    struct App {
        left: u32,
        right: u32,
    }

    fn app_view(_s: &App) -> impl View<App, (), ServalCtx, Element = ServalElement> + use<> {
        el::<_, App, ()>(
            "div",
            (
                lens(counter_button, |s: &mut App| &mut s.left),
                lens(counter_button, |s: &mut App| &mut s.right),
            ),
        )
    }

    /// State composition via `lens`: two independently-stateful counter buttons,
    /// each lensed onto its own `App` field. Clicking the *left* button must
    /// increment only `App::left` (sub-state isolation), and the rebuild must
    /// reflect the new count in the *left* button's DOM text while the right
    /// button's text is untouched.
    #[test]
    fn lens_composes_two_independent_counters() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), app_view, App { left: 0, right: 0 });
        let root = runner.root();

        // Two <button>s, addressable by DOM order (left first, right second).
        let (left_button, right_button) = {
            let dom_ref = dom.borrow();
            let buttons = collect_elements_by_name(&dom_ref, root, "button");
            assert_eq!(buttons.len(), 2, "the app should render two buttons");
            (buttons[0], buttons[1])
        };

        // The button text reads its own count.
        let button_text = |dom: &ScriptedDom, b: NodeId| text_child(dom, b);
        {
            let dom_ref = dom.borrow();
            assert_eq!(button_text(&dom_ref, left_button).as_deref(), Some("0"));
            assert_eq!(button_text(&dom_ref, right_button).as_deref(), Some("0"));
        }
        assert_eq!((runner.state().left, runner.state().right), (0, 0));

        // Click ONLY the left button.
        runner.dispatch_click(left_button, PointerClick { local: (0.0, 0.0) });

        // (a) sub-state isolation: only `left` incremented.
        assert_eq!(
            (runner.state().left, runner.state().right),
            (1, 0),
            "clicking the left counter must touch only App::left"
        );

        // (b) the rebuild updated the left button's DOM text, not the right's.
        // Node identity is stable across the rebuild (the text node is mutated in
        // place), so the captured `NodeId`s still address the same buttons.
        {
            let dom_ref = dom.borrow();
            let buttons = collect_elements_by_name(&dom_ref, runner.root(), "button");
            assert_eq!(buttons.len(), 2);
            assert_eq!(
                button_text(&dom_ref, buttons[0]).as_deref(),
                Some("1"),
                "left button text should follow App::left"
            );
            assert_eq!(
                button_text(&dom_ref, buttons[1]).as_deref(),
                Some("0"),
                "right button text must be unchanged"
            );
        }

        // And the mirror: clicking the right button now moves only `right`.
        runner.dispatch_click(right_button, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            (runner.state().left, runner.state().right),
            (1, 1),
            "clicking the right counter must touch only App::right"
        );
    }

    // --- MARK: Stage 3a — action bubbling (OptionalAction + map_action) --------

    /// A child component with its *own* `Action`: a `<button>` whose click does
    /// not mutate any state directly but returns a [`Bump`] action. The action
    /// is meaningless on its own — a parent decides what it means, via
    /// [`map_action`]. `Bump` opts into [`xilem_serval::Action`] so
    /// `OptionalAction` treats it as a bubbling action rather than `()`.
    #[derive(Clone, Debug, PartialEq)]
    struct Bump;
    impl xilem_serval::Action for Bump {}

    /// The child view's state is a bare `()`: it owns no state, it only emits
    /// `Bump`. This is the action-first analogue of `counter_button`.
    fn bump_button() -> impl View<(), Bump, ServalCtx, Element = ServalElement> + use<> {
        on_click(el::<_, (), Bump>("button", "+"), |_s: &mut (), _ev| Bump)
    }

    /// The parent app: it owns the count and interprets the child's `Bump` as
    /// "increment me". `map_action(child, |state, action| …)` is the stock
    /// `xilem_core` view that turns the child's `Action = Bump` into a parent
    /// effect (mutating `Parent::count`) and a parent `Action = ()`. The child's
    /// `State` is `()`, so it is wrapped in a `lens` onto a throwaway field to
    /// bridge it under the parent state.
    struct Parent {
        count: u32,
        /// A unit sub-state the child component is lensed onto (the child's
        /// `State = ()`), so `map_action` can sit over it under `Parent`.
        unit: (),
    }

    fn parent_view(
        _s: &Parent,
    ) -> impl View<Parent, (), ServalCtx, Element = ServalElement> + use<> {
        // The child emits `Bump` over `State = ()`; lens it onto `Parent::unit`
        // so it composes under `Parent`, then `map_action` interprets the
        // bubbled `Bump` as a parent-side increment.
        let child = lens(|_unit: &mut ()| bump_button(), |p: &mut Parent| &mut p.unit);
        el::<_, Parent, ()>(
            "div",
            map_action(child, |p: &mut Parent, _action: Bump| {
                p.count += 1;
            }),
        )
    }

    /// Action bubbling: the child's `on_click` returns a `Bump` action (not
    /// unit), which `OptionalAction` turns into `MessageResult::Action(Bump)`;
    /// `map_action` intercepts it, applies the parent effect (`count += 1`), and
    /// re-labels the result as the parent's action type. Dispatching the click
    /// must run that mapped effect — the parent *observes and handles* the mapped
    /// child action.
    #[test]
    fn action_bubbles_and_maps_to_parent_effect() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            parent_view,
            Parent { count: 0, unit: () },
        );
        let root = runner.root();

        let button = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, root, "button").expect("a <button> must exist")
        };

        assert_eq!(runner.state().count, 0);

        // The child's `Bump` bubbles as `MessageResult::Action(Bump)`;
        // `map_action`'s map fn runs the *parent effect* (`count += 1`) and
        // re-labels the result as the parent's `Action` — here `()`. So the
        // primary observable is the parent state mutation. (`map_action` maps the
        // action type rather than swallowing it, so the relabelled `()` still
        // surfaces at the root; the runner collects it as one `()` entry — that
        // is expected, not a failure.)
        let bubbled = runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            runner.state().count,
            1,
            "the child Bump action must map to the parent's increment effect"
        );
        assert_eq!(
            bubbled,
            vec![()],
            "map_action relabels Bump -> parent () action, which reaches the root"
        );
    }

    /// The complementary half: an action that is *not* consumed by any
    /// `map_action` reaches the root, where the runner collects it from
    /// `dispatch_click`. This proves `MessageResult::Action` threads all the way
    /// through the runner's `Action` home, and that the runner is generic over a
    /// non-`()` root action.
    #[test]
    fn unmapped_action_reaches_the_runner() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        // The root view's Action IS `Bump`: the child's action bubbles straight
        // to the root with no `map_action` in between, so the runner's Action
        // type unifies to `Bump`.
        fn root_view(_s: &()) -> impl View<(), Bump, ServalCtx, Element = ServalElement> + use<> {
            el::<_, (), Bump>("div", bump_button())
        }

        let mut runner = ServalAppRunner::<_, _, _, Bump>::new(dom.clone(), root_view, ());
        let root = runner.root();

        let button = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, root, "button").expect("a <button> must exist")
        };

        let bubbled = runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            bubbled,
            vec![Bump],
            "an unmapped child action must surface from dispatch_click as a root Action"
        );
    }

    // --- MARK: Stage 3b — keyboard + focus ------------------------------------

    /// An editor app: a text buffer a key handler edits, rendered as
    /// `<div id="editor"><input/>{text}</div>` so the render path (cascade →
    /// layout → paint) covers it as it does the counter.
    struct Editor {
        text: String,
    }

    /// Apply a key to the buffer: append typed chars, Backspace deletes the last.
    fn edit(s: &mut Editor, ev: KeyEvent) {
        match ev.key {
            Key::Character(c) => s.text.push_str(&c),
            Key::Named(NamedKey::Backspace) => {
                s.text.pop();
            }
            Key::Named(_) => {}
        }
    }

    fn ch(s: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Character(s.to_string()),
        }
    }

    fn named(k: NamedKey) -> KeyEvent {
        KeyEvent { key: Key::Named(k) }
    }

    /// `<div id="editor"><input on_key=edit/>{text}</div>`: the focusable
    /// `<input>` carries the key handler; the div's text mirrors the buffer so a
    /// rebuild after each key is observable in the DOM. Concrete `fn`-pointer
    /// handler keeps the type nameable.
    type EditorView = El<
        (
            OnKey<El<&'static str, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>,
            String,
        ),
        Editor,
        (),
    >;

    fn editor_view(s: &Editor) -> EditorView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        el::<_, Editor, ()>(
            "div",
            (
                on_key(el::<_, Editor, ()>("input", ""), handler),
                s.text.clone(),
            ),
        )
        .attr("id", "editor")
    }

    /// Focus routing through the full host: focus the `<input>`, type "h"+"i"
    /// (buffer "hi"), Backspace (buffer "h"); each key's rebuild must reach the
    /// DOM text, and the render path (layout + paint) must still build over the
    /// live `ScriptedDom`.
    #[test]
    fn typed_keys_reach_focused_input_and_rebuild() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            editor_view,
            Editor {
                text: String::new(),
            },
        );
        let root = runner.root();

        let input = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, root, "input").expect("an <input> must exist")
        };

        runner.set_focus(Some(input));
        assert_eq!(runner.focus(), Some(input));

        runner.dispatch_key(ch("h"));
        runner.dispatch_key(ch("i"));
        assert_eq!(runner.state().text, "hi");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("hi"),
            "the rebuild after each key must reach the DOM text"
        );

        runner.dispatch_key(named(NamedKey::Backspace));
        assert_eq!(runner.state().text, "h", "Backspace deletes the last char");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("h")
        );

        // The render path still builds over the edited DOM.
        let scene = scene_from_scripted_dom(&dom.borrow(), SHEET, 800, 600);
        assert_eq!(scene.viewport_width, 800);
        assert!(
            fragments_from_scripted_dom(&dom.borrow(), SHEET, 800, 600)
                .rect_of(runner.root())
                .is_some(),
            "the editor div must still lay out after edits"
        );
    }

    /// No focus → `dispatch_key` is a no-op (empty return, state unchanged).
    #[test]
    fn no_focus_key_dispatch_is_a_noop() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            editor_view,
            Editor {
                text: String::new(),
            },
        );

        assert_eq!(runner.focus(), None);
        let out = runner.dispatch_key(ch("h"));
        assert!(out.is_empty(), "no focus → empty action vec");
        assert_eq!(runner.state().text, "", "no focus → state unchanged");
    }

    /// Click-to-focus through the host: a `<div on_key=edit>` with a `<label>`
    /// child. Hit-testing a point inside the div (Stage 2a) yields a node;
    /// dispatching a click there focuses the div; a subsequent key routes there;
    /// clicking the document (outside any focusable element) clears focus.
    type ClickFocusView =
        OnKey<El<El<&'static str, Editor, ()>, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>;

    fn click_focus_view(_s: &Editor) -> ClickFocusView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        on_key(
            el::<_, Editor, ()>("div", el::<_, Editor, ()>("label", "L")),
            handler,
        )
    }

    #[test]
    fn click_focuses_focusable_div_and_routes_keys() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_focus_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root();

        // Hit-test a point inside the laid-out div (the Stage 2a path), then
        // dispatch a click there — the hit lands on the div or a descendant, and
        // either way click-to-focus picks the nearest focusable ancestor: the div.
        let hit = {
            let dom_ref = dom.borrow();
            hit_test_node(&dom_ref, &["div { display: block; }"], 800, 600, 5.0, 5.0)
                .expect("a point inside the div should hit something")
        };
        runner.dispatch_click(hit, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            runner.focus(),
            Some(div),
            "clicking inside the focusable div focuses it"
        );

        runner.dispatch_key(ch("a"));
        assert_eq!(runner.state().text, "a", "key routes to the focused div");

        // Clicking the document (no focusable ancestor) clears focus.
        let doc = dom.borrow().document();
        runner.dispatch_click(doc, PointerClick { local: (0.0, 0.0) });
        assert_eq!(runner.focus(), None, "clicking outside clears focus");
    }

    /// Key bubbling through the host: a key handler on the parent div fires when
    /// the focused child (`<button>`, click-only, not focusable) has none.
    type BubbleKeyView = OnKey<
        El<
            OnClick<El<&'static str, Editor, ()>, Editor, (), fn(&mut Editor, PointerClick)>,
            Editor,
            (),
        >,
        Editor,
        (),
        fn(&mut Editor, KeyEvent),
    >;

    fn bubble_key_view(_s: &Editor) -> BubbleKeyView {
        let key_handler: fn(&mut Editor, KeyEvent) = edit;
        let click_noop: fn(&mut Editor, PointerClick) = |_s, _ev| {};
        on_key(
            el::<_, Editor, ()>(
                "div",
                on_click(el::<_, Editor, ()>("button", "+"), click_noop),
            ),
            key_handler,
        )
    }

    #[test]
    fn key_bubbles_from_focused_child_to_parent() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            bubble_key_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root();

        let button = {
            let dom_ref = dom.borrow();
            find_element_by_name(&dom_ref, div, "button").expect("a <button> must exist")
        };
        assert_ne!(button, div, "button is a descendant of the handler-bearing div");

        // The button is click-only (not focusable); aim focus at it to exercise
        // the bubble: a key on a child with no key handler reaches the parent.
        runner.set_focus(Some(button));
        runner.dispatch_key(ch("z"));
        assert_eq!(
            runner.state().text,
            "z",
            "key on the focused child bubbles to the parent div's handler"
        );
    }
}
