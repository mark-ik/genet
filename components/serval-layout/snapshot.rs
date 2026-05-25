/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stylo element snapshots from the `DomMutation` stream.
//!
//! Fine-grained restyle (Stylo invalidation) is **snapshot-based**: it
//! compares an element's *old* state (classes / id / attrs) against its
//! current state, against the `Stylist`'s selector dependency map, to
//! mark only the actually-affected elements. The old state must be known
//! at restyle time — but the live DOM has already mutated.
//!
//! serval keeps the DOM provider render-state-free, so the old state
//! rides on the mutation record itself: [`DomMutation::AttributeChanged`]
//! carries `old_value`. [`build_snapshot_map`] turns a drained mutation
//! stream into a Stylo [`SnapshotMap`] (one [`Snapshot`] per changed
//! element), reconstructing each element's **pre-mutation** attribute set
//! from its current attrs + the recorded old values.
//!
//! Mirrors servo's `Document::element_attr_will_change`
//! (`components/script/dom/document.rs`): first-change-wins per element;
//! `attrs` holds the complete old set; `class_changed`/`id_changed`/
//! `other_attributes_changed` + `changed_attrs` flag what moved.
//!
//! Cf. `docs/2026-05-25_fine_grained_restyle_plan.md`.

use std::hash::Hash;

use html5ever::local_name;
use layout_dom_api::{DomMutation, LayoutDom, QualName};
use rustc_hash::FxHashMap;
use style::dom::OpaqueNode;
use style::selector_parser::{Snapshot, SnapshotMap};
use style::servo::attr::{AttrIdentifier, AttrValue};
use style::values::GenericAtomIdent;

/// Build a Stylo [`SnapshotMap`] from a drained `DomMutation` stream.
///
/// One snapshot per element that had an `AttributeChanged`; keyed by the
/// element's `OpaqueNode` (matching `TNode::opaque` in the Stylo adapter,
/// so the invalidator's `SnapshotMap::get(element)` resolves). Structural
/// mutations (insert/remove/subtree/character-data) don't produce
/// attribute snapshots — they're handled by the relayout scope, not the
/// attribute/state invalidator.
pub fn build_snapshot_map<D>(dom: &D, mutations: &[DomMutation<D::NodeId>]) -> SnapshotMap
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // Group attribute changes by node, preserving order (first change to
    // an attr wins for the old value — same as servo's early-return).
    let mut by_node: FxHashMap<D::NodeId, Vec<(&QualName, &Option<String>)>> = FxHashMap::default();
    for m in mutations {
        if let DomMutation::AttributeChanged { node, name, old_value } = m {
            by_node.entry(*node).or_default().push((name, old_value));
        }
    }

    let mut map = SnapshotMap::new();
    for (node, changes) in by_node {
        let snap = build_snapshot(dom, node, &changes);
        // `SnapshotMap` derefs to `FxHashMap<OpaqueNode, Snapshot>`.
        map.insert(OpaqueNode(dom.opaque_id(node) as usize), snap);
    }
    map
}

/// Build one element's [`Snapshot`] from its attribute changes.
fn build_snapshot<D>(
    dom: &D,
    node: D::NodeId,
    changes: &[(&QualName, &Option<String>)],
) -> Snapshot
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut snap = Snapshot::new();

    // First recorded old value per (ns, local) — the value before *any*
    // change in this batch, which is what the pre-mutation set needs.
    let mut first_old: FxHashMap<(layout_dom_api::Namespace, layout_dom_api::LocalName), Option<String>> =
        FxHashMap::default();

    for (name, old) in changes {
        let key = (name.ns.clone(), name.local.clone());
        first_old.entry(key).or_insert_with(|| (*old).clone());

        if name.local == local_name!("id") {
            snap.id_changed = true;
        } else if name.local == local_name!("class") {
            snap.class_changed = true;
        } else {
            snap.other_attributes_changed = true;
        }
        let ident = GenericAtomIdent(name.local.clone());
        if !snap.changed_attrs.contains(&ident) {
            snap.changed_attrs.push(ident);
        }
    }

    // Reconstruct the pre-mutation attribute set: start from the live
    // (post-mutation) attrs, then for each changed attr substitute its
    // old value — or drop it if it was newly added (old_value == None).
    // Unchanged attrs carry their current value (== old value).
    let mut old_attrs: Vec<(AttrIdentifier, AttrValue)> = Vec::new();
    for attr in dom.attributes(node) {
        let key = (attr.name.ns.clone(), attr.name.local.clone());
        let old_str = match first_old.get(&key) {
            Some(Some(old)) => old.clone(),       // changed: the prior value
            Some(None) => continue,               // newly added: didn't exist before
            None => attr.value.to_string(),       // unchanged: current == old
        };
        old_attrs.push((make_ident(attr.name), make_value(attr.name, old_str)));
    }
    snap.attrs = Some(old_attrs);

    snap
}

/// `QualName` → Stylo `AttrIdentifier` (the `GenericAtomIdent` wrappers
/// the same way `adapter_stylo` builds them for `each_attr_name`).
fn make_ident(name: &QualName) -> AttrIdentifier {
    AttrIdentifier {
        local_name: GenericAtomIdent(name.local.clone()),
        name: GenericAtomIdent(name.local.clone()),
        namespace: GenericAtomIdent(name.ns.clone()),
        prefix: name.prefix.clone().map(GenericAtomIdent),
    }
}

/// `(QualName, value)` → Stylo `AttrValue`, choosing the representation
/// the snapshot's readers expect: a space-token list for `class`, an atom
/// for `id`, a plain string otherwise.
fn make_value(name: &QualName, value: String) -> AttrValue {
    if name.local == local_name!("class") {
        AttrValue::from_serialized_tokenlist(value)
    } else if name.local == local_name!("id") {
        AttrValue::from_atomic(value)
    } else {
        AttrValue::String(value)
    }
}

#[cfg(test)]
mod tests {
    use html5ever::{ns, QualName};
    use layout_dom_api::{LayoutDom, LayoutDomMut};
    use selectors::attr::CaseSensitivity;
    use serval_scripted_dom::ScriptedDom;
    use style::invalidation::element::element_wrapper::ElementSnapshot;
    use style::values::AtomIdent;
    use style::Atom;

    use super::*;

    fn html_el(local: &str) -> QualName {
        QualName::new(None, ns!(html), local.into())
    }

    fn attr_name(local: &str) -> QualName {
        QualName::new(None, ns!(), local.into())
    }

    /// Build `html>p`, returning the dom + `<p>`'s id, with build
    /// mutations drained so the next `set_attribute` is the only one seen.
    fn dom_with_p() -> (ScriptedDom, <ScriptedDom as LayoutDom>::NodeId) {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let html = dom.create_element(html_el("html"));
        dom.append_child(root, html);
        let p = dom.create_element(html_el("p"));
        dom.append_child(html, p);
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        (dom, p)
    }

    /// A `class` change snapshots the element's *old* class (so the
    /// invalidator sees the pre-mutation value), flags `class_changed`,
    /// and records `class` in `changed_attrs`.
    #[test]
    fn class_change_snapshot_reports_old_class() {
        let (mut dom, p) = dom_with_p();
        dom.set_attribute(p, attr_name("class"), "old");
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink); // the "old" set is the prior state
        dom.set_attribute(p, attr_name("class"), "new");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);

        let map = build_snapshot_map(&dom, &muts);
        let snap = (*map).get(&OpaqueNode(dom.opaque_id(p) as usize))
            .expect("snapshot for <p>");

        assert!(snap.class_changed(), "class_changed should be set");
        assert!(snap.has_attrs(), "snapshot should carry the old attr set");
        // The snapshot reports the OLD class ("old"), not the live "new".
        assert!(
            snap.has_class(&AtomIdent::new(Atom::from("old")), CaseSensitivity::CaseSensitive),
            "snapshot should report the old class 'old'"
        );
        assert!(
            !snap.has_class(&AtomIdent::new(Atom::from("new")), CaseSensitivity::CaseSensitive),
            "snapshot must not report the post-mutation class 'new'"
        );
    }

    /// An `id` change snapshots the old id and flags `id_changed`.
    #[test]
    fn id_change_snapshot_reports_old_id() {
        let (mut dom, p) = dom_with_p();
        dom.set_attribute(p, attr_name("id"), "first");
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(p, attr_name("id"), "second");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);

        let map = build_snapshot_map(&dom, &muts);
        let snap = (*map).get(&OpaqueNode(dom.opaque_id(p) as usize))
            .expect("snapshot for <p>");

        assert!(snap.id_changed(), "id_changed should be set");
        let old_id = snap.id_attr().expect("snapshot id_attr present");
        assert_eq!(old_id.to_string(), "first", "old id should be 'first'");
    }

    /// A newly-added attribute (no prior value) flags `other` and isn't
    /// mistaken for a class/id change.
    #[test]
    fn newly_added_attr_flags_other_only() {
        let (mut dom, p) = dom_with_p();
        // First time this attr is set → old_value is None.
        dom.set_attribute(p, attr_name("data-x"), "v");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);

        let map = build_snapshot_map(&dom, &muts);
        let snap = (*map).get(&OpaqueNode(dom.opaque_id(p) as usize))
            .expect("snapshot for <p>");
        assert!(snap.other_attr_changed(), "other_attributes_changed should be set");
        assert!(!snap.class_changed(), "class_changed should be false");
        assert!(!snap.id_changed(), "id_changed should be false");
    }
}
