use genet_livery::{Device, IncrementalStyle, InteractionStates, StyleSet, resolve_styles};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{
    DomMutation, LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName,
};
use livery::selector::StatePseudoClass;

fn attr(name: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(name))
}

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

fn assert_matches_full(
    dom: &ScriptedDom,
    incremental: &IncrementalStyle<NodeId>,
    styles: &StyleSet,
    states: &InteractionStates<NodeId>,
) {
    let full = resolve_styles(dom, styles, &Device::screen(800.0, 600.0), states);
    fn compare(
        dom: &ScriptedDom,
        node: NodeId,
        incremental: &IncrementalStyle<NodeId>,
        full: &genet_livery::StylePlane<NodeId>,
    ) {
        if dom.kind(node) == NodeKind::Element {
            assert_eq!(incremental.styles().get(node), full.get(node));
            assert_eq!(
                incremental.styles().custom_properties(node),
                full.custom_properties(node)
            );
        }
        for child in dom.dom_children(node) {
            compare(dom, child, incremental, full);
        }
    }
    compare(dom, dom.document(), incremental, &full);
}

#[test]
fn class_snapshot_restyles_one_branch_and_preserves_sibling_matching() {
    let mut dom = ScriptedDom::from_serialized_document(
        "<html><body>\
         <main id='left'>\
           <section id='target' class='off'><span id='leaf' class='leaf'></span></section>\
           <section id='peer' class='peer'><span></span></section>\
         </main>\
         <aside id='right'><div><span></span></div></aside>\
         </body></html>",
    );
    let mut discarded = Vec::new();
    dom.drain_mutations(&mut discarded);
    let styles =
        StyleSet::cambium(&[".on .leaf { color: #0000ff; } .on + .peer { color: #008000; }"]);
    let states = InteractionStates::default();
    let device = Device::screen(800.0, 600.0);
    let mut session = IncrementalStyle::new();
    let initial = session.update(&dom, &styles, &device, &states, &[]);
    assert!(initial.full_document);

    let target = by_id(&dom, "target");
    dom.set_attribute(target, attr("class"), "warming");
    dom.set_attribute(target, attr("class"), "on");
    dom.set_attribute(target, attr("data-mode"), "ready");
    let mut mutations = Vec::<DomMutation<NodeId>>::new();
    dom.drain_mutations(&mut mutations);
    let stats = session.update(&dom, &styles, &device, &states, &mutations);

    assert_eq!(
        stats.snapshots, 1,
        "writes coalesce to one old-state snapshot"
    );
    assert_eq!(session.snapshots()[0].changed_attributes.len(), 2);
    let class = session.snapshots()[0]
        .changed_attributes
        .iter()
        .find(|attribute| attribute.name.local.as_ref() == "class")
        .expect("class snapshot");
    assert_eq!(class.old_value.as_deref(), Some("off"));
    assert_eq!(stats.hints, 1);
    assert_eq!(stats.restyled_elements, 5);
    assert!(stats.restyled_elements < stats.total_elements);
    assert!(!stats.full_document);
    assert_eq!(
        session
            .styles()
            .computed_style(by_id(&dom, "leaf"), "color"),
        Some("#0000ff".to_string())
    );
    assert_eq!(
        session
            .styles()
            .computed_style(by_id(&dom, "peer"), "color"),
        Some("#008000".to_string())
    );
    assert_matches_full(&dom, &session, &styles, &states);
}

#[test]
fn interaction_state_restyles_only_the_stateful_subtree() {
    let dom = ScriptedDom::from_serialized_document(
        "<html><body>\
         <button id='target'><span id='leaf' class='leaf'></span></button>\
         <aside><span></span></aside>\
         </body></html>",
    );
    let styles = StyleSet::cambium(&["button:hover .leaf { color: #0000ff; }"]);
    let mut states = InteractionStates::default();
    let device = Device::screen(800.0, 600.0);
    let mut session = IncrementalStyle::new();
    session.update(&dom, &styles, &device, &states, &[]);

    states.set(by_id(&dom, "target"), StatePseudoClass::Hover, true);
    let stats = session.update(&dom, &styles, &device, &states, &[]);

    assert_eq!(stats.snapshots, 1);
    assert_eq!(stats.hints, 1);
    assert_eq!(stats.restyled_elements, 2);
    assert!(stats.selector_elements < stats.total_elements);
    assert!(!stats.full_document);
    assert_eq!(
        session
            .styles()
            .computed_style(by_id(&dom, "leaf"), "color"),
        Some("#0000ff".to_string())
    );
    assert_matches_full(&dom, &session, &styles, &states);
}

#[test]
fn structural_selector_restyles_its_parent_after_insert_and_remove() {
    let mut dom = ScriptedDom::from_serialized_document(
        "<html><body><ul id='list'><li id='first'></li></ul><aside></aside></body></html>",
    );
    let mut discarded = Vec::new();
    dom.drain_mutations(&mut discarded);
    let styles = StyleSet::cambium(&["li:last-child { color: #0000ff; }"]);
    let states = InteractionStates::default();
    let device = Device::screen(800.0, 600.0);
    let mut session = IncrementalStyle::new();
    session.update(&dom, &styles, &device, &states, &[]);

    let list = by_id(&dom, "list");
    let first = by_id(&dom, "first");
    let second = dom.create_element(dom.element_name(first).expect("li name").clone());
    dom.set_attribute(second, attr("id"), "second");
    dom.append_child(list, second);
    let mut mutations = Vec::new();
    dom.drain_mutations(&mut mutations);
    let inserted = session.update(&dom, &styles, &device, &states, &mutations);

    assert_eq!(inserted.hints, 1);
    assert_eq!(inserted.restyled_elements, 3);
    assert!(inserted.restyled_elements < inserted.total_elements);
    assert_eq!(
        session.styles().computed_style(first, "color"),
        Some("CanvasText".to_string())
    );
    assert_eq!(
        session.styles().computed_style(second, "color"),
        Some("#0000ff".to_string())
    );
    assert_matches_full(&dom, &session, &styles, &states);

    dom.remove(second);
    mutations.clear();
    dom.drain_mutations(&mut mutations);
    let removed = session.update(&dom, &styles, &device, &states, &mutations);

    assert_eq!(removed.hints, 1);
    assert_eq!(removed.restyled_elements, 2);
    assert!(removed.restyled_elements < removed.total_elements);
    assert_eq!(
        session.styles().computed_style(first, "color"),
        Some("#0000ff".to_string())
    );
    assert_matches_full(&dom, &session, &styles, &states);
}

#[test]
fn stylesheet_generation_makes_document_wide_work_explicit() {
    let dom = ScriptedDom::from_serialized_document(
        "<html><body><main id='left'></main><aside id='right'></aside></body></html>",
    );
    let mut styles = StyleSet::cambium(&["#left { color: #0000ff; }"]);
    let states = InteractionStates::default();
    let device = Device::screen(800.0, 600.0);
    let mut session = IncrementalStyle::new();
    session.update(&dom, &styles, &device, &states, &[]);

    styles
        .insert_author_rule(0, "#right { color: #008000; }", 1)
        .expect("insert rule");
    let stats = session.update(&dom, &styles, &device, &states, &[]);

    assert!(stats.stylesheet_invalidated);
    assert!(stats.full_document);
    assert_eq!(stats.restyled_elements, stats.total_elements);
    assert_eq!(
        session
            .styles()
            .computed_style(by_id(&dom, "right"), "color"),
        Some("#008000".to_string())
    );
}
