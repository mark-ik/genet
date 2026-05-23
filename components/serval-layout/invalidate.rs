/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM-mutation → invalidation classification — the foundation of incremental
//! relayout (#2(b)). Translates each [`DomMutation`] into the scope that must be
//! recomputed and how much (restyle vs relayout vs repaint).
//!
//! The classification is deliberately **conservative**: when in doubt it picks a
//! larger scope (or restyle over relayout) rather than risk a stale result. The
//! coarse full-recompute path (`serval_scripted::relayout_if_dirty`) is the
//! correctness oracle this is diff-tested against — so over-invalidation only
//! costs time, never correctness, and the oracle catches *under*-invalidation.

use layout_dom_api::DomMutation;

/// What a DOM mutation invalidates: the node whose subtree must be recomputed, and
/// how deep the recompute goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Invalidation<Id> {
    /// Re-match selectors + cascade + lay out + paint this node's subtree. Needed
    /// when matching might change (attribute/class edits, structural changes).
    RestyleSubtree(Id),
    /// Lay out + paint this node's subtree; selector matching is unchanged. Needed
    /// for character-data edits (line metrics change, styles don't).
    RelayoutSubtree(Id),
    /// Repaint this node only — geometry unchanged. (Produced by a future
    /// style-diff that detects paint-only `RestyleDamage`; no `DomMutation` maps
    /// here yet.)
    RepaintNode(Id),
}

impl<Id: Copy> Invalidation<Id> {
    /// The node this invalidation is rooted at.
    pub fn node(self) -> Id {
        match self {
            Invalidation::RestyleSubtree(node)
            | Invalidation::RelayoutSubtree(node)
            | Invalidation::RepaintNode(node) => node,
        }
    }
}

/// Classify a single mutation into its invalidation scope.
pub fn classify<Id: Copy>(mutation: &DomMutation<Id>) -> Invalidation<Id> {
    match mutation {
        // A class/id/style attribute can change which selectors match the element
        // and its descendants → restyle the subtree.
        DomMutation::AttributeChanged { node, .. } => Invalidation::RestyleSubtree(*node),
        // Insert/remove affects sibling- and child-dependent selectors and the
        // parent's flow → restyle the parent's subtree.
        DomMutation::Inserted { parent, .. } => Invalidation::RestyleSubtree(*parent),
        DomMutation::Removed { former_parent, .. } => Invalidation::RestyleSubtree(*former_parent),
        // innerHTML rebuilt the node's children → restyle the subtree.
        DomMutation::SubtreeReplaced { node } => Invalidation::RestyleSubtree(*node),
        // Text edits change line metrics but not selector matching → relayout only.
        DomMutation::CharacterDataChanged { node } => Invalidation::RelayoutSubtree(*node),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::{LocalName, Namespace, QualName};

    fn class_attr() -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from("class"))
    }

    #[test]
    fn classify_maps_each_mutation() {
        type Id = u32;
        assert_eq!(
            classify::<Id>(&DomMutation::AttributeChanged {
                node: 1,
                name: class_attr(),
            }),
            Invalidation::RestyleSubtree(1),
        );
        assert_eq!(
            classify::<Id>(&DomMutation::Inserted { node: 2, parent: 3 }),
            Invalidation::RestyleSubtree(3),
        );
        assert_eq!(
            classify::<Id>(&DomMutation::Removed {
                node: 4,
                former_parent: 5,
            }),
            Invalidation::RestyleSubtree(5),
        );
        assert_eq!(
            classify::<Id>(&DomMutation::SubtreeReplaced { node: 6 }),
            Invalidation::RestyleSubtree(6),
        );
        assert_eq!(
            classify::<Id>(&DomMutation::CharacterDataChanged { node: 7 }),
            Invalidation::RelayoutSubtree(7),
        );
        assert_eq!(Invalidation::RelayoutSubtree(7).node(), 7);
    }
}
