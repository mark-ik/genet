/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stylo trait impls for serval-layout.
//!
//! `StyleNodeRef<'a, D>` is the foreign-trait firewall — it wraps a
//! `(dom: &'a D, id: D::NodeId, style: &'a StylePlane<D::NodeId>)` tuple
//! and implements Stylo's trait family (`NodeInfo` / `TNode` / `TDocument` /
//! `TShadowRoot` / `TElement` / `selectors::Element` / `AttributeProvider`)
//! over those three pieces of state.
//!
//! Distinct from `NodeRef` in `adapter.rs`: `NodeRef` is structural-only
//! (used by `construct.rs`) and doesn't carry a `StylePlane` reference.
//! `StyleNodeRef` is the Stylo-bound variant constructed by the cascade
//! when it needs to walk the DOM with style-data access. Splitting keeps
//! the structural path cheap and avoids forcing `StylePlane` through
//! every NodeRef construction.
//!
//! ## Status (2026-05-18)
//!
//! Trait skeleton present; structural methods backed by `LayoutDom`;
//! cascade-time methods (animations, snapshots, mutate_data, etc.)
//! `unimplemented!()` with reasons. Cascade integration deferred — once
//! the cascade runs, the `unimplemented!()` bodies become the next
//! focused work.
//!
//! Architectural reference: Blitz's `packages/blitz-dom/src/stylo.rs`
//! is the closest prior-art impl (alternative DOM + Stylo direct, no
//! `layout_api` scaffolding). Our impls mirror its patterns, adapted to
//! the `(dom, id, style)` shape required by the planes architecture
//! where style state lives in serval-layout-owned planes rather than
//! embedded on DOM nodes.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

#![allow(unsafe_code, dead_code, unused_variables, clippy::needless_lifetimes)]

use std::fmt;
use std::hash::{Hash, Hasher};

use layout_dom_api::{LayoutDom, NodeKind};
use selectors::Element as SelectorsElement;
use selectors::OpaqueElement;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::matching::{ElementSelectorFlags, MatchingContext, QuirksMode, VisitedHandlingMode};
use selectors::sink::Push;
use servo_arc::{Arc, ArcBorrow};
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::context::SharedStyleContext;
use style::data::{ElementDataMut, ElementDataRef};
use style::dom::{
    AttributeProvider, LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode,
    TShadowRoot,
};
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::selector_parser::{
    AttrValue, Lang, NonTSPseudoClass, PseudoElement, RestyleDamage, SelectorImpl,
};
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::scope_rule::ImplicitScopeRoot;
use style::values::{AtomIdent, GenericAtomIdent};
// LocalName / Namespace come from markup5ever (the raw `Atom<…StaticSet>`
// types), not `style::LocalName` (which is `GenericAtomIdent<…>` — a
// different wrapper). The Stylo trait family expects the raw Atom shape.
// LayoutDom uses the same raw types via html5ever, which re-exports
// markup5ever's atoms.
use markup5ever::{LocalName, Namespace};
use stylo_dom::ElementState;

use crate::style::StylePlane;

// =============================================================================
// StyleNodeRef
// =============================================================================

/// A `LayoutDom`-backed handle + StylePlane reference. The Stylo-bound
/// variant of `NodeRef` — implements Stylo's trait family over the
/// `(dom, id, style)` tuple.
pub struct StyleNodeRef<'a, D: LayoutDom> {
    pub(crate) dom: &'a D,
    pub(crate) id: D::NodeId,
    pub(crate) style: &'a StylePlane<D::NodeId>,
}

impl<'a, D: LayoutDom> StyleNodeRef<'a, D> {
    pub fn new(dom: &'a D, id: D::NodeId, style: &'a StylePlane<D::NodeId>) -> Self {
        Self { dom, id, style }
    }

    pub fn document(dom: &'a D, style: &'a StylePlane<D::NodeId>) -> Self {
        Self {
            dom,
            id: dom.document(),
            style,
        }
    }

    fn with_id(&self, id: D::NodeId) -> Self {
        Self {
            dom: self.dom,
            id,
            style: self.style,
        }
    }

    /// Lookup the `StyleEntry` for this node, if cascade has populated it.
    fn entry(&self) -> Option<&'a crate::style::StyleEntry> {
        self.style.get(self.id)
    }

    fn is_element_kind(&self) -> bool {
        matches!(self.dom.kind(self.id), NodeKind::Element)
    }
}

impl<'a, D: LayoutDom> Clone for StyleNodeRef<'a, D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, D: LayoutDom> Copy for StyleNodeRef<'a, D> {}

impl<'a, D: LayoutDom> fmt::Debug for StyleNodeRef<'a, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyleNodeRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<'a, D: LayoutDom> PartialEq for StyleNodeRef<'a, D> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.dom, other.dom) && self.id == other.id
    }
}

impl<'a, D: LayoutDom> Eq for StyleNodeRef<'a, D> {}

impl<'a, D: LayoutDom> Hash for StyleNodeRef<'a, D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Identity is (dom pointer, id). Hash both.
        (self.dom as *const D as usize).hash(state);
        self.id.hash(state);
    }
}

// =============================================================================
// NodeInfo
// =============================================================================

impl<'a, D: LayoutDom> NodeInfo for StyleNodeRef<'a, D> {
    fn is_element(&self) -> bool {
        matches!(self.dom.kind(self.id), NodeKind::Element)
    }

    fn is_text_node(&self) -> bool {
        matches!(self.dom.kind(self.id), NodeKind::Text)
    }
}

// =============================================================================
// TDocument
// =============================================================================

impl<'a, D: LayoutDom> TDocument for StyleNodeRef<'a, D> {
    type ConcreteNode = StyleNodeRef<'a, D>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn is_html_document(&self) -> bool {
        // serval-static-dom only parses HTML; future DOM providers
        // may want to return false (e.g., reader-mode synthetic docs).
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        // LayoutDom doesn't currently expose quirks mode; defaults to
        // standards mode. Will be threaded through when needed.
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        unimplemented!(
            "TDocument::shared_lock — SharedRwLock not wired yet; \
             cascade integration will install one on serval-layout."
        )
    }
}

// =============================================================================
// TShadowRoot
// =============================================================================

impl<'a, D: LayoutDom> TShadowRoot for StyleNodeRef<'a, D> {
    type ConcreteNode = StyleNodeRef<'a, D>;

    fn as_node(&self) -> Self::ConcreteNode {
        // Shadow DOM not supported in static profile; this is unreachable
        // because nothing constructs a shadow root.
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        unimplemented!("Shadow DOM not supported in static profile")
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

// =============================================================================
// AttributeProvider
// =============================================================================

impl<'a, D: LayoutDom> AttributeProvider for StyleNodeRef<'a, D> {
    fn get_attr(
        &self,
        attr: &style::LocalName,
        namespace: &style::Namespace,
    ) -> Option<String> {
        // Stylo's AttributeProvider uses `GenericAtomIdent<*>` wrappers
        // (style::LocalName / style::Namespace), unwrap to the underlying
        // `Atom<*>` for LayoutDom.
        self.dom
            .attribute(self.id, &namespace.0, &attr.0)
            .map(|s| s.to_string())
    }
}

// =============================================================================
// TNode
// =============================================================================

impl<'a, D: LayoutDom> TNode for StyleNodeRef<'a, D> {
    type ConcreteElement = StyleNodeRef<'a, D>;
    type ConcreteDocument = StyleNodeRef<'a, D>;
    type ConcreteShadowRoot = StyleNodeRef<'a, D>;

    fn parent_node(&self) -> Option<Self> {
        self.dom.parent(self.id).map(|p| self.with_id(p))
    }

    fn first_child(&self) -> Option<Self> {
        self.dom
            .dom_children(self.id)
            .next()
            .map(|c| self.with_id(c))
    }

    fn last_child(&self) -> Option<Self> {
        self.dom
            .dom_children(self.id)
            .last()
            .map(|c| self.with_id(c))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.dom.prev_sibling(self.id).map(|s| self.with_id(s))
    }

    fn next_sibling(&self) -> Option<Self> {
        self.dom.next_sibling(self.id).map(|s| self.with_id(s))
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.with_id(self.dom.document())
    }

    fn is_in_document(&self) -> bool {
        // For LayoutDom-backed DOMs, every reachable node is in the
        // document. (Detached subtrees would need a different impl.)
        true
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node().and_then(|n| n.as_element())
    }

    fn opaque(&self) -> OpaqueNode {
        // Identity-keyed against the node id. The Stylo OpaqueNode is a
        // pointer-shaped value; we use a small allocation per node-id
        // for now. Cascade-time hot path; revisit if profiling shows it
        // matters. The proper fix is a stable per-node address from
        // LayoutDom (an `opaque(id) -> usize` primitive).
        unimplemented!(
            "TNode::opaque — needs stable per-node identity; cascade \
             not running yet"
        )
    }

    fn debug_id(self) -> usize {
        // Use the hash of (dom_ptr, id) as a debug-only identifier.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish() as usize
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        if self.is_element_kind() {
            Some(*self)
        } else {
            None
        }
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        if matches!(self.dom.kind(self.id), NodeKind::Document) {
            Some(*self)
        } else {
            None
        }
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        // Static profile: no shadow roots.
        None
    }
}

// =============================================================================
// selectors::Element
// =============================================================================

impl<'a, D: LayoutDom> SelectorsElement for StyleNodeRef<'a, D> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // Same caveat as TNode::opaque; see that method. Cascade not
        // running yet means selector matching isn't exercised.
        unimplemented!(
            "selectors::Element::opaque — needs stable per-node identity; \
             selector matching not running yet"
        )
    }

    fn parent_element(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        // Static profile doesn't synthesize pseudo elements yet.
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut cursor = self.dom.prev_sibling(self.id);
        while let Some(id) = cursor {
            let candidate = self.with_id(id);
            if candidate.is_element_kind() {
                return Some(candidate);
            }
            cursor = self.dom.prev_sibling(id);
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut cursor = self.dom.next_sibling(self.id);
        while let Some(id) = cursor {
            let candidate = self.with_id(id);
            if candidate.is_element_kind() {
                return Some(candidate);
            }
            cursor = self.dom.next_sibling(id);
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        self.dom
            .dom_children(self.id)
            .map(|c| self.with_id(c))
            .find(|n| n.is_element_kind())
    }

    fn is_html_element_in_html_document(&self) -> bool {
        // serval-static-dom is always an HTML document with HTML elements;
        // future DOMs may refine this.
        self.is_element_kind()
    }

    fn has_local_name(&self, local_name: &LocalName) -> bool {
        self.dom
            .element_name(self.id)
            .is_some_and(|q| q.local == *local_name)
    }

    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.dom
            .element_name(self.id)
            .is_some_and(|q| q.ns == *ns)
    }

    fn is_same_type(&self, other: &Self) -> bool {
        match (self.dom.element_name(self.id), other.dom.element_name(other.id)) {
            (Some(a), Some(b)) => a.local == b.local && a.ns == b.ns,
            _ => false,
        }
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&style::Namespace>,
        local_name: &style::LocalName,
        operation: &AttrSelectorOperation<&style::values::AtomString>,
    ) -> bool {
        // Lookup the attribute via LayoutDom (no-namespace match for now).
        // Per Blitz's impl: TODO filter by namespace.
        let _ = _ns;
        let _ = local_name;
        let _ = operation;
        // Real impl: walk attributes(), find matching local_name (filtered
        // by ns constraint), run operation.eval_str(&value). Probe-stage
        // skeleton returns false (no attribute selectors matched).
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        // Static profile: most non-TS pseudo-classes are false (no
        // interaction state, no JS-driven flags). The cascade may still
        // call this during selector matching; for the probe-stage skeleton
        // return false uniformly. Real impl reads `self.entry().map(|e|
        // e.state.contains(...))` for the interaction-state subset.
        let _ = pc;
        false
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        // Read-modify-write on the entry's selector_flags Cell.
        if let Some(entry) = self.entry() {
            let self_flags = flags.for_self();
            if !self_flags.is_empty() {
                entry
                    .selector_flags
                    .set(entry.selector_flags.get() | self_flags);
            }
            let parent_flags = flags.for_parent();
            if !parent_flags.is_empty() {
                if let Some(parent) = self.parent_node() {
                    if let Some(p_entry) = parent.entry() {
                        p_entry
                            .selector_flags
                            .set(p_entry.selector_flags.get() | parent_flags);
                    }
                }
            }
        }
    }

    fn is_link(&self) -> bool {
        // <a href="..."> and <area href="...">. Cascade-time check.
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(
        &self,
        _id: &AtomIdent,
        _case_sensitivity: CaseSensitivity,
    ) -> bool {
        // Real impl reads the interned id from StylePlane entry; the
        // cascade interns at first access. Probe doesn't exercise.
        unimplemented!("selectors::Element::has_id — atom interning not wired yet")
    }

    fn has_class(
        &self,
        _name: &AtomIdent,
        _case_sensitivity: CaseSensitivity,
    ) -> bool {
        unimplemented!("selectors::Element::has_class — atom interning not wired yet")
    }

    fn has_custom_state(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn imported_part(&self, _name: &AtomIdent) -> Option<AtomIdent> {
        None
    }

    fn is_part(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.dom.dom_children(self.id).next().is_none()
    }

    fn is_root(&self) -> bool {
        self.dom.parent(self.id).is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        // Bloom-filter contribution for the descendants-bloom optimization.
        // Real impl: hash the element's local name, classes, and id.
        // Returning false here means the optimization sees no contribution
        // for this element — selector matching still works, just slightly
        // less optimized.
        false
    }
}

// =============================================================================
// TElement
// =============================================================================

impl<'a, D: LayoutDom> TElement for StyleNodeRef<'a, D> {
    type ConcreteNode = StyleNodeRef<'a, D>;
    type TraversalChildrenIterator = TraversalChildren<'a, D>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(TraversalChildren {
            parent: *self,
            children: self.dom.dom_children(self.id).collect(),
            cursor: 0,
        })
    }

    fn is_html_element(&self) -> bool {
        self.is_element_kind()
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        // Inline `style="..."` declaration block. Parsed lazily on first
        // access; would live in StyleEntry alongside other cascade state.
        // Probe stage: none.
        None
    }

    fn animation_rule(
        &self,
        _ctx: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn transition_rule(
        &self,
        _ctx: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn state(&self) -> ElementState {
        self.entry().map(|e| e.state).unwrap_or_else(ElementState::empty)
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&style::Atom> {
        // Stylo expects atom-interned ids. Cascade-time interning belongs
        // in StylePlane; not wired yet.
        None
    }

    fn each_class<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        // Cascade-time iteration over class atoms. Not wired yet.
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        for attr in self.dom.attributes(self.id) {
            // Wrap the markup5ever atom in the GenericAtomIdent wrapper
            // Stylo expects. `style::LocalName` is the type alias.
            callback(&GenericAtomIdent(attr.name.local.clone()));
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        // Static profile: no incremental restyle.
        false
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {
        // No-op: snapshots not used in static profile.
    }

    unsafe fn set_dirty_descendants(&self) {
        // No-op for static profile.
    }

    unsafe fn unset_dirty_descendants(&self) {
        // No-op for static profile.
    }

    fn store_children_to_process(&self, _n: isize) {
        // No-op for sequential traversal.
    }

    fn did_process_child(&self) -> isize {
        0
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        unimplemented!(
            "TElement::ensure_data — StylePlane allocation not wired yet; \
             cascade integration installs ElementDataWrapper on first access."
        )
    }

    unsafe fn clear_data(&self) {
        // Clear the StylePlane's stylo_data slot for this node. Cascade-time.
    }

    fn has_data(&self) -> bool {
        self.entry()
            .map(|e| e.stylo_data.borrow().is_some())
            .unwrap_or(false)
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // The borrow is on the AtomicRefCell holding the Option<ElementDataWrapper>.
        // Returning the ElementDataRef requires extracting an `ElementDataRef`
        // out of the wrapper, which depends on Stylo internals not yet stable
        // here. Stub for now — cascade integration installs this.
        unimplemented!(
            "TElement::borrow_data — ElementDataWrapper accessor not wired yet"
        )
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        unimplemented!(
            "TElement::mutate_data — ElementDataWrapper accessor not wired yet"
        )
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _ctx: &SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(
        &self,
        _ctx: &SharedStyleContext,
        _pseudo: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn has_css_transitions(
        &self,
        _ctx: &SharedStyleContext,
        _pseudo: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn lang_attr(&self) -> Option<AttrValue> {
        None
    }

    fn match_element_lang(
        &self,
        _override_lang: Option<Option<AttrValue>>,
        _value: &Lang,
    ) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        // Real impl: this element is <body> AND its parent is the document
        // root (<html>). Cascade exercises this for the body-style cascade
        // root special case. Probe-stage: false uniformly.
        false
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
        // HTML legacy attribute hints (align, width, height, bgcolor,
        // hidden, etc.). Blitz has a ~150-line impl; ours stays empty
        // until we want real legacy-attribute support. Static profile
        // renders modern HTML where legacy attrs are rare.
    }

    fn local_name(&self) -> &LocalName {
        // The borrow is into the DOM's stored QualName. element_name
        // returns &QualName which contains the LocalName.
        unimplemented!(
            "TElement::local_name — borrow plumbing through &QualName not yet wired; \
             cascade integration will work this out"
        )
    }

    fn namespace(&self) -> &Namespace {
        unimplemented!(
            "TElement::namespace — borrow plumbing through &QualName not yet wired"
        )
    }

    fn query_container_size(
        &self,
        _display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        // Container queries: not exercised at probe stage.
        Default::default()
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.entry()
            .map(|e| e.selector_flags.get().contains(flags))
            .unwrap_or(false)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        self.entry()
            .map(|e| {
                let f = e.selector_flags.get();
                if f.contains(
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING,
                ) {
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING
                } else if f.contains(
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR,
                ) {
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR
                } else if f.contains(
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING,
                ) {
                    ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING
                } else {
                    ElementSelectorFlags::empty()
                }
            })
            .unwrap_or_else(ElementSelectorFlags::empty)
    }

    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        None
    }

    fn compute_layout_damage(_old: &ComputedValues, _new: &ComputedValues) -> RestyleDamage {
        // Damage computation drives incremental relayout; for static
        // profile (no incremental), the default no-damage is fine.
        Default::default()
    }
}

// =============================================================================
// Traversal iterator
// =============================================================================

/// Iterator over a node's children in DOM order. Used by
/// `TElement::traversal_children`. Materializes the child id list eagerly
/// because Stylo's `LayoutIterator<T>` expects a sized iterator type.
pub struct TraversalChildren<'a, D: LayoutDom> {
    parent: StyleNodeRef<'a, D>,
    children: Vec<D::NodeId>,
    cursor: usize,
}

impl<'a, D: LayoutDom> Iterator for TraversalChildren<'a, D> {
    type Item = StyleNodeRef<'a, D>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = *self.children.get(self.cursor)?;
        self.cursor += 1;
        Some(self.parent.with_id(id))
    }
}
