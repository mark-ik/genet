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

/// A durable reference to an element, minted from a projection and resolvable
/// against a later one.
///
/// The handle *is* the DOM node id. That is sound, and it is sound for a reason
/// worth stating, because the obvious alternative is unsound:
///
/// - **A rebuild does not invalidate it.** xilem-serval's `rebuild` reuses the
///   existing node and diffs its attributes and children in place, so a view
///   update that preserves an element preserves its id.
/// - **A dead handle can never name a live element.** `ScriptedDom` allocates
///   from a monotonic counter and never recycles an index on removal, so a
///   removed node's id is never handed to a later node. Resolution therefore
///   answers "live" or "stale", never "a different element".
///
/// A `role + name` anchor would *not* have that property: a second button
/// labelled `"Delete"` silently satisfies an anchor minted against the first.
/// Names locate an element; they do not identify one. So the captured role and
/// name here are for **diagnosis only** ("this handle was: button \"Save
/// draft\""), never for re-resolution. Re-finding is the caller's decision to
/// make out loud, with a fresh query.
///
/// A handle is scoped to one document. `ScriptedDom` fences its ids with a
/// document tag on 64-bit debug builds, so a cross-document handle trips an
/// assertion there; in release, keep handles beside the surface they came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handle<Id> {
    node: Id,
    role: Role,
    name: Option<String>,
}

impl<Id: Copy> Handle<Id> {
    /// The DOM node this handle names. Prefer
    /// [`resolve`](Projection::resolve), which reports staleness.
    pub fn node(&self) -> Id {
        self.node
    }

    /// What this handle was when it was minted, for error messages. Says nothing
    /// about what it is now.
    pub fn describe(&self) -> String {
        match &self.name {
            Some(name) => format!("{:?} {name:?}", self.role),
            None => format!("unnamed {:?}", self.role),
        }
    }
}

/// The outcome of resolving a [`Handle`] against a projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution<Id> {
    /// The element is still in the tree, and is the same element.
    Live(Id),
    /// The element is gone. It has not been replaced by something else wearing
    /// its id; that cannot happen.
    Stale,
}

impl<Id: Copy + Eq> Projection<Id> {
    /// Mint a durable handle for a node found in this projection.
    pub fn handle(&self, projected: &ProjectedNode<Id>) -> Handle<Id> {
        Handle {
            node: projected.dom,
            role: projected.node.role(),
            name: projected.node.label().map(str::to_string),
        }
    }

    /// Resolve a handle against this projection.
    ///
    /// Linear in the projection's size. A host projecting every frame and
    /// resolving many handles should index `nodes` by `dom` once; the honest
    /// shape is a scan until that is measured to matter.
    pub fn resolve(&self, handle: &Handle<Id>) -> Resolution<Id> {
        if self.nodes.iter().any(|node| node.dom == handle.node) {
            Resolution::Live(handle.node)
        } else {
            Resolution::Stale
        }
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
    use accesskit::{Action, Node as AccessNode, NodeId as AccessNodeId};
    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};

    const SHEET: &[&str] = &["div, button, custom-leaf { display: block; }"];

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
            projection
                .find_all(&ElementQuery::default().actionable(true))
                .len(),
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

    /// A view update that preserves an element preserves its id: xilem-serval's
    /// `rebuild` reuses the node and diffs attributes in place. The DOM-level
    /// equivalent is an attribute change, after which the handle still resolves to
    /// the same element.
    #[test]
    fn a_handle_survives_an_update_that_preserves_the_element() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let save = button(&mut dom, root, "Save");

        let plane = fragments(&dom);
        let projection = project_all(&dom, &plane, &mut NoLeafA11y);
        let query = ElementQuery::role(Role::Button);
        let handle = projection.handle(projection.find_one(&query).expect("button"));

        // The kind of churn a rebuild produces: attributes change, node persists.
        dom.set_attribute(save, attr_name("aria-label"), "Save draft");
        let plane = fragments(&dom);
        let reprojected = project_all(&dom, &plane, &mut NoLeafA11y);

        assert_eq!(reprojected.resolve(&handle), Resolution::Live(save));
        // The handle's captured name is a snapshot for diagnosis, not a live read.
        assert_eq!(handle.describe(), "Button \"Save\"");
    }

    /// The invariant the whole handle design rests on: `ScriptedDom` allocates
    /// from a monotonic counter and never recycles an index, so a removed node's
    /// id is never reissued. A stale handle therefore cannot alias a live element.
    ///
    /// If someone ever adds a free list to the arena, this test fails, and the
    /// `Handle` doc comment becomes a lie. That is the point of asserting it here
    /// rather than trusting it.
    #[test]
    fn a_removed_element_goes_stale_and_its_id_is_never_reissued() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let doomed = button(&mut dom, root, "Delete");

        let plane = fragments(&dom);
        let projection = project_all(&dom, &plane, &mut NoLeafA11y);
        let handle = projection.handle(
            projection
                .find_one(&ElementQuery::role(Role::Button))
                .expect("button"),
        );

        dom.remove(doomed);
        let replacement = button(&mut dom, root, "Delete");
        assert_ne!(
            replacement, doomed,
            "the arena must not reissue a removed node's id"
        );

        let plane = fragments(&dom);
        let reprojected = project_all(&dom, &plane, &mut NoLeafA11y);
        assert_eq!(
            reprojected.resolve(&handle),
            Resolution::Stale,
            "a handle to a removed element is stale, never rebound"
        );
        // ...and the replacement, identically named, is a genuinely different
        // element. A `role + name` anchor would have silently bound to it.
        let fresh = reprojected
            .find_one(&ElementQuery::role(Role::Button))
            .expect("the replacement");
        assert_eq!(fresh.dom, replacement);
    }

    /// The payoff of wiring `Leaf::accessibility`: a chisel leaf's interior is
    /// addressable by automation for free, through the same projection and the
    /// same query a DOM control uses. Without a leaf source the very same DOM
    /// yields nothing, which is what "opaque" meant before the hook existed.
    #[test]
    fn a_leaf_interior_is_queryable_like_any_control() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let leaf = dom.create_element(html("custom-leaf"));
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
