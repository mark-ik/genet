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
    use xilem_serval::{El, OnClick, PointerClick, ServalAppRunner, el, on_click};

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
}
