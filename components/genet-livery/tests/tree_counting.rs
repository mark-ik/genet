//! Harvest H5: `sibling-index()` and `sibling-count()` resolved from the
//! element's position in the retained tree, and reinvalidated when the child
//! list changes underneath them.

use genet_livery::{Device, IncrementalStyle, InteractionStates, StyleSet, resolve_styles};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind};
use livery::PropertyId;

fn by_id(dom: &ScriptedDom, expected: &str) -> NodeId {
    fn find(dom: &ScriptedDom, node: NodeId, expected: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(expected)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|child| find(dom, child, expected))
    }
    find(dom, dom.document(), expected).expect("fixture id")
}

fn z_index(plane: &genet_livery::StylePlane<NodeId>, node: NodeId) -> String {
    plane
        .get(node)
        .expect("styled element")
        .get(PropertyId::ZIndex)
        .to_css_string()
}

#[test]
fn tree_counting_functions_resolve_from_the_element_position() {
    let dom = ScriptedDom::from_serialized_document(
        "<html><body><div id='group'>\
         <span id='first'></span>\
         <span id='second'></span>\
         <span id='third'></span>\
         </div></body></html>",
    );
    let styles = StyleSet::cambium(&[
        "span { z-index: calc(sibling-index()); } #third { z-index: calc(sibling-count() * 10); }",
    ]);
    let plane = resolve_styles(
        &dom,
        &styles,
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );

    assert_eq!(z_index(&plane, by_id(&dom, "first")), "1");
    assert_eq!(z_index(&plane, by_id(&dom, "second")), "2");
    assert_eq!(z_index(&plane, by_id(&dom, "third")), "30");
}

#[test]
fn non_element_siblings_do_not_shift_the_ordinals() {
    let dom = ScriptedDom::from_serialized_document(
        "<html><body><div id='group'>\
         text<span id='first'></span>\
         <!-- comment -->more text<span id='second'></span>\
         </div></body></html>",
    );
    let styles = StyleSet::cambium(&["span { z-index: calc(sibling-index()); }"]);
    let plane = resolve_styles(
        &dom,
        &styles,
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );

    assert_eq!(z_index(&plane, by_id(&dom, "first")), "1");
    assert_eq!(z_index(&plane, by_id(&dom, "second")), "2");
}

#[test]
fn the_root_element_counts_itself_as_the_only_sibling() {
    let dom = ScriptedDom::from_serialized_document("<html><body></body></html>");
    let styles = StyleSet::cambium(&["html { z-index: calc(sibling-index() + sibling-count()); }"]);
    let plane = resolve_styles(
        &dom,
        &styles,
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );

    let root = dom
        .dom_children(dom.document())
        .find(|child| dom.kind(*child) == NodeKind::Element)
        .expect("root element");
    assert_eq!(z_index(&plane, root), "2");
}

#[test]
fn removing_a_sibling_recounts_the_group_incrementally() {
    let mut dom = ScriptedDom::from_serialized_document(
        "<html><body><div id='group'>\
         <span id='first'></span>\
         <span id='doomed'></span>\
         <span id='third'></span>\
         </div></body></html>",
    );
    let mut discarded = Vec::new();
    dom.drain_mutations(&mut discarded);
    let styles = StyleSet::cambium(&[
        "span { z-index: calc(sibling-index() * 100 + sibling-count()); }",
    ]);
    let states = InteractionStates::default();
    let device = Device::screen(800.0, 600.0);
    let mut session = IncrementalStyle::new();
    session.update(&dom, &styles, &device, &states, &[]);

    let third = by_id(&dom, "third");
    assert_eq!(z_index(session.styles(), third), "303");

    // No selector here is structural: the widening comes from the
    // tree-counting function in the declared value.
    dom.remove(by_id(&dom, "doomed"));
    let mut mutations = Vec::<DomMutation<NodeId>>::new();
    dom.drain_mutations(&mut mutations);
    let stats = session.update(&dom, &styles, &device, &states, &mutations);

    assert_eq!(z_index(session.styles(), third), "202");
    assert!(
        !stats.full_document,
        "a child-list change widens to the parent, not the document"
    );

    // The incremental plane agrees with a fresh full cascade.
    let full = resolve_styles(&dom, &styles, &device, &states);
    for id in ["first", "third"] {
        let node = by_id(&dom, id);
        assert_eq!(session.styles().get(node), full.get(node), "{id}");
    }
}
