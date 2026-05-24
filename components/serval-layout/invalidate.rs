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

    /// Whether recomputing this invalidation also recomputes the node's
    /// descendants (true for the subtree variants, false for a node-local repaint).
    fn covers_descendants(self) -> bool {
        matches!(
            self,
            Invalidation::RestyleSubtree(_) | Invalidation::RelayoutSubtree(_)
        )
    }

    /// How much work this invalidation implies, for subsumption ordering:
    /// restyle ⊇ relayout ⊇ repaint.
    fn strength(self) -> u8 {
        match self {
            Invalidation::RestyleSubtree(_) => 2,
            Invalidation::RelayoutSubtree(_) => 1,
            Invalidation::RepaintNode(_) => 0,
        }
    }
}

/// Reduce a batch of invalidations to a minimal set. Drops any invalidation whose
/// node is a descendant of another invalidation that (a) covers its descendants and
/// (b) is at least as strong — that ancestor's subtree recompute already does the
/// descendant's work. A weaker ancestor (e.g. relayout) does **not** subsume a
/// stronger descendant (e.g. restyle), which is kept. `parent_of` walks the DOM up.
pub fn coalesce<Id: Copy + Eq>(
    invalidations: &[Invalidation<Id>],
    parent_of: impl Fn(Id) -> Option<Id>,
) -> Vec<Invalidation<Id>> {
    let mut kept: Vec<Invalidation<Id>> = Vec::new();
    'outer: for &inv in invalidations {
        for &other in invalidations {
            if other.node() != inv.node()
                && other.covers_descendants()
                && other.strength() >= inv.strength()
                && is_ancestor(other.node(), inv.node(), &parent_of)
            {
                continue 'outer; // subsumed by a stronger/equal ancestor subtree
            }
        }
        if !kept.iter().any(|k| k.node() == inv.node()) {
            kept.push(inv);
        }
    }
    kept
}

fn is_ancestor<Id: Copy + Eq>(
    ancestor: Id,
    mut node: Id,
    parent_of: &impl Fn(Id) -> Option<Id>,
) -> bool {
    while let Some(parent) = parent_of(node) {
        if parent == ancestor {
            return true;
        }
        node = parent;
    }
    false
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

    #[test]
    fn coalesce_subsumes_descendants() {
        use std::collections::HashMap;
        // 0 → 1 → 2 → 3, and 0 → 4
        let parents: HashMap<u32, u32> = [(1, 0), (2, 1), (3, 2), (4, 0)].into_iter().collect();
        let parent_of = |id: u32| parents.get(&id).copied();
        let invs = vec![
            Invalidation::RestyleSubtree(1u32),
            Invalidation::RelayoutSubtree(2), // descendant of 1
            Invalidation::RestyleSubtree(3),  // descendant of 1
            Invalidation::RestyleSubtree(4),  // sibling subtree
        ];
        let mut nodes: Vec<u32> = coalesce(&invs, parent_of).iter().map(|i| i.node()).collect();
        nodes.sort();
        assert_eq!(nodes, vec![1, 4], "1's descendants subsumed; sibling 4 kept");
    }

    #[test]
    fn coalesce_keeps_stronger_descendant() {
        use std::collections::HashMap;
        let parents: HashMap<u32, u32> = [(1, 0)].into_iter().collect();
        let parent_of = |id: u32| parents.get(&id).copied();
        // Ancestor only relayouts; descendant needs a restyle the ancestor won't do.
        let invs = vec![
            Invalidation::RelayoutSubtree(0u32),
            Invalidation::RestyleSubtree(1),
        ];
        assert_eq!(
            coalesce(&invs, parent_of).len(),
            2,
            "weaker ancestor must not subsume a stronger descendant",
        );
    }
}
