/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! **In-progress draft, not yet wired into `lib.rs`.**
//!
//! First-pass attempt at the Stylo trait impls (NodeInfo / TNode /
//! TDocument / TShadowRoot / TElement / selectors::Element /
//! AttributeProvider) for `LayoutDomAdapter`. The trait signatures here
//! are partly wrong — written from memory + incomplete trait reading
//! rather than side-by-side with the script-side reference impls.
//!
//! ## Next-session strategy
//!
//! 1. Read `components/script/layout_dom/servo_dangerous_style_node.rs`
//!    (151 lines) and `servo_dangerous_style_element.rs` (933 lines) in
//!    full. These are the canonical reference for what TElement /
//!    TNode actually demand, including the exact signatures Rust will
//!    accept for things like `BorrowedLocalName`,
//!    `<Self::ConcreteNode as TNode>::ConcreteShadowRoot`,
//!    `ElementDataRef<'_>` (vs. style::data::AtomicRef), etc.
//! 2. Adapt each method body to use `LayoutDom` primitives where the
//!    operation is structural; use `unimplemented!()` /
//!    `unreachable!()` only where the static profile genuinely doesn't
//!    exercise it (paint worklets, atom-interned id/class, restyle
//!    dirty bits, etc.).
//! 3. Add `Hash` and `AttributeProvider` impls in adapter.rs (they're
//!    TElement super-traits but unrelated to the Stylo trait surface).
//! 4. Once cargo check is green on this file alone, mod-declare in
//!    lib.rs and add a smoke test that round-trips a simple selector
//!    match through `selectors::Element`.
//!
//! ## What's wrong in this first pass
//!
//! - Made-up methods that don't exist on TElement: `primary_box_size`,
//!   `each_link_in_parent_implicit_scope`, `parent_element_with_filter`.
//! - Missing method: `has_selector_flags`.
//! - Wrong return type on `id` (should be `Option<&WeakAtom>` not
//!   `Option<&AtomIdent>`).
//! - Wrong `ensure_data` / `borrow_data` / `mutate_data` return types
//!   (should be `ElementDataMut<'_>` / `ElementDataRef<'_>`).
//! - `dom::ElementState` should be `stylo_dom::ElementState`.
//! - `ElementSelectorFlags` lives in `selectors::matching`, not
//!   `style::dom`.
//! - TElement's `shadow_root()` returns
//!   `Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot>`, not
//!   `Self::ConcreteShadowRoot`.
//! - Missing `AttributeProvider` and `Hash` impls (TElement super-
//!   traits).
//!
//! Below is preserved as a structural starting point — the *shape* of
//! the file (which trait impls go where, the method-name-to-LayoutDom
//! mapping for structural methods) is mostly right even where the
//! signatures aren't.

#![allow(dead_code, unused_imports)]

//! `style::dom::*` + `selectors::Element` trait impls for
//! [`LayoutDomAdapter`].
//!
//! Most methods are `unimplemented!()` stubs. The static profile does not
//! currently run Stylo's cascade or selector matching — these impls exist to
//! satisfy the `layout_api::DangerousStyleNode<'dom>` /
//! `DangerousStyleElement<'dom>` trait bounds so that `LayoutDomTypeBundle`
//! wiring becomes possible.
//!
//! As Stylo integration lights up phase-by-phase, the stubs get replaced
//! with real implementations that read from the `StyleStorage` + `AtomStorage`
//! side-tables the adapter will eventually carry (see Stylo paper-probe
//! findings in `docs/2026-05-16_layout_dom_api_design.md`).

#![allow(unsafe_code)]

use std::iter;

use layout_dom_api::LayoutDom;
use selectors::Element as SelectorsElement;
use selectors::OpaqueElement;
use selectors::attr::{
    AttrSelectorOperation, CaseSensitivity, NamespaceConstraint,
};
use selectors::matching::{ElementSelectorFlags, MatchingContext};
use style::dom::{
    LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot,
};
use style::selector_parser::SelectorImpl;

use crate::adapter::LayoutDomAdapter;

// -- NodeInfo --------------------------------------------------------------

impl<'a, D: LayoutDom> NodeInfo for LayoutDomAdapter<'a, D> {
    fn is_element(&self) -> bool {
        matches!(self.dom.kind(self.id), layout_dom_api::NodeKind::Element)
    }

    fn is_text_node(&self) -> bool {
        matches!(self.dom.kind(self.id), layout_dom_api::NodeKind::Text)
    }
}

// -- TNode -----------------------------------------------------------------

impl<'a, D: LayoutDom> TNode for LayoutDomAdapter<'a, D> {
    type ConcreteElement = Self;
    type ConcreteDocument = Self;
    type ConcreteShadowRoot = Self;

    fn parent_node(&self) -> Option<Self> {
        self.dom.parent(self.id).map(|pid| self.with_id(pid))
    }

    fn first_child(&self) -> Option<Self> {
        self.dom.dom_children(self.id).next().map(|cid| self.with_id(cid))
    }

    fn last_child(&self) -> Option<Self> {
        self.dom.dom_children(self.id).last().map(|cid| self.with_id(cid))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.dom.prev_sibling(self.id).map(|s| self.with_id(s))
    }

    fn next_sibling(&self) -> Option<Self> {
        self.dom.next_sibling(self.id).map(|s| self.with_id(s))
    }

    fn owner_doc(&self) -> Self {
        self.with_id(self.dom.document())
    }

    fn is_in_document(&self) -> bool {
        // Static profile: every reachable node is in the document.
        true
    }

    fn traversal_parent(&self) -> Option<Self> {
        // For static profile, traversal parent == DOM parent (no shadow DOM).
        self.parent_node().and_then(|p| {
            if matches!(p.dom.kind(p.id), layout_dom_api::NodeKind::Element) {
                Some(p)
            } else {
                None
            }
        })
    }

    fn opaque(&self) -> OpaqueNode {
        // Static profile doesn't run Stylo's cascade yet, so identity-keyed
        // selector matching isn't exercised. When Stylo lights up, this needs
        // a stable per-node identity — likely a `LayoutDom::opaque(id) -> u64`
        // primitive on the trait, or a per-DOM offset table.
        unimplemented!(
            "LayoutDomAdapter::opaque() — stable per-node identity not wired \
             yet; Stylo cascade is not exercised in the static profile"
        )
    }

    fn debug_id(self) -> usize {
        // Same caveat as opaque(). Useful only inside Stylo debug output.
        0
    }

    fn as_element(&self) -> Option<Self> {
        if self.is_element() { Some(*self) } else { None }
    }

    fn as_document(&self) -> Option<Self> {
        if matches!(self.dom.kind(self.id), layout_dom_api::NodeKind::Document) {
            Some(*self)
        } else {
            None
        }
    }

    fn as_shadow_root(&self) -> Option<Self> {
        // Static profile has no shadow DOM.
        None
    }
}

// -- TDocument -------------------------------------------------------------

impl<'a, D: LayoutDom> TDocument for LayoutDomAdapter<'a, D> {
    type ConcreteNode = Self;

    fn as_node(&self) -> Self {
        *self
    }

    fn is_html_document(&self) -> bool {
        // serval-static-dom only parses HTML for now.
        true
    }

    fn quirks_mode(&self) -> selectors::matching::QuirksMode {
        // Static profile defaults to standards mode; LayoutDom doesn't
        // currently expose the parsed quirks mode.
        selectors::matching::QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &style::shared_lock::SharedRwLock {
        // Static profile doesn't run Stylo; this is wired when the cascade
        // lights up. The lock would live in a serval-layout-owned side table.
        unimplemented!(
            "LayoutDomAdapter::shared_lock() — Stylo SharedRwLock not wired \
             yet; static profile doesn't run the cascade"
        )
    }
}

// -- TShadowRoot -----------------------------------------------------------

impl<'a, D: LayoutDom> TShadowRoot for LayoutDomAdapter<'a, D> {
    type ConcreteNode = Self;

    fn as_node(&self) -> Self {
        // Static profile has no shadow roots; calling `as_node()` on a
        // would-be shadow root is itself unreachable.
        unreachable!("static profile has no shadow roots")
    }

    fn host(&self) -> Self {
        unreachable!("static profile has no shadow roots")
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

// -- selectors::Element ---------------------------------------------------

impl<'a, D: LayoutDom> SelectorsElement for LayoutDomAdapter<'a, D> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        unimplemented!("LayoutDomAdapter::opaque() — see TNode::opaque comment")
    }

    fn parent_element(&self) -> Option<Self> {
        self.dom
            .parent(self.id)
            .map(|p| self.with_id(p))
            .filter(|p| matches!(p.dom.kind(p.id), layout_dom_api::NodeKind::Element))
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
        let mut cursor = self.dom.prev_sibling(self.id);
        while let Some(id) = cursor {
            let candidate = self.with_id(id);
            if matches!(candidate.dom.kind(candidate.id), layout_dom_api::NodeKind::Element) {
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
            if matches!(candidate.dom.kind(candidate.id), layout_dom_api::NodeKind::Element) {
                return Some(candidate);
            }
            cursor = self.dom.next_sibling(id);
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        self.dom
            .dom_children(self.id)
            .map(|id| self.with_id(id))
            .find(|n| matches!(n.dom.kind(n.id), layout_dom_api::NodeKind::Element))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        // Static profile is always HTML-in-HTML.
        self.is_element()
    }

    fn has_local_name(
        &self,
        _local_name: &<Self::Impl as selectors::SelectorImpl>::BorrowedLocalName,
    ) -> bool {
        unimplemented!("selector matching not exercised in the static profile")
    }

    fn has_namespace(
        &self,
        _ns: &<Self::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        unimplemented!("selector matching not exercised in the static profile")
    }

    fn is_same_type(&self, _other: &Self) -> bool {
        unimplemented!()
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&<Self::Impl as selectors::SelectorImpl>::NamespaceUrl>,
        _local_name: &<Self::Impl as selectors::SelectorImpl>::LocalName,
        _operation: &AttrSelectorOperation<&<Self::Impl as selectors::SelectorImpl>::AttrValue>,
    ) -> bool {
        unimplemented!()
    }

    fn match_non_ts_pseudo_class(
        &self,
        _pc: &<Self::Impl as selectors::SelectorImpl>::NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        unimplemented!()
    }

    fn match_pseudo_element(
        &self,
        _pe: &<Self::Impl as selectors::SelectorImpl>::PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        unimplemented!()
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // Static profile: no incremental restyle, no flags to track.
    }

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(
        &self,
        _id: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        _case_sensitivity: CaseSensitivity,
    ) -> bool {
        unimplemented!()
    }

    fn has_class(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        _case_sensitivity: CaseSensitivity,
    ) -> bool {
        unimplemented!()
    }

    fn has_custom_state(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> bool {
        false
    }

    fn imported_part(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> Option<<Self::Impl as selectors::SelectorImpl>::Identifier> {
        None
    }

    fn is_part(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.dom.dom_children(self.id).next().is_none()
    }

    fn is_root(&self) -> bool {
        self.dom.parent(self.id).is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut selectors::bloom::BloomFilter) -> bool {
        false
    }
}

// -- TElement (mostly stubs) ----------------------------------------------

impl<'a, D: LayoutDom> TElement for LayoutDomAdapter<'a, D> {
    type ConcreteNode = Self;
    type TraversalChildrenIterator = iter::Empty<Self>;

    fn as_node(&self) -> Self {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        // Static profile doesn't run Stylo traversal yet.
        LayoutIterator(iter::empty())
    }

    fn is_html_element(&self) -> bool {
        self.is_element()
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(
        &self,
    ) -> Option<servo_arc::ArcBorrow<style::shared_lock::Locked<style::properties::PropertyDeclarationBlock>>>
    {
        None
    }

    fn animation_rule(
        &self,
        _: &style::context::SharedStyleContext,
    ) -> Option<servo_arc::Arc<style::shared_lock::Locked<style::properties::PropertyDeclarationBlock>>>
    {
        None
    }

    fn transition_rule(
        &self,
        _: &style::context::SharedStyleContext,
    ) -> Option<servo_arc::Arc<style::shared_lock::Locked<style::properties::PropertyDeclarationBlock>>>
    {
        None
    }

    fn state(&self) -> dom::ElementState {
        dom::ElementState::empty()
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&style::values::AtomIdent> {
        None
    }

    fn each_class<F>(&self, _callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&style::values::AtomIdent),
    {
    }

    fn each_attr_name<F>(&self, _callback: F)
    where
        F: FnMut(&style::LocalName),
    {
    }

    fn has_dirty_descendants(&self) -> bool {
        false
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {}

    unsafe fn set_dirty_descendants(&self) {}

    unsafe fn unset_dirty_descendants(&self) {}

    fn store_children_to_process(&self, _n: isize) {}

    fn did_process_child(&self) -> isize {
        0
    }

    unsafe fn ensure_data(&self) -> style::data::AtomicRefMut<style::data::ElementData> {
        unimplemented!(
            "LayoutDomAdapter::ensure_data — element data side table not wired"
        )
    }

    unsafe fn clear_data(&self) {}

    fn has_data(&self) -> bool {
        false
    }

    fn borrow_data(&self) -> Option<style::data::AtomicRef<style::data::ElementData>> {
        None
    }

    fn mutate_data(&self) -> Option<style::data::AtomicRefMut<style::data::ElementData>> {
        None
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _: &style::context::SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(
        &self,
        _: &style::context::SharedStyleContext,
        _: Option<style::selector_parser::PseudoElement>,
    ) -> bool {
        false
    }

    fn has_css_transitions(
        &self,
        _: &style::context::SharedStyleContext,
        _: Option<style::selector_parser::PseudoElement>,
    ) -> bool {
        false
    }

    fn shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        None
    }

    fn match_element_lang(
        &self,
        _: Option<style::selector_parser::Lang>,
        _: &style::selector_parser::Lang,
    ) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        false
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: selectors::matching::VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: selectors::sink::Push<style::applicable_declarations::ApplicableDeclarationBlock>,
    {
    }

    fn local_name(&self) -> &style::LocalName {
        unimplemented!(
            "LayoutDomAdapter::local_name — needs atom interning side table"
        )
    }

    fn namespace(
        &self,
    ) -> &<<Self as SelectorsElement>::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
        unimplemented!(
            "LayoutDomAdapter::namespace — needs atom interning side table"
        )
    }

    fn query_container_size(
        &self,
        _: &style::values::specified::ContainerType,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        Default::default()
    }

    fn primary_box_size(&self) -> euclid::default::Size2D<app_units::Au> {
        Default::default()
    }

    fn each_link_in_parent_implicit_scope<F: FnMut(Self)>(&self, _f: F) {}

    fn relative_selector_search_direction(
        &self,
    ) -> style::dom::ElementSelectorFlags {
        style::dom::ElementSelectorFlags::empty()
    }

    fn parent_element_with_filter(
        &self,
        _: &mut style::invalidation::element::invalidator::SiblingTraversalMap<Self>,
    ) -> Option<Self> {
        self.parent_element()
    }
}
