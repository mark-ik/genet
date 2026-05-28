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
