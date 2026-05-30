/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! End-to-end backend probe (Stage 1a).
//!
//! Drives the full `View`/`ViewSequence` path: builds a view tree into a
//! `ScriptedDom`, rebuilds it with structural and attribute changes, and
//! asserts both the resulting tree shape and the drained `DomMutation`s for:
//!   1. an initial tree,
//!   2. a middle insert,
//!   3. a middle delete,
//!   4. an attribute change and removal.

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut, Namespace, NodeKind};
use serval_scripted_dom::{NodeId, ScriptedDom};
use xilem_core::{MessageCtx, MessageResult, View};

use crate::{DomHandle, El, ServalCtx, ServalElement, el};

// --- MARK: read helpers -------------------------------------------------------

/// Element-child local names of `node`, in DOM order (skips text nodes).
fn element_child_names(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
    dom.dom_children(node)
        .filter(|&c| dom.kind(c) == NodeKind::Element)
        .map(|c| dom.element_name(c).unwrap().local.to_string())
        .collect()
}

/// All child node kinds + payloads (element local name or text data), in order.
fn child_summary(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
    dom.dom_children(node)
        .map(|c| match dom.kind(c) {
            NodeKind::Element => format!("<{}>", dom.element_name(c).unwrap().local),
            NodeKind::Text => format!("#text:{}", dom.text(c).unwrap_or("")),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Read an attribute in the null namespace.
fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
    dom.attribute(node, &Namespace::from(""), &name.into())
}

fn count_inserted(muts: &[DomMutation<NodeId>]) -> usize {
    muts.iter()
        .filter(|m| matches!(m, DomMutation::Inserted { .. }))
        .count()
}

fn count_removed(muts: &[DomMutation<NodeId>]) -> usize {
    muts.iter()
        .filter(|m| matches!(m, DomMutation::Removed { .. }))
        .count()
}

fn drain(dom: &DomHandle) -> Vec<DomMutation<NodeId>> {
    let mut out = Vec::new();
    dom.borrow_mut().drain_mutations(&mut out);
    out
}

// --- MARK: the view under test ------------------------------------------------

/// `(<a>, optional <b>, <c>)` children under a root `<div>`. The middle `<b>`
/// is present only when `with_middle` is true — toggling it exercises a true
/// middle insert/delete (reference node = `<c>`), not a tail append.
type TestView = El<
    (
        El<&'static str, (), ()>,
        Option<El<&'static str, (), ()>>,
        El<&'static str, (), ()>,
    ),
    (),
    (),
>;

fn app_logic(with_middle: bool, attr_on: bool) -> TestView {
    let middle = with_middle.then(|| el::<_, (), ()>("b", "B"));
    let view = el::<_, (), ()>(
        "div",
        (
            el::<_, (), ()>("a", "A"),
            middle,
            el::<_, (), ()>("c", "C"),
        ),
    );
    if attr_on {
        view.attr("id", "root").attr("class", "panel")
    } else {
        view
    }
}

/// Build the view, attach its root element under the document root, and return
/// the live state for subsequent rebuilds.
struct Harness {
    dom: DomHandle,
    ctx: ServalCtx,
    root_el: ServalElement,
    view: TestView,
    view_state: <TestView as View<(), (), ServalCtx>>::ViewState,
}

impl Harness {
    fn build(with_middle: bool, attr_on: bool) -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut ctx = ServalCtx::new(dom.clone());
        let view = app_logic(with_middle, attr_on);
        let (root_el, view_state) = view.build(&mut ctx, &mut ());
        // Attach the produced root under the document root.
        let doc_root = dom.borrow().document();
        dom.borrow_mut()
            .insert_before(doc_root, root_el.node, None);
        Self {
            dom,
            ctx,
            root_el,
            view,
            view_state,
        }
    }

    fn root(&self) -> NodeId {
        self.root_el.node
    }

    fn rebuild(&mut self, with_middle: bool, attr_on: bool) {
        let next = app_logic(with_middle, attr_on);
        let mut node = self.root_el.node;
        let mut_ref = crate::ServalElementMut {
            node: &mut node,
            dom: self.dom.clone(),
        };
        next.rebuild(
            &self.view,
            &mut self.view_state,
            &mut self.ctx,
            mut_ref,
            &mut (),
        );
        self.root_el.node = node;
        self.view = next;
    }
}

// --- MARK: 1. initial tree ----------------------------------------------------

#[test]
fn initial_tree_lands_in_dom() {
    let h = Harness::build(false, false);
    let dom = h.dom.borrow();
    let root = h.root();

    assert_eq!(dom.kind(root), NodeKind::Element);
    assert_eq!(dom.element_name(root).unwrap().local.to_string(), "div");
    // <a>, <c> with the middle absent.
    assert_eq!(element_child_names(&dom, root), vec!["a", "c"]);

    // Each leaf holds its text node.
    let a = dom.dom_children(root).next().unwrap();
    assert_eq!(child_summary(&dom, a), vec!["#text:A".to_string()]);
}

#[test]
fn initial_tree_records_inserts() {
    let h = Harness::build(false, false);
    let muts = drain(&h.dom);
    // Detached creations record nothing; each *attach* records one Inserted:
    //   #text:A -> <a>, #text:C -> <c>, <a> -> <div>, <c> -> <div>,
    //   <div> -> document = 5 inserts. (The root <div> create is detached.)
    assert_eq!(count_inserted(&muts), 5, "muts: {muts:?}");
    assert_eq!(count_removed(&muts), 0);
}

// --- MARK: 2. middle insert ---------------------------------------------------

#[test]
fn middle_insert_orders_and_records() {
    let mut h = Harness::build(false, false);
    let _ = drain(&h.dom); // clear the build mutations

    h.rebuild(true, false);

    let dom = h.dom.borrow();
    let root = h.root();
    assert_eq!(
        element_child_names(&dom, root),
        vec!["a", "b", "c"],
        "middle <b> must land between <a> and <c>"
    );
    drop(dom);

    let muts = drain(&h.dom);
    // The inserted <b> plus its text child = 2 inserts, no removals.
    assert_eq!(count_inserted(&muts), 2, "muts: {muts:?}");
    assert_eq!(count_removed(&muts), 0, "muts: {muts:?}");
    // The <b> insert is recorded under the root <div>.
    let root = h.root();
    assert!(
        muts.iter().any(|m| matches!(
            m,
            DomMutation::Inserted { parent, .. } if *parent == root
        )),
        "expected an Inserted under root: {muts:?}"
    );
}

// --- MARK: 3. middle delete ---------------------------------------------------

#[test]
fn middle_delete_orders_and_records() {
    let mut h = Harness::build(true, false); // start with <b> present
    assert_eq!(
        element_child_names(&h.dom.borrow(), h.root()),
        vec!["a", "b", "c"]
    );
    let _ = drain(&h.dom);

    h.rebuild(false, false); // drop the middle

    let dom = h.dom.borrow();
    let root = h.root();
    assert_eq!(element_child_names(&dom, root), vec!["a", "c"]);
    drop(dom);

    let muts = drain(&h.dom);
    // Removing <b> records exactly one Removed (the subtree drops with it).
    assert_eq!(count_removed(&muts), 1, "muts: {muts:?}");
    assert_eq!(count_inserted(&muts), 0, "muts: {muts:?}");
    let root = h.root();
    assert!(
        muts.iter().any(|m| matches!(
            m,
            DomMutation::Removed { former_parent, .. } if *former_parent == root
        )),
        "expected a Removed from root: {muts:?}"
    );
}

// --- MARK: 4. attribute change + removal --------------------------------------

#[test]
fn attribute_set_then_removed() {
    // Build with attributes off, then turn them on (set), then off again (remove).
    let mut h = Harness::build(false, false);
    let _ = drain(&h.dom);

    // Set: id="root", class="panel".
    h.rebuild(false, true);
    {
        let dom = h.dom.borrow();
        let root = h.root();
        assert_eq!(attr(&dom, root, "id"), Some("root"));
        assert_eq!(attr(&dom, root, "class"), Some("panel"));
    }
    let set_muts = drain(&h.dom);
    let set_attr_changes: Vec<_> = set_muts
        .iter()
        .filter(|m| matches!(m, DomMutation::AttributeChanged { .. }))
        .collect();
    assert_eq!(
        set_attr_changes.len(),
        2,
        "two attributes newly added: {set_muts:?}"
    );
    // Newly added -> old_value None.
    assert!(
        set_attr_changes.iter().all(|m| matches!(
            m,
            DomMutation::AttributeChanged { old_value: None, .. }
        )),
        "set: {set_attr_changes:?}"
    );

    // Remove: both attributes gone.
    h.rebuild(false, false);
    {
        let dom = h.dom.borrow();
        let root = h.root();
        assert_eq!(attr(&dom, root, "id"), None);
        assert_eq!(attr(&dom, root, "class"), None);
    }
    let rm_muts = drain(&h.dom);
    let rm_attr_changes: Vec<_> = rm_muts
        .iter()
        .filter(|m| matches!(m, DomMutation::AttributeChanged { .. }))
        .collect();
    assert_eq!(
        rm_attr_changes.len(),
        2,
        "two attributes removed: {rm_muts:?}"
    );
    // Removal records the prior value as old_value.
    assert!(
        rm_attr_changes.iter().all(|m| matches!(
            m,
            DomMutation::AttributeChanged { old_value: Some(_), .. }
        )),
        "remove: {rm_attr_changes:?}"
    );
}

// --- MARK: message routing smoke ----------------------------------------------

#[test]
fn message_to_unknown_path_is_handled() {
    // No event wiring in Stage 1a, but the message plumbing must compile and
    // route. A bare text view returns Stale for any message.
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let mut ctx = ServalCtx::new(dom.clone());
    let view = "hello";
    let (mut element, mut state) = View::<(), (), ServalCtx>::build(&view, &mut ctx, &mut ());
    let mut node = element.node;
    let env = xilem_core::Environment::new();
    let mut msg = MessageCtx::new(env, Vec::new(), xilem_core::DynMessage::new(()));
    let mut_ref = crate::ServalElementMut {
        node: &mut node,
        dom: dom.clone(),
    };
    let result: MessageResult<()> =
        View::<(), (), ServalCtx>::message(&view, &mut state, &mut msg, mut_ref, &mut ());
    assert!(matches!(result, MessageResult::Stale));
    element.node = node;
}

// --- MARK: Stage 3a — component composition (backend-only) --------------------
//
// These prove the `xilem_core` composition vocabulary works over `ServalCtx`
// using only this crate + the `ScriptedDom` — no serval-layout/netrender. The
// `pelt-live` suite asserts the same with full render-path coverage; these are
// the boundary-level twin, so a `lens`/`map_action`/`OptionalAction` regression
// is caught even with the engine stack absent.

#[cfg(test)]
mod composition {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, PointerClick, ServalAppRunner, ServalCtx, ServalElement, View, el, lens,
        map_action, on_click,
    };

    /// The text of the single text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// Every element in `node`'s subtree (pre-order) named `name`.
    fn elements_named(dom: &ScriptedDom, node: NodeId, name: &str) -> Vec<NodeId> {
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

    // A reusable counter component over a bare `u32` sub-state; the canonical
    // `lens` component shape. `+ use<>` keeps the opaque type from capturing the
    // input lifetime (else it can't be a single `V` for `FnMut(&_) -> V`).
    fn counter_button(
        count: &mut u32,
    ) -> impl View<u32, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, u32, ()>("button", count.to_string()),
            |c: &mut u32, _ev| *c += 1,
        )
    }

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

    /// `lens` composes two independently-stateful counters: clicking the left
    /// button moves only `App::left`, and the rebuild updates only its text.
    #[test]
    fn lens_isolates_substate() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), app_view, App { left: 0, right: 0 });
        let root = runner.root();

        let (left, right) = {
            let d = dom.borrow();
            let bs = elements_named(&d, root, "button");
            assert_eq!(bs.len(), 2, "two counter buttons");
            (bs[0], bs[1])
        };

        runner.dispatch_click(left, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            (runner.state().left, runner.state().right),
            (1, 0),
            "only the lensed left sub-state changes"
        );
        {
            let d = dom.borrow();
            assert_eq!(text_child(&d, left).as_deref(), Some("1"));
            assert_eq!(text_child(&d, right).as_deref(), Some("0"));
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Bump;
    impl crate::Action for Bump {}

    fn bump_button() -> impl View<(), Bump, ServalCtx, Element = ServalElement> + use<> {
        on_click(el::<_, (), Bump>("button", "+"), |_s: &mut (), _ev| Bump)
    }

    struct Parent {
        count: u32,
        unit: (),
    }

    fn parent_view(
        _s: &Parent,
    ) -> impl View<Parent, (), ServalCtx, Element = ServalElement> + use<> {
        let child = lens(|_u: &mut ()| bump_button(), |p: &mut Parent| &mut p.unit);
        el::<_, Parent, ()>(
            "div",
            map_action(child, |p: &mut Parent, _a: Bump| {
                p.count += 1;
            }),
        )
    }

    /// `OptionalAction` + `map_action`: the child returns a `Bump` action, which
    /// `map_action` turns into the parent effect `count += 1`.
    #[test]
    fn map_action_applies_parent_effect() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            parent_view,
            Parent { count: 0, unit: () },
        );
        let root = runner.root();
        let button = {
            let d = dom.borrow();
            elements_named(&d, root, "button")[0]
        };

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            runner.state().count,
            1,
            "child Bump action maps to the parent increment"
        );
    }
}

// --- MARK: Stage 3b — keyboard + focus (backend-only) -------------------------
//
// The headless twin of `pelt-live`'s Stage 3b suite: focus routing, the
// no-focus no-op, click-to-focus, and key bubbling — proven over the
// `ScriptedDom` with no serval-layout/netrender, so a key-registry/focus
// regression is caught even with the engine stack absent.

#[cfg(test)]
mod keyboard {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, El, Key, KeyEvent, NamedKey, OnClick, OnKey, PointerClick, ServalAppRunner, el,
        on_click, on_key,
    };

    /// The app state: a text buffer a key handler edits.
    struct Editor {
        text: String,
    }

    /// Append the typed character / apply the editing key to `text`. A free
    /// function (not a closure) so the view type stays a nameable `fn` pointer.
    fn edit(s: &mut Editor, ev: KeyEvent) {
        match ev.key {
            Key::Character(c) => s.text.push_str(&c),
            Key::Named(NamedKey::Backspace) => {
                s.text.pop();
            }
            Key::Named(_) => {}
        }
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
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

    fn ch(s: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Character(s.to_string()),
        }
    }

    fn named(k: NamedKey) -> KeyEvent {
        KeyEvent { key: Key::Named(k) }
    }

    // --- focus routing --------------------------------------------------------

    /// `<div><input on_key=edit/><span/></div>`: a focusable `<input>` (carries a
    /// key handler) beside a non-focusable `<span>`. Concrete `fn`-pointer
    /// handler so the type is nameable.
    type FocusView = El<
        (
            OnKey<El<&'static str, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>,
            El<&'static str, Editor, ()>,
        ),
        Editor,
        (),
    >;

    fn focus_view(_s: &Editor) -> FocusView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        el::<_, Editor, ()>(
            "div",
            (
                on_key(el::<_, Editor, ()>("input", ""), handler),
                el::<_, Editor, ()>("span", "x"),
            ),
        )
    }

    /// Focus the `<input>`, type "h" then "i" (state == "hi"), then Backspace
    /// (state == "h"). The key handler routes to the focused element.
    #[test]
    fn typed_keys_reach_focused_element() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            focus_view,
            Editor {
                text: String::new(),
            },
        );
        let root = runner.root();

        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("an <input> must exist")
        };

        runner.set_focus(Some(input));
        assert_eq!(runner.focus(), Some(input));

        runner.dispatch_key(ch("h"));
        runner.dispatch_key(ch("i"));
        assert_eq!(runner.state().text, "hi", "typed chars append to the buffer");

        runner.dispatch_key(named(NamedKey::Backspace));
        assert_eq!(runner.state().text, "h", "Backspace deletes the last char");
    }

    /// With no focus (never focused), `dispatch_key` is a no-op: it returns empty
    /// and leaves state unchanged. Also covered: explicitly clearing focus.
    #[test]
    fn no_focus_is_a_noop() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            focus_view,
            Editor {
                text: String::new(),
            },
        );

        assert_eq!(runner.focus(), None, "nothing focused initially");
        let out = runner.dispatch_key(ch("h"));
        assert!(out.is_empty(), "no focus → empty action vec");
        assert_eq!(runner.state().text, "", "no focus → state unchanged");

        // Explicitly clearing focus is the same no-op.
        runner.set_focus(None);
        let out = runner.dispatch_key(ch("z"));
        assert!(out.is_empty());
        assert_eq!(runner.state().text, "");
    }

    // --- click sets focus -----------------------------------------------------

    /// `<div on_key=edit><label/></div>`: the focusable `<div>` carries the key
    /// handler and contains a non-focusable `<label>` child. The div also has no
    /// click handler — focus does not depend on `on_click`.
    type ClickFocusView =
        OnKey<El<El<&'static str, Editor, ()>, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>;

    fn click_focus_view(_s: &Editor) -> ClickFocusView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        on_key(
            el::<_, Editor, ()>("div", el::<_, Editor, ()>("label", "L")),
            handler,
        )
    }

    /// Clicking the focusable div (via its child) focuses the div; a subsequent
    /// `dispatch_key` routes there. Then clicking a non-focusable node clears
    /// focus to `None`.
    #[test]
    fn click_sets_and_clears_focus() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_focus_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root(); // the OnKey wraps the root <div>

        let label = {
            let d = dom.borrow();
            find_element_by_name(&d, div, "label").expect("a <label>")
        };

        assert_eq!(runner.focus(), None);

        // Click the label (a child of the focusable div) → focus the div, since
        // the nearest focusable ancestor of the label is the div.
        runner.dispatch_click(label, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            runner.focus(),
            Some(div),
            "clicking inside the focusable div focuses the div"
        );

        // A key now reaches the div's handler.
        runner.dispatch_key(ch("a"));
        assert_eq!(runner.state().text, "a", "key routes to the focused div");

        // Clicking a node whose ancestor chain has no key handler clears focus.
        // The document root is such a node (above the div, no handler).
        let doc = dom.borrow().document();
        runner.dispatch_click(doc, PointerClick { local: (0.0, 0.0) });
        assert_eq!(
            runner.focus(),
            None,
            "clicking outside any focusable element clears focus"
        );
    }

    // --- key bubbling ---------------------------------------------------------

    /// A key handler on a *parent* fires when the focused child has none.
    /// `<div on_key=edit><button on_click=noop/></div>`: focus the button (no key
    /// handler) and dispatch — the key bubbles to the div's handler. The button
    /// carries only a click handler, so it is *not* focusable; we `set_focus`
    /// it directly to model "a non-focusable node happened to be focused" and
    /// confirm the bubble still reaches the parent.
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
    fn key_bubbles_from_focused_child_to_parent_handler() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            bubble_key_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, div, "button").expect("a <button>")
        };
        assert_ne!(button, div, "button is a descendant of the handler-bearing div");

        // The button has only a click handler, so it is not focusable; aim focus
        // at it directly to exercise the bubble (focus on a child with no key
        // handler → the parent's handler fires).
        runner.set_focus(Some(button));
        runner.dispatch_key(ch("z"));
        assert_eq!(
            runner.state().text,
            "z",
            "key on the focused child bubbles to the parent div's handler"
        );
    }
}

// --- MARK: Stage 3 — form controls (text_field, backend-only) -----------------
//
// The headless twin of `pelt-live`'s Stage 3 form-control coverage: a reusable
// `text_field` whose state is its own `TextInput` (buffer + caret), edited through the focus + key
// dispatch foundation, and composed under a larger struct via `lens` — proven
// over the `ScriptedDom` with no serval-layout/netrender, so a regression in the
// field's edit handler or its `lens` composition is caught with the engine stack
// absent. (NB: per Stage 3b, space arrives as `Named(Space)`, not
// `Character(" ")`, so the sequence below exercises that path explicitly.)

#[cfg(test)]
mod controls {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, Key, KeyEvent, NamedKey, ServalAppRunner, ServalCtx, ServalElement, TextInput,
        View, el, lens, text_field,
    };

    /// The text data of the single text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
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

    fn ch(s: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Character(s.to_string()),
        }
    }

    fn named(k: NamedKey) -> KeyEvent {
        KeyEvent { key: Key::Named(k) }
    }

    /// Type the canonical sequence into a focused field; the buffer progresses:
    ///   "h", "i", Space, "y", "o", Backspace
    ///     → "h", "hi", "hi ", "hi y", "hi yo", "hi y"
    /// Space goes through `Named(Space)` (the Stage 3b convention); the caret
    /// stays at the end throughout, so each char inserts there and Backspace
    /// removes the trailing "o". Callers assert the resulting buffer / DOM text.
    fn type_hi_y(runner: &mut impl FieldRunner) {
        runner.key(ch("h"));
        runner.key(ch("i"));
        runner.key(named(NamedKey::Space));
        runner.key(ch("y"));
        runner.key(ch("o"));
        runner.key(named(NamedKey::Backspace));
    }

    /// Minimal shim so `type_hi_y` drives either runner shape (the bare-`String`
    /// runner and the `lens`-composed struct runner) through one `dispatch_key`.
    trait FieldRunner {
        fn key(&mut self, ev: KeyEvent);
    }

    // --- bare String state ----------------------------------------------------

    impl<Logic, V> FieldRunner for ServalAppRunner<TextInput, Logic, V, ()>
    where
        Logic: FnMut(&TextInput) -> V,
        V: View<TextInput, (), ServalCtx, Element = ServalElement>,
    {
        fn key(&mut self, ev: KeyEvent) {
            self.dispatch_key(ev);
        }
    }

    /// A `text_field` over bare `TextInput` state: focus it, type the sequence,
    /// and assert the buffer reads `"hi y"` (caret at the end) and the field's
    /// `<input>` text shows the rendered buffer with the caret marker (`"hi y|"`).
    #[test]
    fn text_field_edits_its_own_buffer() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            // The field's state IS the whole app state here: a `TextInput`.
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let root = runner.root();

        // The field renders an <input> (the focusable element).
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("the field renders an <input>")
        };

        runner.set_focus(Some(input));
        type_hi_y(&mut runner);

        assert_eq!(runner.state().text(), "hi y", "the field edited its buffer");
        assert_eq!(runner.state().caret(), 4, "caret sits at the end after typing");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("hi y|"),
            "the <input> DOM text shows the buffer with the caret marker at the end"
        );
    }

    /// Caret movement + insert/delete at the caret: type, move left, insert in the
    /// middle, then delete on both sides. Proves the field is an insertion-point
    /// editor (not append-only), and that the rendered caret marker tracks the
    /// caret position.
    #[test]
    fn text_field_caret_moves_and_edits() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "input").expect("an <input>")
        };
        runner.set_focus(Some(input));

        runner.key(ch("h"));
        runner.key(ch("i")); // "hi", caret 2
        runner.key(named(NamedKey::ArrowLeft)); // caret 1: "h|i"
        runner.key(ch("X")); // insert at 1: "hXi", caret 2
        assert_eq!(runner.state().text(), "hXi");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("hX|i"),
            "marker after the inserted X"
        );

        runner.key(named(NamedKey::ArrowLeft));
        runner.key(named(NamedKey::ArrowLeft)); // caret 0: "|hXi"
        runner.key(named(NamedKey::Delete)); // delete after caret (h): "Xi", caret 0
        assert_eq!(runner.state().text(), "Xi");
        assert_eq!(runner.state().caret(), 0);

        runner.key(named(NamedKey::ArrowRight)); // caret 1
        runner.key(named(NamedKey::Backspace)); // delete before caret (X): "i", caret 0
        assert_eq!(runner.state().text(), "i");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("|i"),
            "caret at the start"
        );
    }

    /// `TextInput`'s edits are character-correct across a multi-byte char (`é` is
    /// two UTF-8 bytes), exercised directly on the model (no runner): a byte-index
    /// bug would split `é` and panic or corrupt the buffer.
    #[test]
    fn text_input_edits_are_char_correct() {
        let mut t = TextInput::new("aé"); // 2 chars, caret at 2
        assert_eq!(t.caret(), 2);
        t.move_left(); // caret 1 (between 'a' and 'é')
        t.insert_str("X"); // "aXé", caret 2
        assert_eq!(t.text(), "aXé");
        assert_eq!(t.caret(), 2);
        t.backspace(); // remove 'X' before caret: "aé", caret 1
        assert_eq!(t.text(), "aé");
        t.delete(); // remove 'é' after caret: "a", caret 1
        assert_eq!(t.text(), "a");
        assert_eq!(t.caret(), 1);
        assert_eq!(t.display(), "a|");
    }

    // --- lens-composed struct field -------------------------------------------

    /// A larger app with an independently-edited text field plus a sibling field
    /// the edits must *not* touch — proving the `text_field` composes via `lens`.
    struct Form {
        name: TextInput,
        other: String,
    }

    impl<Logic, V> FieldRunner for ServalAppRunner<Form, Logic, V, ()>
    where
        Logic: FnMut(&Form) -> V,
        V: View<Form, (), ServalCtx, Element = ServalElement>,
    {
        fn key(&mut self, ev: KeyEvent) {
            self.dispatch_key(ev);
        }
    }

    /// `lens(text_field, |f| &mut f.name)` under a `<div>`: the field edits only
    /// `Form::name`, leaving `Form::other` untouched. The same key sequence as
    /// the bare-`String` test, proving the field is a drop-in `lens` component.
    #[test]
    fn text_field_composes_under_lens() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_f: &Form| {
                // `text_field` takes `&TextInput` (the field renders from a
                // shared read), so a thin `|s: &mut TextInput| text_field(s)`
                // adapter bridges it to the `Fn(&mut ChildState) -> View` shape
                // `lens` wants — the same adapter the Stage 3a `bump_button` test
                // uses.
                el::<_, Form, ()>(
                    "div",
                    lens(|s: &mut TextInput| text_field(s), |f: &mut Form| &mut f.name),
                )
            },
            Form {
                name: TextInput::default(),
                other: "untouched".to_string(),
            },
        );
        let root = runner.root();

        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("the field renders an <input>")
        };

        runner.set_focus(Some(input));
        type_hi_y(&mut runner);

        assert_eq!(
            runner.state().name.text(),
            "hi y",
            "only the lensed field changed"
        );
        assert_eq!(
            runner.state().other, "untouched",
            "the sibling field must be untouched"
        );
        assert_eq!(
            text_child(&dom.borrow(), input).as_deref(),
            Some("hi y|"),
            "the rebuild reflected the edits (with caret marker) into the lensed field's <input> text"
        );
    }

    /// Home / End jump the caret to the line ends (the new named keys), driven
    /// through the full key-dispatch + edit path.
    #[test]
    fn text_field_home_end_move_caret() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "input").expect("an <input>")
        };
        runner.set_focus(Some(input));

        runner.key(ch("a"));
        runner.key(ch("b"));
        runner.key(ch("c")); // "abc", caret 3
        runner.key(named(NamedKey::Home));
        assert_eq!(runner.state().caret(), 0, "Home jumps to the start");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("|abc"),
            "marker at the start after Home"
        );

        runner.key(named(NamedKey::End));
        assert_eq!(runner.state().caret(), 3, "End jumps to the end");
        assert_eq!(
            text_child(&dom.borrow(), runner.root()).as_deref(),
            Some("abc|"),
            "marker at the end after End"
        );
    }
}

// --- MARK: Stage 3 — capture phase (backend-only) -----------------------------
//
// The headless twin of `pelt-live`'s capture-phase suite: per-listener phase
// (`.capture(true)` vs the default bubble), and the dispatch order it produces
// (capture → target → bubble). Each handler appends a label to a shared
// `Vec<String>` log on the app state, so the assertions read the literal firing
// order — proven over the `ScriptedDom` with no serval-layout/netrender.

#[cfg(test)]
mod capture {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, Key, KeyEvent, PointerClick, ServalAppRunner, ServalCtx, ServalElement, View,
        el, on_click, on_key,
    };

    /// The app state: an ordered firing log every handler appends to.
    struct Log {
        events: Vec<String>,
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
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

    fn ch(s: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Character(s.to_string()),
        }
    }

    // --- click ordering: capture parent before bubble child -------------------

    /// `<div on_click(capture) ><button on_click /></div>`: a capture-phase
    /// handler on the parent div, a default (bubble) handler on the child button.
    /// Each logs its label. Clicking the button must fire the capture parent
    /// *before* the bubble child.
    fn click_phase_view(_s: &Log) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>(
                "div",
                on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
                    s.events.push("bubble-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-parent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn click_capture_parent_fires_before_bubble_child() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_phase_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().events,
            vec!["capture-parent".to_string(), "bubble-child".to_string()],
            "capture ancestor fires before the bubble target (capture → target → bubble)"
        );
    }

    // --- key ordering: same, via dispatch_key ---------------------------------

    /// `<div on_key(capture)><span on_key /></div>`: a capture-phase key handler
    /// on the div, a default (bubble) key handler on the span. Focus the span and
    /// dispatch a key — the capture div must fire before the bubble span.
    fn key_phase_view(_s: &Log) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_key(
            el::<_, Log, ()>(
                "div",
                on_key(el::<_, Log, ()>("span", "x"), |s: &mut Log, _ev| {
                    s.events.push("bubble-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-parent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn key_capture_parent_fires_before_bubble_child() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), key_phase_view, Log { events: Vec::new() });
        let root = runner.root();

        let span = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "span").expect("a <span>")
        };

        // The span carries a (bubble) key handler, so it is focusable.
        runner.set_focus(Some(span));
        runner.dispatch_key(ch("a"));

        assert_eq!(
            runner.state().events,
            vec!["capture-parent".to_string(), "bubble-child".to_string()],
            "capture key ancestor fires before the bubble focused node"
        );
    }

    // --- default is bubble ----------------------------------------------------

    /// A default `on_click` (no `.capture()`) on the parent, a `.capture(true)`
    /// handler on... the same parent? No — to show the default sits in the bubble
    /// pass *after* a capture ancestor, the capture handler is on the (outer)
    /// grandparent and the default on the (inner) child. Click the child: the
    /// capture grandparent fires first, the default child last, proving the
    /// no-`.capture()` listener only runs in the bubble pass.
    fn default_is_bubble_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>(
                "section",
                on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
                    // No `.capture()` → default bubble. Must fire AFTER the
                    // capture grandparent, never before it.
                    s.events.push("default-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-grandparent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn default_on_click_only_fires_in_bubble_pass() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            default_is_bubble_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().events,
            vec![
                "capture-grandparent".to_string(),
                "default-child".to_string()
            ],
            "a default (no `.capture()`) listener fires only in the bubble pass, \
             after the capture ancestor"
        );
    }

    // --- capture-only ancestor fires on a descendant click --------------------

    /// `<div on_click(capture)><button /></div>`: only the div carries a handler,
    /// and it is capture-phase. The button has no handler. Clicking the button (a
    /// descendant) must still fire the capture ancestor.
    fn capture_only_ancestor_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>("div", el::<_, Log, ()>("button", "+")),
            |s: &mut Log, _ev| s.events.push("capture-ancestor".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn capture_only_ancestor_fires_on_descendant_click() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            capture_only_ancestor_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };
        assert_ne!(button, root, "button is a descendant of the capture div");

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().events,
            vec!["capture-ancestor".to_string()],
            "a capture-only ancestor fires when a handler-less descendant is clicked"
        );
    }

    // --- a node's listener fires in exactly one phase -------------------------

    /// A single capture handler on a node that is itself the click target fires
    /// exactly once (in the capture pass), never twice — the target-node listener
    /// is in whichever single phase it registered.
    fn single_capture_target_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
            s.events.push("fired".to_string());
        })
        .capture(true)
    }

    #[test]
    fn target_listener_fires_in_exactly_one_phase() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            single_capture_target_view,
            Log { events: Vec::new() },
        );
        let button = runner.root();

        runner.dispatch_click(button, PointerClick { local: (0.0, 0.0) });

        assert_eq!(
            runner.state().events,
            vec!["fired".to_string()],
            "a capture listener on the target itself fires once, not in both passes"
        );
    }
}
