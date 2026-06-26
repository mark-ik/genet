/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ScriptedDom` → [`accesskit::TreeUpdate`]: accessibility-tree emission.
//!
//! The Stage 3 `DOM → AccessKit` slice. The serval-as-host plan names a11y
//! emission as one of the two real engine-completeness costs (alongside form
//! controls), and argues it is *more* natural from a semantic DOM than from an
//! arbitrary widget tree — this builder is the concrete demonstration: it walks
//! the live [`ScriptedDom`], maps each element to an accessibility [`Role`] by
//! tag, takes its accessible name from its direct text content, and reads its
//! bounds from the layout [`FragmentPlane`]. The focused node (the runner's
//! `focus`) becomes the tree's focus.
//!
//! This is the *production* half — the pure `DOM → TreeUpdate` mapping. Surfacing
//! it to a screen reader is the host's job: an `accesskit_winit::Adapter` in the
//! window would call [`accesskit_tree`] from its `update_if_active` and feed the
//! result on each change. That live wiring is a follow-up (it needs a real
//! window + screen reader to verify); the mapping below is exercised headlessly.
//!
//! Text is *folded* into the owning element's label rather than emitted as
//! separate nodes: a `<button>+</button>` becomes one `Button` node named `"+"`,
//! matching how an accessibility tree names a control by its text content. A
//! later slice can emit inline `TextRun` nodes when richer text a11y is wanted.
//!
//! Lives in `pelt-live` (the host): a screen-reader adapter is a host concern,
//! and keeping `accesskit` out of the core layout crate avoids a load-bearing
//! dep there. If we later want this engine-side and reusable, it would graft
//! naturally onto `serval_layout::ServalLaneView` (which already bundles
//! dom + styles + fragments).

use accesskit::{
    Node as AccessNode, NodeId as AccessNodeId, Rect, Role, Toggled, Tree, TreeId, TreeUpdate,
};
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
use serval_layout::FragmentPlane;
use serval_scripted_dom::{NodeId, ScriptedDom};

/// The accesskit id for a serval node: its raw arena index (stable for the
/// document's lifetime), so a11y ids line up 1:1 with DOM nodes.
fn access_id(node: NodeId) -> AccessNodeId {
    AccessNodeId(node.raw() as u64)
}

/// Read `node`'s null-namespace attribute `name` — HTML/ARIA attributes
/// (`role`, `aria-checked`, …) live in the null namespace.
fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
    dom.attribute(node, &Namespace::default(), &LocalName::from(name))
}

/// The accessibility [`Role`] for `node`, by ARIA `role` attribute, then node
/// kind / element tag.
///
/// An explicit ARIA `role` wins over the tag, so a host control stamped on a
/// styled `<div>`/`<button>` (`role="radio"` / `"switch"` / `"checkbox"`) reaches
/// the screen reader as that control rather than a neutral container — the bridge
/// previously dropped these (`docs/2026-06-24_grand_audit.md` direction 2). An
/// unrecognised role token falls through to the tag mapping. The document root is
/// the tree's [`Role::Window`]; the handful of tags the host views use map to
/// their natural roles; anything else is a [`Role::GenericContainer`] (a neutral
/// grouping node). Text nodes never reach here — their content is folded into the
/// owning element's label.
fn role_for(dom: &ScriptedDom, node: NodeId) -> Role {
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

/// Concatenated direct text-child content of `node` — its accessible name (so a
/// `<button>+</button>` is named `"+"`, an `<input>` by its buffer, etc.).
fn direct_text(dom: &ScriptedDom, node: NodeId) -> String {
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

/// Build the accesskit node for `node` (whose parent sits at `parent_origin` in
/// absolute coordinates), appending it and its element descendants to `out`, and
/// return its id. Recursive: a node's absolute origin is its parent's plus its
/// own parent-relative layout location, so bounds accumulate down the tree.
fn build(
    dom: &ScriptedDom,
    fragments: &FragmentPlane<NodeId>,
    node: NodeId,
    parent_origin: (f64, f64),
    out: &mut Vec<(AccessNodeId, AccessNode)>,
) -> AccessNodeId {
    let id = access_id(node);
    let mut access = AccessNode::new(role_for(dom, node));

    // Accessible name from the element's direct text content.
    let name = direct_text(dom, node);
    if !name.is_empty() {
        access.set_label(name);
    }

    // A stamped `aria-checked` (on a radio / checkbox / switch role) becomes the
    // accesskit toggled state, so a screen reader announces the current selection.
    if let Some(toggled) = attr(dom, node, "aria-checked").and_then(|v| match v {
        "true" => Some(Toggled::True),
        "false" => Some(Toggled::False),
        "mixed" => Some(Toggled::Mixed),
        _ => None,
    }) {
        access.set_toggled(toggled);
    }

    // Absolute bounds from layout (taffy's `location` is parent-relative, so we
    // accumulate). A node with no fragment (e.g. the unlaid-out document root)
    // contributes no bounds and passes its parent's origin through.
    let origin = match fragments.rect_of(node) {
        Some(layout) => {
            let x0 = parent_origin.0 + layout.location.x as f64;
            let y0 = parent_origin.1 + layout.location.y as f64;
            access.set_bounds(Rect::new(
                x0,
                y0,
                x0 + layout.size.width as f64,
                y0 + layout.size.height as f64,
            ));
            (x0, y0)
        },
        None => parent_origin,
    };

    // Element children become a11y children; text children were folded into the
    // label above, so they are not separate nodes.
    let mut children = Vec::new();
    for child in dom.dom_children(node) {
        if dom.kind(child) == NodeKind::Element {
            children.push(build(dom, fragments, child, origin, out));
        }
    }
    access.set_children(children);

    out.push((id, access));
    id
}

/// Emit an [`accesskit::TreeUpdate`] for the live `dom`, using `fragments` for
/// node geometry and `focus` for the focused node.
///
/// The document node is the tree root ([`Role::Window`]); every element below it
/// becomes a node with a role, an accessible name (its text), bounds, and its
/// element children. `focus` is the runner's focused node, or the root when
/// nothing is focused (accesskit requires a focus that names a node in the tree).
pub fn accesskit_tree(
    dom: &ScriptedDom,
    fragments: &FragmentPlane<NodeId>,
    focus: Option<NodeId>,
) -> TreeUpdate {
    let root = dom.document();
    let mut nodes = Vec::new();
    build(dom, fragments, root, (0.0, 0.0), &mut nodes);
    TreeUpdate {
        nodes,
        tree: Some(Tree::new(access_id(root))),
        tree_id: TreeId::ROOT,
        focus: access_id(focus.unwrap_or(root)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::fragments_from_scripted_dom;
    use layout_dom_api::{LayoutDomMut, LocalName, Namespace, QualName};

    const SHEET: &[&str] = &["div, p, button { display: block; }"];

    fn html(local: &str) -> QualName {
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/1999/xhtml"),
            LocalName::from(local),
        )
    }

    /// A null-namespace attribute name (where HTML/ARIA attributes live).
    fn attr_name(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    /// Build `<div><p>13</p><button>+</button></div>`, lay it out, and assert the
    /// emitted accessibility tree: roles by tag, names from text, bounds from
    /// layout, the child relationships, and focus pointing at the focused node.
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

        let fragments = fragments_from_scripted_dom(&dom, SHEET, 800, 600);
        let tree = accesskit_tree(&dom, &fragments, Some(button));

        // Tree-level: root tree id, the document root as the tree root, focus on
        // the button.
        assert_eq!(tree.tree_id, TreeId::ROOT);
        assert_eq!(tree.tree.as_ref().unwrap().root, access_id(root));
        assert_eq!(tree.focus, access_id(button));

        let node = |n: NodeId| {
            tree.nodes
                .iter()
                .find(|(id, _)| *id == access_id(n))
                .map(|(_, node)| node)
                .unwrap_or_else(|| panic!("node missing from a11y tree"))
        };

        // The document root is a Window containing the <div>.
        let root_node = node(root);
        assert_eq!(root_node.role(), Role::Window);
        assert!(root_node.children().contains(&access_id(div)));

        // The <button> is a Button named "+", with bounds and no element children.
        let button_node = node(button);
        assert_eq!(button_node.role(), Role::Button);
        assert_eq!(button_node.label(), Some("+"));
        assert!(button_node.bounds().is_some(), "laid-out node has bounds");
        assert!(button_node.children().is_empty());

        // The <p> is a Paragraph named "13".
        let p_node = node(p);
        assert_eq!(p_node.role(), Role::Paragraph);
        assert_eq!(p_node.label(), Some("13"));

        // Text nodes are folded into labels, not emitted as separate a11y nodes.
        assert!(
            tree.nodes.iter().all(|(id, _)| *id != access_id(plus)),
            "the '+' text node is folded into the button's label, not its own node"
        );
    }

    /// A stamped ARIA `role` overrides the tag, and `aria-checked` becomes the
    /// accesskit toggled state — the radio-group / switch path the host controls
    /// emit (grand_audit direction 2). A `<div role="radio" aria-checked="true">`
    /// is a checked `RadioButton`; a `<button role="switch" aria-checked="false">`
    /// is an off `Switch`, its `role` winning over the `<button>` tag.
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

        let fragments = fragments_from_scripted_dom(&dom, SHEET, 800, 600);
        let tree = accesskit_tree(&dom, &fragments, None);
        let node = |n: NodeId| {
            tree.nodes
                .iter()
                .find(|(id, _)| *id == access_id(n))
                .map(|(_, node)| node)
                .unwrap_or_else(|| panic!("node missing from a11y tree"))
        };

        let radio_node = node(radio);
        assert_eq!(radio_node.role(), Role::RadioButton, "role attr overrides the <div> tag");
        assert_eq!(radio_node.toggled(), Some(Toggled::True), "aria-checked=true is checked");

        let switch_node = node(switch);
        assert_eq!(switch_node.role(), Role::Switch, "role attr overrides the <button> tag");
        assert_eq!(switch_node.toggled(), Some(Toggled::False), "aria-checked=false is unchecked");
    }

    /// With nothing focused, the tree's focus falls back to the root (accesskit
    /// requires focus to name a node in the tree).
    #[test]
    fn focus_falls_back_to_root() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(html("div"));
        dom.append_child(root, div);

        let fragments = fragments_from_scripted_dom(&dom, SHEET, 800, 600);
        let tree = accesskit_tree(&dom, &fragments, None);
        assert_eq!(tree.focus, access_id(root));
    }
}
