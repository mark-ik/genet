use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use layout_dom_api::{LayoutDom, NodeKind};
use livery::selector::{
    Atom, AttrSelectorOperation, BloomFilter, CaseSensitivity, Element, ElementSelectorFlags,
    LiverySelectorImpl, NamespaceConstraint, NoPseudoElement, OpaqueElement, StatePseudoClass,
};
use selectors::matching::MatchingContext;

/// Host-supplied dynamic pseudo-class state for one document.
pub struct InteractionStates<Id> {
    states: HashMap<Id, HashSet<StatePseudoClass>>,
}

impl<Id> Default for InteractionStates<Id> {
    fn default() -> Self {
        Self {
            states: HashMap::new(),
        }
    }
}

impl<Id> InteractionStates<Id>
where
    Id: Copy + Eq + std::hash::Hash,
{
    pub fn set(&mut self, id: Id, state: StatePseudoClass, enabled: bool) {
        if enabled {
            self.states.entry(id).or_default().insert(state);
        } else if let Some(states) = self.states.get_mut(&id) {
            states.remove(&state);
            if states.is_empty() {
                self.states.remove(&id);
            }
        }
    }

    pub fn matches(&self, id: Id, state: StatePseudoClass) -> bool {
        self.states
            .get(&id)
            .is_some_and(|states| states.contains(&state))
    }
}

/// Stable identity and state storage used while matching one DOM.
pub struct SelectorTree<'a, D: LayoutDom> {
    dom: &'a D,
    identities: HashMap<D::NodeId, Box<u64>>,
    states: &'a InteractionStates<D::NodeId>,
}

impl<'a, D: LayoutDom> SelectorTree<'a, D> {
    pub fn new(dom: &'a D, states: &'a InteractionStates<D::NodeId>) -> Self {
        let mut identities = HashMap::new();
        let mut pending = vec![dom.document()];
        while let Some(id) = pending.pop() {
            identities.insert(id, Box::new(dom.opaque_id(id)));
            pending.extend(dom.dom_children(id));
        }
        Self {
            dom,
            identities,
            states,
        }
    }

    pub fn dom(&self) -> &'a D {
        self.dom
    }

    pub fn element(&self, id: D::NodeId) -> Option<ElementRef<'_, 'a, D>> {
        (self.dom.kind(id) == NodeKind::Element).then_some(ElementRef { tree: self, id })
    }
}

/// A selector-facing element reference over a neutral Genet DOM.
pub struct ElementRef<'tree, 'dom, D: LayoutDom> {
    tree: &'tree SelectorTree<'dom, D>,
    id: D::NodeId,
}

impl<D: LayoutDom> Copy for ElementRef<'_, '_, D> {}

impl<D: LayoutDom> Clone for ElementRef<'_, '_, D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: LayoutDom> fmt::Debug for ElementRef<'_, '_, D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ElementRef").field(&self.id).finish()
    }
}

impl<'tree, 'dom, D: LayoutDom> ElementRef<'tree, 'dom, D> {
    pub fn id(self) -> D::NodeId {
        self.id
    }

    fn dom(self) -> &'dom D {
        self.tree.dom
    }

    fn attribute(self, namespace: &str, local: &str) -> Option<&'dom str> {
        self.dom().attributes(self.id).find_map(|attribute| {
            (attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local)
                .then_some(attribute.value)
        })
    }

    fn sibling_element(self, previous: bool) -> Option<Self> {
        let mut sibling = if previous {
            self.dom().prev_sibling(self.id)
        } else {
            self.dom().next_sibling(self.id)
        };
        while let Some(id) = sibling {
            if let Some(element) = self.tree.element(id) {
                return Some(element);
            }
            sibling = if previous {
                self.dom().prev_sibling(id)
            } else {
                self.dom().next_sibling(id)
            };
        }
        None
    }
}

impl<D: LayoutDom> Element for ElementRef<'_, '_, D> {
    type Impl = LiverySelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(
            self.tree
                .identities
                .get(&self.id)
                .expect("selector identity exists")
                .as_ref(),
        )
    }

    fn parent_element(&self) -> Option<Self> {
        let mut parent = self.dom().parent(self.id);
        while let Some(id) = parent {
            if let Some(element) = self.tree.element(id) {
                return Some(element);
            }
            parent = self.dom().parent(id);
        }
        None
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling_element(true)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling_element(false)
    }

    fn first_element_child(&self) -> Option<Self> {
        self.dom()
            .dom_children(self.id)
            .find_map(|id| self.tree.element(id))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.dom()
            .element_name(self.id)
            .is_some_and(|name| name.ns.as_ref() == "http://www.w3.org/1999/xhtml")
    }

    fn has_local_name(&self, local_name: &Atom) -> bool {
        self.dom().element_name(self.id).is_some_and(|name| {
            name.local
                .as_ref()
                .eq_ignore_ascii_case(local_name.as_str())
        })
    }

    fn has_namespace(&self, namespace: &Atom) -> bool {
        self.dom()
            .element_name(self.id)
            .is_some_and(|name| name.ns.as_ref() == namespace.as_str())
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.dom().element_name(self.id) == other.dom().element_name(other.id)
    }

    fn attr_matches(
        &self,
        namespace: &NamespaceConstraint<&Atom>,
        local_name: &Atom,
        operation: &AttrSelectorOperation<&livery::selector::AttributeValue>,
    ) -> bool {
        self.dom().attributes(self.id).any(|attribute| {
            let namespace_matches = match namespace {
                NamespaceConstraint::Any => true,
                NamespaceConstraint::Specific(namespace) => {
                    attribute.name.ns.as_ref() == namespace.as_str()
                },
            };
            namespace_matches
                && attribute.name.local.as_ref() == local_name.as_str()
                && operation.eval_str(attribute.value)
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo: &StatePseudoClass,
        _context: &mut MatchingContext<LiverySelectorImpl>,
    ) -> bool {
        self.tree.states.matches(self.id, *pseudo)
    }

    fn match_pseudo_element(
        &self,
        pseudo: &NoPseudoElement,
        _context: &mut MatchingContext<LiverySelectorImpl>,
    ) -> bool {
        match *pseudo {}
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        self.has_local_name(&Atom::from("a")) && self.attribute("", "href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        self.has_local_name(&Atom::from("slot"))
    }

    fn has_id(&self, id: &Atom, case_sensitivity: CaseSensitivity) -> bool {
        self.attribute("", "id")
            .is_some_and(|value| case_sensitivity.eq(value.as_bytes(), id.as_str().as_bytes()))
    }

    fn has_class(&self, class: &Atom, case_sensitivity: CaseSensitivity) -> bool {
        self.attribute("", "class").is_some_and(|classes| {
            classes
                .split_ascii_whitespace()
                .any(|value| case_sensitivity.eq(value.as_bytes(), class.as_str().as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &Atom) -> bool {
        false
    }

    fn imported_part(&self, _name: &Atom) -> Option<Atom> {
        None
    }

    fn is_part(&self, _name: &Atom) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.dom()
            .dom_children(self.id)
            .all(|id| match self.dom().kind(id) {
                NodeKind::Element => false,
                NodeKind::Text => self.dom().text(id).is_none_or(str::is_empty),
                _ => true,
            })
    }

    fn is_root(&self) -> bool {
        self.parent_element().is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}
