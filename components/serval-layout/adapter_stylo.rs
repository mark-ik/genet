/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stylo trait impls for serval-layout.
//!
//! `StyleNodeRef<'a, D>` is the foreign-trait firewall — it implements
//! Stylo's trait family (`NodeInfo` / `TNode` / `TDocument` / `TShadowRoot` /
//! `TElement` / `selectors::Element` / `AttributeProvider`) over the
//! `(dom, id, plane)` triple required by the planes architecture.
//!
//! **Size constraint**: Stylo's style-sharing cache is a thread-local byte
//! buffer sized for a pointer-shaped element type (`FakeCandidate {
//! _element: usize, … }` in upstream `style/sharing/mod.rs`). Blitz
//! satisfies this with `type BlitzNode<'a> = &'a Node` — they embed style
//! state on each `Node`. We keep the planes split (style state lives in
//! `StylePlane`, not on DOM nodes), so to match the 8-byte assumption
//! `StyleNodeRef` carries only `D::NodeId` and stashes `(dom, plane)` in
//! TLS for the cascade duration via [`CascadeGuard`]. Methods that need
//! `dom`/`plane` access read from the TLS slot.
//!
//! **Invariant**: At most one cascade per thread at a time. `CascadeGuard`
//! enforces single-active-context via stack-saved `prev` (nested guards
//! restore the outer ctx on drop).
//!
//! Distinct from `NodeRef` in `adapter.rs`: `NodeRef` is structural-only
//! (used by `construct.rs`) and doesn't need TLS. `StyleNodeRef` is the
//! Stylo-bound variant — only valid inside a `CascadeGuard::enter` scope.
//!
//! Architectural reference: Blitz's `packages/blitz-dom/src/stylo.rs` is
//! the closest prior-art impl. Our impls mirror its patterns, adapted
//! to the TLS-context shape.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

#![allow(unsafe_code, dead_code, unused_variables, clippy::needless_lifetimes)]

use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

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
    AttrValue, Lang, NonTSPseudoClass, PseudoElement, RestyleDamage, SelectorImpl, SnapshotMap,
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
// Cascade thread-local context
// =============================================================================

/// Type-erased pointers to the active cascade's dom + plane + shared
/// lock. Set by [`CascadeGuard::enter`], cleared on drop. Single slot
/// per thread.
#[derive(Copy, Clone)]
struct CascadeCtx {
    dom: *const u8,
    plane: *const u8,
    /// SharedRwLock pointer — Stylo needs it for stylesheet rule
    /// access via `TDocument::shared_lock`. Outside cascade calls,
    /// the trait method panics (no active context); during cascade,
    /// it returns this borrowed reference.
    shared_lock: *const u8,
    /// `SnapshotMap` pointer for incremental restyle — null for a full
    /// cascade (no snapshots). When present, `has_snapshot` queries it.
    snapshot_map: *const u8,
}

thread_local! {
    static CASCADE_CTX: Cell<Option<CascadeCtx>> = const { Cell::new(None) };
}

/// RAII guard that installs `(dom, plane)` pointers in TLS for the duration
/// of a cascade traversal. `StyleNodeRef::dom()` / `plane()` resolve through
/// these pointers; outside a guard, those calls panic.
///
/// The `'a` parameter ties the guard's lifetime to the borrowed dom +
/// plane references. Nested guards are supported — drop restores the
/// previous context.
pub struct CascadeGuard<'a, D: LayoutDom> {
    prev: Option<CascadeCtx>,
    _phantom: PhantomData<(
        &'a D,
        &'a StylePlane<D::NodeId>,
        &'a SharedRwLock,
        &'a SnapshotMap,
    )>,
}

impl<'a, D: LayoutDom> CascadeGuard<'a, D> {
    /// Enter a cascade context. Pointers to `dom`, `plane`, the
    /// `SharedRwLock`, and an optional `SnapshotMap` are stashed in TLS
    /// until this guard drops. Pass `None` for `snapshot_map` on a full
    /// cascade (no incremental snapshots); the incremental restyle path
    /// passes `Some`.
    ///
    /// Asserts `D::NodeId` is pointer-shaped (size + align match `usize`),
    /// the condition Stylo's style-sharing cache enforces at runtime.
    /// Hitting this assertion means the caller is trying to use a DOM
    /// whose `NodeId` type doesn't fit Stylo's typeless TLS cache layout.
    pub fn enter(
        dom: &'a D,
        plane: &'a StylePlane<D::NodeId>,
        shared_lock: &'a SharedRwLock,
        snapshot_map: Option<&'a SnapshotMap>,
    ) -> Self {
        assert_eq!(
            std::mem::size_of::<D::NodeId>(),
            std::mem::size_of::<usize>(),
            "D::NodeId must be pointer-sized for Stylo style-sharing cache",
        );
        assert_eq!(
            std::mem::align_of::<D::NodeId>(),
            std::mem::align_of::<usize>(),
            "D::NodeId must have pointer alignment for Stylo style-sharing cache",
        );
        let new = CascadeCtx {
            dom: dom as *const D as *const u8,
            plane: plane as *const StylePlane<D::NodeId> as *const u8,
            shared_lock: shared_lock as *const SharedRwLock as *const u8,
            snapshot_map: snapshot_map
                .map_or(std::ptr::null(), |m| m as *const SnapshotMap as *const u8),
        };
        let prev = CASCADE_CTX.with(|c| c.replace(Some(new)));
        Self {
            prev,
            _phantom: PhantomData,
        }
    }
}

impl<'a, D: LayoutDom> Drop for CascadeGuard<'a, D> {
    fn drop(&mut self) {
        CASCADE_CTX.with(|c| c.set(self.prev));
    }
}

/// Read the active TLS cascade context. Panics if no guard is active.
fn cascade_ctx() -> CascadeCtx {
    CASCADE_CTX.with(|c| {
        c.get()
            .expect("StyleNodeRef accessed outside CascadeGuard scope")
    })
}

// =============================================================================
// StyleNodeRef
// =============================================================================

/// A Stylo-bound DOM handle. Carries only `D::NodeId`; `dom`/`plane`
/// access goes through TLS (set by [`CascadeGuard`]). Size matches
/// `usize`, which is the shape Stylo's style-sharing cache assumes.
///
/// **Lifetime**: `'a` is a marker representing the lifetime of the active
/// [`CascadeGuard`]; the borrow checker can't enforce TLS validity, so the
/// caller must ensure all `StyleNodeRef<'a, D>` instances are dropped
/// before the guard.
pub struct StyleNodeRef<'a, D: LayoutDom> {
    pub(crate) id: D::NodeId,
    _phantom: PhantomData<&'a (D, StylePlane<D::NodeId>)>,
}

impl<'a, D: LayoutDom> StyleNodeRef<'a, D> {
    /// Construct a `StyleNodeRef` for the given node id. Must be called
    /// within a [`CascadeGuard::enter`] scope; methods on the returned
    /// ref read `(dom, plane)` from TLS.
    pub fn new(id: D::NodeId) -> Self {
        Self {
            id,
            _phantom: PhantomData,
        }
    }

    /// Build a `StyleNodeRef` for the document root. Requires an active
    /// `CascadeGuard`.
    pub fn document_root() -> Self {
        let dom: &'a D = Self::dom_from_ctx();
        Self::new(dom.document())
    }

    fn with_id(&self, id: D::NodeId) -> Self {
        Self::new(id)
    }

    /// Resolve `&'a D` from TLS. SAFETY: only valid inside a guard scope.
    fn dom_from_ctx() -> &'a D {
        // SAFETY: CascadeGuard::enter stored a `*const D` here; the
        // 'a lifetime is the guard's lifetime, which the caller is
        // responsible for not outliving.
        unsafe { &*(cascade_ctx().dom as *const D) }
    }

    /// Resolve `&'a StylePlane<D::NodeId>` from TLS.
    fn plane_from_ctx() -> &'a StylePlane<D::NodeId> {
        // SAFETY: see dom_from_ctx.
        unsafe { &*(cascade_ctx().plane as *const StylePlane<D::NodeId>) }
    }

    /// Resolve `&'a SharedRwLock` from TLS.
    fn shared_lock_from_ctx() -> &'a SharedRwLock {
        // SAFETY: see dom_from_ctx.
        unsafe { &*(cascade_ctx().shared_lock as *const SharedRwLock) }
    }

    /// Resolve the active `SnapshotMap` from TLS, if the cascade was
    /// entered with one (incremental restyle). `None` for a full cascade.
    fn snapshot_map_from_ctx() -> Option<&'a SnapshotMap> {
        let ptr = cascade_ctx().snapshot_map;
        if ptr.is_null() {
            None
        } else {
            // SAFETY: see dom_from_ctx; the pointer was stored from a
            // `&'a SnapshotMap` that outlives the guard.
            Some(unsafe { &*(ptr as *const SnapshotMap) })
        }
    }

    pub(crate) fn dom(&self) -> &'a D {
        Self::dom_from_ctx()
    }

    pub(crate) fn plane(&self) -> &'a StylePlane<D::NodeId> {
        Self::plane_from_ctx()
    }

    /// Lookup the `StyleEntry` for this node, if cascade has populated it.
    fn entry(&self) -> Option<&'a crate::style::StyleEntry> {
        self.plane().get(self.id)
    }

    fn is_element_kind(&self) -> bool {
        matches!(self.dom().kind(self.id), NodeKind::Element)
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
        // Within a cascade, all StyleNodeRefs share the same TLS context;
        // identity is the node id alone.
        self.id == other.id
    }
}

impl<'a, D: LayoutDom> Eq for StyleNodeRef<'a, D> {}

impl<'a, D: LayoutDom> Hash for StyleNodeRef<'a, D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

// =============================================================================
// NodeInfo
// =============================================================================

impl<'a, D: LayoutDom> NodeInfo for StyleNodeRef<'a, D> {
    fn is_element(&self) -> bool {
        matches!(self.dom().kind(self.id), NodeKind::Element)
    }

    fn is_text_node(&self) -> bool {
        matches!(self.dom().kind(self.id), NodeKind::Text)
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
        // Resolved through TLS — only valid inside a CascadeGuard scope,
        // which is the only context where Stylo invokes this anyway.
        Self::shared_lock_from_ctx()
    }
}

// =============================================================================
// TShadowRoot
// =============================================================================

impl<'a, D: LayoutDom> TShadowRoot for StyleNodeRef<'a, D> {
    type ConcreteNode = StyleNodeRef<'a, D>;

    fn as_node(&self) -> Self::ConcreteNode {
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
        self.dom()
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
        self.dom().parent(self.id).map(|p| self.with_id(p))
    }

    fn first_child(&self) -> Option<Self> {
        self.dom()
            .dom_children(self.id)
            .next()
            .map(|c| self.with_id(c))
    }

    fn last_child(&self) -> Option<Self> {
        self.dom()
            .dom_children(self.id)
            .last()
            .map(|c| self.with_id(c))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.dom().prev_sibling(self.id).map(|s| self.with_id(s))
    }

    fn next_sibling(&self) -> Option<Self> {
        self.dom().next_sibling(self.id).map(|s| self.with_id(s))
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.with_id(self.dom().document())
    }

    fn is_in_document(&self) -> bool {
        true
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node().and_then(|n| n.as_element())
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.dom().opaque_id(self.id) as usize)
    }

    fn debug_id(self) -> usize {
        self.dom().opaque_id(self.id) as usize
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        if self.is_element_kind() {
            Some(*self)
        } else {
            None
        }
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        if matches!(self.dom().kind(self.id), NodeKind::Document) {
            Some(*self)
        } else {
            None
        }
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }
}

// =============================================================================
// selectors::Element
// =============================================================================

impl<'a, D: LayoutDom> SelectorsElement for StyleNodeRef<'a, D> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // Stable per-node identity via LayoutDom::opaque_id. Stored as a
        // fake `NonNull<()>` (matching Blitz's pattern). The `+1` ensures
        // non-null even for `opaque_id == 0`.
        let raw = self.dom().opaque_id(self.id).wrapping_add(1) as usize;
        let ptr = std::ptr::NonNull::new(raw as *mut ())
            .expect("opaque_id + 1 cannot be zero");
        OpaqueElement::from_non_null_ptr(ptr)
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
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut cursor = self.dom().prev_sibling(self.id);
        while let Some(id) = cursor {
            let candidate = self.with_id(id);
            if candidate.is_element_kind() {
                return Some(candidate);
            }
            cursor = self.dom().prev_sibling(id);
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut cursor = self.dom().next_sibling(self.id);
        while let Some(id) = cursor {
            let candidate = self.with_id(id);
            if candidate.is_element_kind() {
                return Some(candidate);
            }
            cursor = self.dom().next_sibling(id);
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        self.dom()
            .dom_children(self.id)
            .map(|c| self.with_id(c))
            .find(|n| n.is_element_kind())
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.is_element_kind()
    }

    fn has_local_name(&self, local_name: &LocalName) -> bool {
        self.dom()
            .element_name(self.id)
            .is_some_and(|q| q.local == *local_name)
    }

    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.dom()
            .element_name(self.id)
            .is_some_and(|q| q.ns == *ns)
    }

    fn is_same_type(&self, other: &Self) -> bool {
        match (self.dom().element_name(self.id), other.dom().element_name(other.id)) {
            (Some(a), Some(b)) => a.local == b.local && a.ns == b.ns,
            _ => false,
        }
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&style::Namespace>,
        local_name: &style::LocalName,
        operation: &AttrSelectorOperation<&style::values::AtomString>,
    ) -> bool {
        // Find the attribute matching `local_name` under the namespace
        // constraint, then run the operation (`[attr]` exists, `[attr=v]`,
        // `[attr~=v]`, …) against its value. `style::{LocalName,Namespace}`
        // are `GenericAtomIdent` wrappers over the raw markup5ever atoms
        // `LayoutDom` stores, hence the `.0`.
        let dom = self.dom();
        match ns {
            NamespaceConstraint::Specific(ns) => dom
                .attribute(self.id, &ns.0, &local_name.0)
                .is_some_and(|value| operation.eval_str(value)),
            NamespaceConstraint::Any => dom.attributes(self.id).any(|attr| {
                attr.name.local == local_name.0 && operation.eval_str(attr.value)
            }),
        }
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
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
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        use style::CaseSensitivityExt;
        let no_ns = Namespace::default();
        let id_local = LocalName::from("id");
        let Some(id_attr) = self.dom().attribute(self.id, &no_ns, &id_local) else {
            return false;
        };
        let atom = style::Atom::from(id_attr);
        case_sensitivity.eq_atom(&atom, id)
    }

    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        use style::CaseSensitivityExt;
        let no_ns = Namespace::default();
        let class_local = LocalName::from("class");
        let Some(class_attr) = self.dom().attribute(self.id, &no_ns, &class_local) else {
            return false;
        };
        for token in class_attr.split_ascii_whitespace() {
            let atom = style::Atom::from(token);
            if case_sensitivity.eq_atom(&atom, name) {
                return true;
            }
        }
        false
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
        self.dom().dom_children(self.id).next().is_none()
    }

    fn is_root(&self) -> bool {
        self.dom().parent(self.id).is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
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
            children: self.dom().dom_children(self.id).collect(),
            cursor: 0,
            _phantom: PhantomData,
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
        // Atom-interned by `StylePlane::populate_for_elements` before
        // the cascade runs; returns a borrow rooted in the StylePlane.
        self.entry().and_then(|e| e.id_atom.as_ref())
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        let no_ns = Namespace::default();
        let class_local = LocalName::from("class");
        let Some(class_attr) = self.dom().attribute(self.id, &no_ns, &class_local) else {
            return;
        };
        for token in class_attr.split_ascii_whitespace() {
            let atom = style::Atom::from(token);
            callback(AtomIdent::cast(&atom));
        }
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
        for attr in self.dom().attributes(self.id) {
            callback(&GenericAtomIdent(attr.name.local.clone()));
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        self.entry().is_some_and(|e| e.dirty_descendants.get())
    }

    fn has_snapshot(&self) -> bool {
        // A snapshot exists for this element iff the active incremental
        // restyle captured one. Full cascade: no SnapshotMap → false.
        Self::snapshot_map_from_ctx().is_some_and(|m| m.get(self).is_some())
    }

    fn handled_snapshot(&self) -> bool {
        // No snapshot ⇒ nothing to handle ⇒ treated as handled (matches
        // the prior stub's `true` for the full-cascade path).
        if !self.has_snapshot() {
            return true;
        }
        self.entry().is_some_and(|e| e.handled_snapshot.get())
    }

    unsafe fn set_handled_snapshot(&self) {
        if let Some(entry) = self.entry() {
            entry.handled_snapshot.set(true);
        }
    }

    unsafe fn set_dirty_descendants(&self) {
        if let Some(entry) = self.entry() {
            entry.dirty_descendants.set(true);
        }
    }

    unsafe fn unset_dirty_descendants(&self) {
        if let Some(entry) = self.entry() {
            entry.dirty_descendants.set(false);
        }
    }

    fn store_children_to_process(&self, _n: isize) {}

    fn did_process_child(&self) -> isize {
        0
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // The StylePlane is `&'a StylePlane`, so we can't allocate a new
        // entry on the fly without &mut access. The cascade orchestrator
        // pre-populates entries via `StylePlane::populate_for_elements`
        // before calling the cascade. If the entry is missing, that's a
        // cascade-orchestration bug.
        let entry = self
            .entry()
            .expect("ensure_data: StylePlane entry must exist; cascade should pre-populate");
        // SAFETY: Stylo cascade guarantees exclusive access per node.
        unsafe { entry.ensure_data() }
    }

    unsafe fn clear_data(&self) {
        if let Some(entry) = self.entry() {
            // SAFETY: same cascade-exclusive-access invariant.
            unsafe { entry.clear_data() }
        }
    }

    fn has_data(&self) -> bool {
        self.entry().is_some_and(|e| e.has_data())
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.entry().and_then(|e| e.borrow_data())
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: Stylo cascade has exclusive access per node during
        // traversal. Callers outside the cascade must not call this.
        self.entry().and_then(|e| unsafe { e.mutate_data() })
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
        false
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
    }

    fn local_name(&self) -> &LocalName {
        &self
            .dom()
            .element_name(self.id)
            .expect("local_name called on non-element node")
            .local
    }

    fn namespace(&self) -> &Namespace {
        &self
            .dom()
            .element_name(self.id)
            .expect("namespace called on non-element node")
            .ns
    }

    fn query_container_size(
        &self,
        _display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
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
    children: Vec<D::NodeId>,
    cursor: usize,
    _phantom: PhantomData<&'a D>,
}

impl<'a, D: LayoutDom> Iterator for TraversalChildren<'a, D> {
    type Item = StyleNodeRef<'a, D>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = *self.children.get(self.cursor)?;
        self.cursor += 1;
        Some(StyleNodeRef::new(id))
    }
}
