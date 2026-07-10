/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Semantic element queries over a laid-out DOM: find a node the way a person
//! describes it, by what it *is* and what it is *called*.
//!
//! Automation and tests address elements through the same
//! [`Projection`](crate::Projection) a screen reader reads, rather than through
//! coordinates or a parallel index. Two consequences fall out of that, and both
//! are the point:
//!
//! - A query cannot drift from what assistive tech sees, because there is one
//!   projection and one walk. If a control is unfindable here, it is unreachable
//!   for a screen reader too, and that is a bug in the app, not in the query.
//! - Chisel leaf interiors are queryable for free. A `Knob` that announces itself
//!   as a slider is found by `role = Slider`, because the leaf filled its own node
//!   during the same walk.
//!
//! Geometry is deliberately absent from the query vocabulary. A caller that needs
//! a click point reads [`bounds`](accesskit::Node::bounds) off the match.

use accesskit::Role;

use crate::a11y::{ProjectedNode, Projection};

/// How an accessible name must relate to the queried text.
///
/// Names are author-facing prose, so exactness is often the wrong test: a button
/// labelled `"Save draft"` should be findable as `"Save"` when that is what the
/// caller knows. Both are offered rather than guessing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameMatch {
    /// The accessible name equals this text exactly.
    Exact(String),
    /// The accessible name contains this text.
    Contains(String),
}

impl NameMatch {
    fn matches(&self, name: Option<&str>) -> bool {
        let Some(name) = name else {
            // A node with no accessible name matches no name query. Absent is not
            // empty: an unnamed control is a defect, and a query that silently
            // matched it would hide the defect.
            return false;
        };
        match self {
            Self::Exact(text) => name == text,
            Self::Contains(text) => name.contains(text.as_str()),
        }
    }
}

/// What to look for. An empty query matches every projected node; each field
/// added narrows it. Fields are ANDed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementQuery {
    /// The node's AccessKit role, after ARIA `role=` and leaf promotion.
    pub role: Option<Role>,
    /// The node's accessible name (`aria-label`, else its direct text, else a
    /// leaf's fallback).
    pub name: Option<NameMatch>,
    /// Whether the node must advertise an action a host can route. `Some(true)`
    /// finds only things that can actually be operated.
    pub actionable: Option<bool>,
}

impl ElementQuery {
    pub fn role(role: Role) -> Self {
        Self {
            role: Some(role),
            ..Self::default()
        }
    }

    pub fn named(mut self, name: NameMatch) -> Self {
        self.name = Some(name);
        self
    }

    pub fn actionable(mut self, actionable: bool) -> Self {
        self.actionable = Some(actionable);
        self
    }

    fn matches<Id>(&self, projected: &ProjectedNode<Id>) -> bool {
        if let Some(role) = self.role {
            if projected.node.role() != role {
                return false;
            }
        }
        if let Some(name) = &self.name {
            if !name.matches(projected.node.label()) {
                return false;
            }
        }
        if let Some(actionable) = self.actionable {
            let routable = crate::a11y::ROUTABLE_ACTIONS
                .iter()
                .any(|action| projected.node.supports_action(*action));
            if routable != actionable {
                return false;
            }
        }
        true
    }
}

impl<Id: Copy> Projection<Id> {
    /// Every node matching `query`, in the projection's own order (children
    /// before parents).
    pub fn find_all(&self, query: &ElementQuery) -> Vec<&ProjectedNode<Id>> {
        self.nodes
            .iter()
            .filter(|projected| query.matches(projected))
            .collect()
    }

    /// The single node matching `query`.
    ///
    /// `None` when nothing matched *or when more than one thing did*. An
    /// ambiguous query is a caller error, not a coin flip: silently taking the
    /// first match is how a test starts passing against the wrong button.
    pub fn find_one(&self, query: &ElementQuery) -> Option<&ProjectedNode<Id>> {
        let mut matches = self.find_all(query).into_iter();
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::{LeafA11ySource, NoLeafA11y, project};
    use crate::{ImagePlane, StylePlane, layout, run_cascade};
    use accesskit::{Action, NodeId as AccessNodeId, Node as AccessNode};
    use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    const SHEET: &[&str] = &["div, button, chisel-leaf { display: block; }"];

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

    fn fragments(dom: &ScriptedDom) -> crate::FragmentPlane<NodeId> {
        let mut styles = StylePlane::new();
        run_cascade(dom, &mut styles, euclid::Size2D::new(800.0, 600.0), SHEET, None);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        layout(dom, &styles, &ImagePlane::new(), viewport).0
    }

    fn project_all<'a>(
        dom: &'a ScriptedDom,
        plane: &'a crate::FragmentPlane<NodeId>,
        leaves: &mut dyn LeafA11ySource,
    ) -> Projection<NodeId> {
        project(
            dom,
            plane,
            dom.document(),
            &|d: &ScriptedDom, n: NodeId| AccessNodeId(d.opaque_id(n)),
            &|_d: &ScriptedDom, _n: NodeId| false,
            leaves,
            true,
        )
    }

    /// A knob leaf, as chisel's `Knob` describes itself.
    struct KnobLeaf;

    impl LeafA11ySource for KnobLeaf {
        fn describe_leaf(&mut self, _key: u64, node: &mut AccessNode) {
            node.set_role(Role::Slider);
            node.set_numeric_value(0.4);
            node.add_action(Action::SetValue);
        }
    }

    fn button(dom: &mut ScriptedDom, parent: NodeId, label: &str) -> NodeId {
        let node = dom.create_element(html("button"));
        let text = dom.create_text(label);
        dom.append_child(node, text);
        dom.append_child(parent, node);
        node
    }

    #[test]
    fn finds_a_control_by_role_and_name() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let save = button(&mut dom, root, "Save draft");
        let cancel = button(&mut dom, root, "Cancel");

        let plane = fragments(&dom);
        let projection = project_all(&dom, &plane, &mut NoLeafA11y);

        let exact = ElementQuery::role(Role::Button).named(NameMatch::Exact("Cancel".into()));
        assert_eq!(projection.find_one(&exact).map(|m| m.dom), Some(cancel));

        // Partial names are how a caller usually knows a control.
        let partial = ElementQuery::role(Role::Button).named(NameMatch::Contains("Save".into()));
        assert_eq!(projection.find_one(&partial).map(|m| m.dom), Some(save));

        // Both buttons are routable; neither the document nor the text is.
        assert_eq!(
            projection.find_all(&ElementQuery::default().actionable(true)).len(),
            2
        );
    }

    /// An ambiguous query is a caller error. Returning the first match is how a
    /// test starts silently passing against the wrong button.
    #[test]
    fn find_one_refuses_an_ambiguous_match() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        button(&mut dom, root, "Delete");
        button(&mut dom, root, "Delete");

        let plane = fragments(&dom);
        let projection = project_all(&dom, &plane, &mut NoLeafA11y);

        let query = ElementQuery::role(Role::Button).named(NameMatch::Exact("Delete".into()));
        assert_eq!(projection.find_all(&query).len(), 2);
        assert!(
            projection.find_one(&query).is_none(),
            "two matches must not resolve to one"
        );
    }

    /// The payoff of wiring `Leaf::accessibility`: a chisel leaf's interior is
    /// addressable by automation for free, through the same projection and the
    /// same query a DOM control uses. Without a leaf source the very same DOM
    /// yields nothing, which is what "opaque" meant before the hook existed.
    #[test]
    fn a_leaf_interior_is_queryable_like_any_control() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let leaf = dom.create_element(html("chisel-leaf"));
        dom.set_attribute(leaf, attr_name("key"), "1");
        dom.append_child(root, leaf);

        let plane = fragments(&dom);
        let query = ElementQuery::role(Role::Slider).actionable(true);

        let opaque = project_all(&dom, &plane, &mut NoLeafA11y);
        assert!(
            opaque.find_one(&query).is_none(),
            "an undescribed leaf is unreachable, for automation and screen readers alike"
        );

        let described = project_all(&dom, &plane, &mut KnobLeaf);
        let found = described.find_one(&query).expect("the knob is findable");
        assert_eq!(found.dom, leaf);
        assert_eq!(found.node.numeric_value(), Some(0.4));
        assert_eq!(described.actionable(), vec![leaf]);
    }
}

