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
    Action, Node as AccessNode, NodeId as AccessNodeId, Rect, Role, Toggled, Tree, TreeId,
    TreeUpdate,
};
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

use crate::construct::chisel_leaf_key_of;
use crate::fragment::FragmentPlane;

/// Host-side source of chisel-leaf accessibility semantics.
///
/// Mirrors [`LeafPaintSource`](crate::LeafPaintSource): serval-layout knows
/// `<chisel-leaf key="…">` as an element, not chisel's types, so the host
/// bridges a leaf key to its registered leaf. A leaf fills its own AccessKit
/// node (a knob announces as a slider carrying its value) and may override the
/// role the tag walk assigned, since a leaf's interior is invisible to the DOM.
///
/// Two things are not the leaf's to decide:
///
/// - **Geometry.** The walk stamps absolute bounds after the leaf has spoken, so
///   a leaf can never disagree with layout about where it is.
/// - **A name the author gave it.** The walk resolves `aria-label` (then direct
///   text) *before* calling the leaf, so an author who wrote
///   `<chisel-leaf aria-label="Session graph">` has said what this instance is,
///   and a leaf's generic self-description must not overwrite that. A leaf that
///   wants a fallback name checks [`AccessNode::label`] first and only fills a
///   gap. Roles and values carry no such rule: those are facts about the widget,
///   not editorial choices, so a leaf always wins on them.
pub trait LeafA11ySource {
    /// Fill `node` with the semantics of the leaf registered under `key`. An
    /// absent key must leave `node` untouched (the leaf stays an opaque box).
    fn describe_leaf(&mut self, key: u64, node: &mut AccessNode);
}

/// A source with no leaves: every `<chisel-leaf>` stays an opaque container.
/// What [`accesskit_tree`] and [`build_subtree`] use when the caller has none.
pub struct NoLeafA11y;

impl LeafA11ySource for NoLeafA11y {
    fn describe_leaf(&mut self, _key: u64, _node: &mut AccessNode) {}
}

/// The actions a host can route back to a node. A node advertising any of these
/// is handed back to the caller, whether it acquired the action from its role
/// (a `<button>` takes `Click`) or from a leaf declaring its own (a slider takes
/// `SetValue` / `Increment` / `Decrement`).
pub const ROUTABLE_ACTIONS: [Action; 5] = [
    Action::Click,
    Action::Focus,
    Action::SetValue,
    Action::Increment,
    Action::Decrement,
];

/// One projected node: the AccessKit node, its id, and the DOM node it came from.
///
/// Keeping the DOM node is what lets a caller act on what it found. Automation
/// queries the same tree a screen reader reads, then routes back through the DOM,
/// so the two can never disagree about what is on screen.
pub struct ProjectedNode<Id> {
    pub dom: Id,
    pub id: AccessNodeId,
    pub node: AccessNode,
}

/// A laid-out subtree projected once: every node, in insertion order (children
/// before parents), paired with its DOM origin.
///
/// This is the single semantic projection. [`accesskit_tree`] and
/// [`build_subtree`] are views onto it, and element queries read it rather than
/// re-deriving roles from the DOM, so a query can never drift from what assistive
/// tech sees.
pub struct Projection<Id> {
    pub root: AccessNodeId,
    pub nodes: Vec<ProjectedNode<Id>>,
}

impl<Id: Copy> Projection<Id> {
    /// The DOM nodes advertising an action a host can route (see
    /// [`ROUTABLE_ACTIONS`]), whether it came from a role or from a leaf.
    pub fn actionable(&self) -> Vec<Id> {
        self.nodes
            .iter()
            .filter(|projected| {
                ROUTABLE_ACTIONS
                    .iter()
                    .any(|action| projected.node.supports_action(*action))
            })
            .map(|projected| projected.dom)
            .collect()
    }
}

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

/// The shared subtree walk behind both [`accesskit_tree`] (the sealed engine
/// tree) and [`build_subtree`] (a host stitching several subtrees). `id_of`
/// assigns each node its id, `skip` prunes element subtrees the caller projects
/// elsewhere, `leaves` fills in each `<chisel-leaf>`'s interior semantics, and
/// `advertise_actions` gates whether controls declare the host action they
/// accept (recording them in `actionable`) — off for the engine tree so hosts
/// that don't route actions don't promise affordances they can't honor.
#[allow(clippy::too_many_arguments)]
fn walk<D, I, S>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    origins: &HashMap<D::NodeId, (f32, f32)>,
    node: D::NodeId,
    id_of: &I,
    skip: &S,
    leaves: &mut dyn LeafA11ySource,
    advertise_actions: bool,
    out: &mut Vec<ProjectedNode<D::NodeId>>,
) -> AccessNodeId
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    I: Fn(&D, D::NodeId) -> AccessNodeId,
    S: Fn(&D, D::NodeId) -> bool,
{
    let id = id_of(dom, node);
    let mut access = AccessNode::new(role_for(dom, node));

    // Accessible name: `aria-label` wins (ARIA semantics), else the node's direct
    // text. Icon-only or nested controls carry no direct text, so `aria-label` is
    // how a host names them.
    let name = attr(dom, node, "aria-label")
        .map(str::to_string)
        .unwrap_or_else(|| direct_text(dom, node));
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

    // A chisel leaf is a replaced element: its interior is invisible to the DOM,
    // so the leaf speaks for itself here. It may override the role the tag walk
    // assigned, name itself, carry a value, and declare its own actions. It runs
    // after the DOM-derived semantics (so a leaf wins) and before bounds (so
    // layout wins on geometry).
    if let Some(key) = chisel_leaf_key_of(dom, node) {
        leaves.describe_leaf(key, &mut access);
    }

    if advertise_actions {
        // Toggle controls (switch / checkbox / radio) are invoked via `Click` in
        // AccessKit, same as a button; a text field takes `Focus`. Read the role
        // back off the node, not from the tag, so a leaf that promoted itself to
        // a control is treated as one.
        let action = match access.role() {
            Role::Button | Role::Switch | Role::CheckBox | Role::RadioButton => {
                Some(Action::Click)
            }
            Role::TextInput => Some(Action::Focus),
            _ => None,
        };
        if let Some(action) = action {
            access.add_action(action);
        }
        // Whether this node is routable is read back off the finished node by
        // `Projection::actionable`, so a leaf that declared `SetValue` on itself
        // counts exactly like a `<button>` that got `Click` from its role.
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
        if dom.kind(child) == NodeKind::Element && !skip(dom, child) {
            children.push(walk(
                dom,
                fragments,
                origins,
                child,
                id_of,
                skip,
                leaves,
                advertise_actions,
                out,
            ));
        }
    }
    access.set_children(children);

    out.push(ProjectedNode {
        dom: node,
        id,
        node: access,
    });
    id
}

/// Project a laid-out subtree once. Everything else in this module is a view onto
/// the result: the sealed engine tree, a host's stitchable subtree, and element
/// queries all read the same nodes.
#[allow(clippy::too_many_arguments)]
pub fn project<D, I, S>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    root: D::NodeId,
    id_of: &I,
    skip: &S,
    leaves: &mut dyn LeafA11ySource,
    advertise_actions: bool,
) -> Projection<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    I: Fn(&D, D::NodeId) -> AccessNodeId,
    S: Fn(&D, D::NodeId) -> bool,
{
    let origins = origins_of(dom, fragments);
    let mut nodes = Vec::new();
    let root_id = walk(
        dom,
        fragments,
        &origins,
        root,
        id_of,
        skip,
        leaves,
        advertise_actions,
        &mut nodes,
    );
    Projection {
        root: root_id,
        nodes,
    }
}

fn origins_of<D>(dom: &D, fragments: &FragmentPlane<D::NodeId>) -> HashMap<D::NodeId, (f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    crate::serval_lane::accumulate_origins(dom, fragments)
        .into_iter()
        .map(|(id, p)| (id, (p.x, p.y)))
        .collect()
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
    let projection = project(
        dom,
        fragments,
        root,
        &|d: &D, n: D::NodeId| access_id(d, n),
        &|_d: &D, _n: D::NodeId| false,
        &mut NoLeafA11y,
        false,
    );

    TreeUpdate {
        nodes: projection
            .nodes
            .into_iter()
            .map(|projected| (projected.id, projected.node))
            .collect(),
        tree: Some(Tree::new(access_id(dom, root))),
        tree_id: TreeId::ROOT,
        focus: access_id(dom, focus.unwrap_or(root)),
    }
}

/// Walk a laid-out subtree into AccessKit nodes for a host that stitches several
/// subtrees (chrome, content panes, host root) into one tree before converting
/// once. Returns the `(id, node)` pairs in insertion order, the subtree root's
/// id, and the DOM nodes that advertise a host action (buttons, text fields) so
/// the host can route an AccessKit request back to its activation path.
///
/// `id_of` assigns each node its id: a stitching host salts ids into a range
/// disjoint from its other subtrees, where [`accesskit_tree`] uses the DOM's
/// opaque id. `skip` prunes element subtrees the host projects elsewhere (a pane
/// it gives richer, actionable a11y of its own). Roles honor ARIA `role=` then
/// tag, and `aria-checked` sets toggled state — the same leaf logic as the
/// engine tree, so a host subtree never drifts behind on standards support.
///
/// Chisel leaves stay opaque containers here. A host with leaves calls
/// [`build_subtree_with_leaves`] instead.
#[allow(clippy::type_complexity)]
pub fn build_subtree<D, I, S>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    root: D::NodeId,
    id_of: &I,
    skip: &S,
) -> (Vec<(AccessNodeId, AccessNode)>, AccessNodeId, Vec<D::NodeId>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    I: Fn(&D, D::NodeId) -> AccessNodeId,
    S: Fn(&D, D::NodeId) -> bool,
{
    build_subtree_with_leaves(dom, fragments, root, id_of, skip, &mut NoLeafA11y)
}

/// [`build_subtree`], with each `<chisel-leaf>`'s interior filled in by `leaves`.
///
/// A leaf is a replaced element, so nothing about its interior reaches the DOM;
/// without a source it projects as an unlabeled container. With one, a `Knob`
/// announces as a slider carrying its value, a `Meter` as a meter, and a leaf
/// that declares an action (`SetValue`, `Click`) is handed back in the
/// actionable list exactly like a `<button>`, so one routing path serves DOM
/// controls and leaf interiors alike.
#[allow(clippy::type_complexity)]
pub fn build_subtree_with_leaves<D, I, S>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    root: D::NodeId,
    id_of: &I,
    skip: &S,
    leaves: &mut dyn LeafA11ySource,
) -> (Vec<(AccessNodeId, AccessNode)>, AccessNodeId, Vec<D::NodeId>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    I: Fn(&D, D::NodeId) -> AccessNodeId,
    S: Fn(&D, D::NodeId) -> bool,
{
    let projection = project(dom, fragments, root, id_of, skip, leaves, true);
    let actionable = projection.actionable();
    let root_id = projection.root;
    let nodes = projection
        .nodes
        .into_iter()
        .map(|projected| (projected.id, projected.node))
        .collect();
    (nodes, root_id, actionable)
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

    /// A `LeafA11ySource` standing in for chisel's registry: key 7 is a knob.
    struct KnobAt7;

    impl LeafA11ySource for KnobAt7 {
        fn describe_leaf(&mut self, key: u64, node: &mut AccessNode) {
            if key != 7 {
                return;
            }
            node.set_role(Role::Slider);
            node.set_label("Gain");
            node.set_numeric_value(0.25);
            node.add_action(Action::SetValue);
        }
    }

    /// A leaf offering a *fallback* name, the way `GraphGlyph` does: it fills the
    /// gap only when the author named nothing.
    struct FallbackNamed;

    impl LeafA11ySource for FallbackNamed {
        fn describe_leaf(&mut self, _key: u64, node: &mut AccessNode) {
            node.set_role(Role::GraphicsObject);
            if node.label().is_none() {
                node.set_label("graph: 3 nodes, 2 links");
            }
        }
    }

    /// The author placing a leaf knows what this instance depicts; the leaf only
    /// knows what kind of thing it is. So `aria-label` outranks a leaf's generic
    /// self-description, while the leaf still wins on role, which is a fact about
    /// the widget rather than an editorial choice.
    #[test]
    fn a_leaf_fallback_name_never_overwrites_the_authors_aria_label() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();

        let named = dom.create_element(html("chisel-leaf"));
        dom.set_attribute(named, attr_name("key"), "1");
        dom.set_attribute(named, attr_name("aria-label"), "Session graph");
        dom.append_child(root, named);

        let unnamed = dom.create_element(html("chisel-leaf"));
        dom.set_attribute(unnamed, attr_name("key"), "2");
        dom.append_child(root, unnamed);

        let fragments = fragments_from_scripted_dom(&dom);
        let (nodes, _, _) = build_subtree_with_leaves(
            &dom,
            &fragments,
            root,
            &|d: &ScriptedDom, n: NodeId| access_id(d, n),
            &|_d: &ScriptedDom, _n: NodeId| false,
            &mut FallbackNamed,
        );
        let node = |n: NodeId| {
            nodes
                .iter()
                .find(|(id, _)| *id == access_id(&dom, n))
                .map(|(_, node)| node)
                .expect("leaf in tree")
        };

        assert_eq!(
            node(named).label(),
            Some("Session graph"),
            "the author's aria-label survives the leaf"
        );
        assert_eq!(
            node(unnamed).label(),
            Some("graph: 3 nodes, 2 links"),
            "the leaf fills a name only where the author left none"
        );
        assert_eq!(
            node(named).role(),
            Role::GraphicsObject,
            "the leaf still wins on role, which is not the author's to state"
        );
    }

    /// A `<chisel-leaf>` is a replaced element, so nothing about its interior
    /// reaches the DOM. Without a source it projects as an opaque, unlabeled
    /// container; with one, the leaf names itself, promotes its own role, carries
    /// its value, and lands in the routable set on the strength of the action it
    /// declared — the same handback a `<button>` gets. Layout still owns bounds.
    #[test]
    fn chisel_leaf_interior_reaches_the_tree_through_its_source() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let leaf = dom.create_element(html("chisel-leaf"));
        dom.set_attribute(leaf, attr_name("key"), "7");
        dom.append_child(root, leaf);

        let fragments = fragments_from_scripted_dom(&dom);
        let id_of = |d: &ScriptedDom, n: NodeId| access_id(d, n);
        let no_skip = |_d: &ScriptedDom, _n: NodeId| false;

        // Without a source: opaque. The leaf is not a control and not routable.
        let (nodes, _, actionable) = build_subtree(&dom, &fragments, root, &id_of, &no_skip);
        let bare = nodes
            .iter()
            .find(|(id, _)| *id == access_id(&dom, leaf))
            .map(|(_, n)| n)
            .expect("leaf node present");
        assert_eq!(bare.role(), Role::GenericContainer, "opaque without a source");
        assert_eq!(bare.label(), None);
        assert!(actionable.is_empty(), "an opaque leaf advertises nothing");

        // With a source: the leaf speaks for itself.
        let (nodes, _, actionable) =
            build_subtree_with_leaves(&dom, &fragments, root, &id_of, &no_skip, &mut KnobAt7);
        let knob = nodes
            .iter()
            .find(|(id, _)| *id == access_id(&dom, leaf))
            .map(|(_, n)| n)
            .expect("leaf node present");
        assert_eq!(knob.role(), Role::Slider, "the leaf promoted its own role");
        assert_eq!(knob.label(), Some("Gain"));
        assert_eq!(knob.numeric_value(), Some(0.25));
        assert!(knob.supports_action(Action::SetValue));
        assert!(
            knob.bounds().is_some(),
            "layout owns geometry, not the leaf"
        );
        assert_eq!(
            actionable,
            vec![leaf],
            "a leaf that declares an action is routable, like a button"
        );
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
