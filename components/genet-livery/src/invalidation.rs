//! Neutral incremental style invalidation for Livery.
//!
//! The mutable DOM reports facts as `DomMutation`; this module owns the style
//! snapshots, conservative restyle hints, and retained computed plane. It uses
//! selector dependency summaries only to widen a scope. Ambiguity therefore
//! costs work, never correctness.

use std::{collections::HashMap, hash::Hash};

use layout_dom_api::{DomMutation, LayoutDom, NodeKind, QualName};
use livery::{ComputedValues, custom::CustomProperties, media::Device};

use crate::style::resolve_subtree;
use crate::{InteractionStates, SelectorTree, StylePlane, StyleSet, resolve_styles};

/// One pre-mutation attribute value retained for invalidation diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeSnapshot {
    pub name: QualName,
    pub old_value: Option<String>,
}

/// Coalesced change facts for one element. Repeated writes to one attribute
/// retain the first old value, the state before the whole mutation batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElementSnapshot<Id> {
    pub node: Id,
    pub changed_attributes: Vec<AttributeSnapshot>,
    pub state_changed: bool,
}

/// Work performed by the latest incremental style update.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestyleStats {
    /// Elements carrying a coalesced attribute or interaction snapshot.
    pub snapshots: usize,
    /// Disjoint subtree roots after ancestor coalescing.
    pub hints: usize,
    /// Elements whose cascade was recomputed.
    pub restyled_elements: usize,
    /// Elements whose identity was needed while selector matching.
    pub selector_elements: usize,
    /// Attached elements retained after this pass.
    pub total_elements: usize,
    /// True whenever this pass recomputed every attached element.
    pub full_document: bool,
    /// The style-set generation changed since the prior pass.
    pub stylesheet_invalidated: bool,
    /// The media device changed since the prior pass.
    pub device_invalidated: bool,
}

/// A computed style plane retained across DOM, interaction, and stylesheet
/// changes. Callers supply the mutation batch without transferring ownership;
/// this lets scripting and layout observe the same DOM log independently.
pub struct IncrementalStyle<Id> {
    plane: StylePlane<Id>,
    initialized: bool,
    force_full: bool,
    stylesheet_generation: u64,
    interaction_generation: u64,
    device: Option<Device>,
    snapshots: Vec<ElementSnapshot<Id>>,
    last_stats: RestyleStats,
}

impl<Id> Default for IncrementalStyle<Id> {
    fn default() -> Self {
        Self {
            plane: StylePlane::default(),
            initialized: false,
            force_full: false,
            stylesheet_generation: 0,
            interaction_generation: 0,
            device: None,
            snapshots: Vec::new(),
            last_stats: RestyleStats::default(),
        }
    }
}

impl<Id> IncrementalStyle<Id>
where
    Id: Copy + Eq + Hash,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn styles(&self) -> &StylePlane<Id> {
        &self.plane
    }

    pub fn snapshots(&self) -> &[ElementSnapshot<Id>] {
        &self.snapshots
    }

    pub fn last_stats(&self) -> RestyleStats {
        self.last_stats
    }

    /// Force the next update through the full-document correctness path.
    pub fn invalidate(&mut self) {
        self.force_full = true;
    }

    pub fn update<D>(
        &mut self,
        dom: &D,
        style_set: &StyleSet,
        device: &Device,
        states: &InteractionStates<Id>,
        mutations: &[DomMutation<Id>],
    ) -> RestyleStats
    where
        D: LayoutDom<NodeId = Id>,
    {
        let state_nodes: Vec<_> = states
            .changed_since(self.interaction_generation)
            .filter(|id| dom.is_live(*id))
            .collect();
        self.snapshots = build_snapshots(mutations, &state_nodes);

        let generation = style_set.generation();
        let stylesheet_invalidated = self.initialized && generation != self.stylesheet_generation;
        let device_invalidated = self.initialized && self.device != Some(*device);
        if !self.initialized || self.force_full || stylesheet_invalidated || device_invalidated {
            self.plane = resolve_styles(dom, style_set, device, states);
            self.initialized = true;
            self.force_full = false;
            self.stylesheet_generation = generation;
            self.interaction_generation = states.generation();
            self.device = Some(*device);
            let total = self.plane.len();
            self.last_stats = RestyleStats {
                snapshots: self.snapshots.len(),
                hints: usize::from(total > 0),
                restyled_elements: total,
                selector_elements: total,
                total_elements: total,
                full_document: total > 0,
                stylesheet_invalidated,
                device_invalidated,
            };
            return self.last_stats;
        }

        let sibling_dependencies = style_set.has_sibling_dependencies();
        let structural_dependencies = style_set.has_structural_dependencies();
        let mut roots = Vec::new();

        for mutation in mutations {
            match mutation {
                DomMutation::AttributeChanged { node, .. } => {
                    push_element_hint(dom, &mut roots, *node, sibling_dependencies);
                },
                DomMutation::Inserted { node, parent } => {
                    if structural_dependencies {
                        push_element_hint(dom, &mut roots, *parent, sibling_dependencies);
                    } else {
                        push_root(dom, &mut roots, *node);
                    }
                },
                DomMutation::Removed {
                    node,
                    former_parent,
                } => {
                    remove_subtree(dom, &mut self.plane, *node);
                    if structural_dependencies {
                        push_element_hint(dom, &mut roots, *former_parent, sibling_dependencies);
                    }
                },
                DomMutation::CharacterDataChanged { node } => {
                    if structural_dependencies && let Some(parent) = dom.parent(*node) {
                        push_element_hint(dom, &mut roots, parent, sibling_dependencies);
                    }
                },
                DomMutation::SubtreeReplaced { node } => {
                    if structural_dependencies {
                        push_element_hint(dom, &mut roots, *node, sibling_dependencies);
                    } else {
                        push_root(dom, &mut roots, *node);
                    }
                },
                DomMutation::Moved {
                    node,
                    from_parent,
                    to_parent,
                } => {
                    if structural_dependencies {
                        push_element_hint(dom, &mut roots, *from_parent, sibling_dependencies);
                        push_element_hint(dom, &mut roots, *to_parent, sibling_dependencies);
                    } else {
                        push_root(dom, &mut roots, *node);
                    }
                },
            }
        }
        for node in state_nodes {
            push_element_hint(dom, &mut roots, node, sibling_dependencies);
        }

        // innerHTML can retire old ids before the mutation is observed.
        self.plane.retain(|id| dom.is_live(id));
        roots.retain(|id| dom.is_live(*id));
        let selector_tree = SelectorTree::for_roots(
            dom,
            states,
            &roots,
            sibling_dependencies || structural_dependencies,
        );
        let selector_elements = selector_tree.len();
        let mut restyled_elements = 0;
        for root in roots.iter().copied() {
            let (parent, parent_custom) = inherited_parent(dom, &self.plane, root);
            remove_subtree(dom, &mut self.plane, root);
            restyled_elements += resolve_subtree(
                &selector_tree,
                style_set,
                device,
                root,
                parent.as_ref(),
                parent_custom.as_ref(),
                &mut self.plane,
            );
        }

        self.stylesheet_generation = generation;
        self.interaction_generation = states.generation();
        self.device = Some(*device);
        let total = self.plane.len();
        self.last_stats = RestyleStats {
            snapshots: self.snapshots.len(),
            hints: roots.len(),
            restyled_elements,
            selector_elements,
            total_elements: total,
            full_document: total > 0 && restyled_elements >= total,
            stylesheet_invalidated: false,
            device_invalidated: false,
        };
        self.last_stats
    }
}

fn build_snapshots<Id>(
    mutations: &[DomMutation<Id>],
    state_nodes: &[Id],
) -> Vec<ElementSnapshot<Id>>
where
    Id: Copy + Eq + Hash,
{
    let mut snapshots = HashMap::<Id, ElementSnapshot<Id>>::new();
    for mutation in mutations {
        if let DomMutation::AttributeChanged {
            node,
            name,
            old_value,
        } = mutation
        {
            let snapshot = snapshots.entry(*node).or_insert_with(|| ElementSnapshot {
                node: *node,
                changed_attributes: Vec::new(),
                state_changed: false,
            });
            if !snapshot
                .changed_attributes
                .iter()
                .any(|attribute| attribute.name == *name)
            {
                snapshot.changed_attributes.push(AttributeSnapshot {
                    name: name.clone(),
                    old_value: old_value.clone(),
                });
            }
        }
    }
    for node in state_nodes {
        snapshots
            .entry(*node)
            .and_modify(|snapshot| snapshot.state_changed = true)
            .or_insert_with(|| ElementSnapshot {
                node: *node,
                changed_attributes: Vec::new(),
                state_changed: true,
            });
    }
    snapshots.into_values().collect()
}

fn push_element_hint<D>(
    dom: &D,
    roots: &mut Vec<D::NodeId>,
    node: D::NodeId,
    sibling_dependencies: bool,
) where
    D: LayoutDom,
{
    if !dom.is_live(node) {
        return;
    }
    let root = if sibling_dependencies {
        dom.parent(node).unwrap_or(node)
    } else {
        node
    };
    push_root(dom, roots, root);
}

fn push_root<D>(dom: &D, roots: &mut Vec<D::NodeId>, root: D::NodeId)
where
    D: LayoutDom,
{
    if !dom.is_live(root) || roots.iter().any(|ancestor| contains(dom, *ancestor, root)) {
        return;
    }
    roots.retain(|descendant| !contains(dom, root, *descendant));
    roots.push(root);
}

fn contains<D>(dom: &D, ancestor: D::NodeId, mut node: D::NodeId) -> bool
where
    D: LayoutDom,
{
    loop {
        if node == ancestor {
            return true;
        }
        let Some(parent) = dom.parent(node) else {
            return false;
        };
        node = parent;
    }
}

fn inherited_parent<D>(
    dom: &D,
    plane: &StylePlane<D::NodeId>,
    root: D::NodeId,
) -> (Option<ComputedValues>, Option<CustomProperties>)
where
    D: LayoutDom,
{
    let mut parent = dom.parent(root);
    while let Some(id) = parent {
        if dom.kind(id) == NodeKind::Element {
            return (plane.get(id).cloned(), plane.custom_properties(id).cloned());
        }
        parent = dom.parent(id);
    }
    (None, None)
}

fn remove_subtree<D>(dom: &D, plane: &mut StylePlane<D::NodeId>, root: D::NodeId)
where
    D: LayoutDom,
{
    if dom.is_live(root) {
        for child in dom.dom_children(root) {
            remove_subtree(dom, plane, child);
        }
    }
    plane.remove(root);
}
