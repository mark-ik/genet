/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `LayoutDom` + layout fragments -> AccessKit tree emission.
//!
//! The builder lives beside `ServalLaneView` so every consumer with a laid-out
//! Serval lane can ask for the same accessibility tree. The host still owns the
//! platform adapter; this module only emits the engine-side `TreeUpdate`.

use std::collections::HashMap;
use std::hash::Hash;

use accesskit::{
    Node as AccessNode, NodeId as AccessNodeId, Rect, Role, Toggled, Tree, TreeId, TreeUpdate,
};
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

use crate::fragment::FragmentPlane;

fn access_id<D: LayoutDom>(dom: &D, node: D::NodeId) -> AccessNodeId {
    AccessNodeId(dom.opaque_id(node))
}

fn attr<'a, D>(dom: &'a D, node: D::NodeId, name: &str) -> Option<&'a str>
where
    D: LayoutDom,
{
    dom.attribute(node, &Namespace::default(), &LocalName::from(name))
}

fn role_for<D>(dom: &D, node: D::NodeId) -> Role
where
    D: LayoutDom,
{
    if dom.kind(node) == NodeKind::Element {
        if let Some(role) = attr(dom, node, "role") {
            match role {
                "button" => return Role::Button,
                "checkbox" => return Role::CheckBox,
                "radio" => return Role::RadioButton,
                "radiogroup" => return Role::RadioGroup,
                "switch" => return Role::Switch,
                _ => {},
            }
        }
    }

    match dom.kind(node) {
        NodeKind::Document => Role::Window,
        NodeKind::Element => match dom.element_name(node).map(|q| q.local.as_ref()) {
            Some("button") => Role::Button,
            Some("input") => Role::TextInput,
            Some("p") => Role::Paragraph,
            Some("label") => Role::Label,
            Some("html") => Role::Document,
            _ => Role::GenericContainer,
        },
        _ => Role::GenericContainer,
    }
}

fn direct_text<D>(dom: &D, node: D::NodeId) -> String
where
    D: LayoutDom,
{
    let mut name = String::new();
    for child in dom.dom_children(node) {
        if dom.kind(child) == NodeKind::Text {
            if let Some(text) = dom.text(child) {
                name.push_str(text);
            }
        }
    }
    name
}

fn build<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    origins: &HashMap<D::NodeId, (f32, f32)>,
    node: D::NodeId,
    out: &mut Vec<(AccessNodeId, AccessNode)>,
) -> AccessNodeId
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let id = access_id(dom, node);
    let mut access = AccessNode::new(role_for(dom, node));

    let name = direct_text(dom, node);
    if !name.is_empty() {
        access.set_label(name);
    }

    if let Some(toggled) = attr(dom, node, "aria-checked").and_then(|v| match v {
        "true" => Some(Toggled::True),
        "false" => Some(Toggled::False),
        "mixed" => Some(Toggled::Mixed),
        _ => None,
    }) {
        access.set_toggled(toggled);
    }

    if let (Some(&(x0, y0)), Some(layout)) = (origins.get(&node), fragments.rect_of(node)) {
        let (x0, y0) = (x0 as f64, y0 as f64);
        access.set_bounds(Rect::new(
            x0,
            y0,
            x0 + layout.size.width as f64,
            y0 + layout.size.height as f64,
        ));
    }

    let mut children = Vec::new();
    for child in dom.dom_children(node) {
        if dom.kind(child) == NodeKind::Element {
            children.push(build(dom, fragments, origins, child, out));
        }
    }
    access.set_children(children);

    out.push((id, access));
    id
}

/// Emit an AccessKit tree for a laid-out Serval DOM.
pub fn accesskit_tree<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    focus: Option<D::NodeId>,
) -> TreeUpdate
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let root = dom.document();
    let origins: HashMap<D::NodeId, (f32, f32)> =
        crate::serval_lane::accumulate_origins(dom, fragments)
            .into_iter()
            .map(|(id, p)| (id, (p.x, p.y)))
            .collect();
    let mut nodes = Vec::new();
    build(dom, fragments, &origins, root, &mut nodes);

    TreeUpdate {
        nodes,
        tree: Some(Tree::new(access_id(dom, root))),
        tree_id: TreeId::ROOT,
        focus: access_id(dom, focus.unwrap_or(root)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImagePlane, StylePlane, layout, run_cascade};
    use layout_dom_api::{LayoutDomMut, QualName};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    const SHEET: &[&str] = &["div, p, button { display: block; }"];

    fn html(local: &str) -> QualName {
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/1999/xhtml"),
            LocalName::from(local),
        )
    }

    fn attr_name(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    fn fragments_from_scripted_dom(dom: &ScriptedDom) -> FragmentPlane<NodeId> {
        let mut styles = StylePlane::new();
        run_cascade(
            dom,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            SHEET,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        layout(dom, &styles, &ImagePlane::new(), viewport).0
    }

    #[test]
    fn dom_maps_to_accessibility_tree() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(html("div"));
        dom.append_child(root, div);
        let p = dom.create_element(html("p"));
        dom.append_child(div, p);
        let count = dom.create_text("13");
        dom.append_child(p, count);
        let button = dom.create_element(html("button"));
        dom.append_child(div, button);
        let plus = dom.create_text("+");
        dom.append_child(button, plus);

        let fragments = fragments_from_scripted_dom(&dom);
        let tree = accesskit_tree(&dom, &fragments, Some(button));

        assert_eq!(tree.tree_id, TreeId::ROOT);
        assert_eq!(tree.tree.as_ref().unwrap().root, access_id(&dom, root));
        assert_eq!(tree.focus, access_id(&dom, button));

        let node = |n: NodeId| {
            tree.nodes
                .iter()
                .find(|(id, _)| *id == access_id(&dom, n))
                .map(|(_, node)| node)
                .unwrap_or_else(|| panic!("node missing from a11y tree"))
        };

        let root_node = node(root);
        assert_eq!(root_node.role(), Role::Window);
        assert!(root_node.children().contains(&access_id(&dom, div)));

        let button_node = node(button);
        assert_eq!(button_node.role(), Role::Button);
        assert_eq!(button_node.label(), Some("+"));
        assert!(button_node.bounds().is_some(), "laid-out node has bounds");
        assert!(button_node.children().is_empty());

        let p_node = node(p);
        assert_eq!(p_node.role(), Role::Paragraph);
        assert_eq!(p_node.label(), Some("13"));

        assert!(
            tree.nodes
                .iter()
                .all(|(id, _)| *id != access_id(&dom, plus)),
            "text nodes are folded into element labels"
        );
    }

    #[test]
    fn aria_role_and_checked_reach_the_tree() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(html("div"));
        dom.append_child(root, div);

        let radio = dom.create_element(html("div"));
        dom.set_attribute(radio, attr_name("role"), "radio");
        dom.set_attribute(radio, attr_name("aria-checked"), "true");
        dom.append_child(div, radio);

        let switch = dom.create_element(html("button"));
        dom.set_attribute(switch, attr_name("role"), "switch");
        dom.set_attribute(switch, attr_name("aria-checked"), "false");
        dom.append_child(div, switch);

        let fragments = fragments_from_scripted_dom(&dom);
        let tree = accesskit_tree(&dom, &fragments, None);
        let node = |n: NodeId| {
            tree.nodes
                .iter()
                .find(|(id, _)| *id == access_id(&dom, n))
                .map(|(_, node)| node)
                .unwrap_or_else(|| panic!("node missing from a11y tree"))
        };

        let radio_node = node(radio);
        assert_eq!(
            radio_node.role(),
            Role::RadioButton,
            "role attr overrides the div tag"
        );
        assert_eq!(
            radio_node.toggled(),
            Some(Toggled::True),
            "aria-checked=true is checked"
        );

        let switch_node = node(switch);
        assert_eq!(
            switch_node.role(),
            Role::Switch,
            "role attr overrides the button tag"
        );
        assert_eq!(
            switch_node.toggled(),
            Some(Toggled::False),
            "aria-checked=false is unchecked"
        );
    }

    #[test]
    fn serval_lane_view_emits_accessibility_tree() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let button = dom.create_element(html("button"));
        dom.append_child(root, button);
        let label = dom.create_text("Go");
        dom.append_child(button, label);

        let mut styles = StylePlane::new();
        run_cascade(
            &dom,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            SHEET,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&dom, &styles, &ImagePlane::new(), viewport);
        let view = crate::ServalLaneView::new(&dom, &styles, &fragments);

        let tree = view.accesskit_tree(Some(button));
        assert_eq!(tree.focus, access_id(&dom, button));
        assert!(
            tree.nodes
                .iter()
                .any(|(id, node)| *id == access_id(&dom, button)
                    && node.role() == Role::Button
                    && node.label() == Some("Go"))
        );
    }

    #[test]
    fn focus_falls_back_to_root() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(html("div"));
        dom.append_child(root, div);

        let fragments = fragments_from_scripted_dom(&dom);
        let tree = accesskit_tree(&dom, &fragments, None);
        assert_eq!(tree.focus, access_id(&dom, root));
    }
}
