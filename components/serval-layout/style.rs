/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// `UnsafeCell` access for the Stylo `ElementData` slot matches Stylo's
// cascade exclusive-access invariant. Documented per-method.
#![allow(unsafe_code)]

//! Style plane skeleton.
//!
//! Per the planes architecture, computed style lives in a `serval-layout`-
//! owned side table keyed by `D::NodeId`. The real implementation will be
//! populated by Stylo's cascade running over `NodeRef` (Stylo trait impls
//! live in `adapter_stylo.rs`, currently a draft). For the probe slice,
//! `StylePlane` is populated by hand — the test constructs the entries
//! directly, bypassing the cascade. This validates the construct + Taffy
//! pipeline without committing to the Stylo adapter shape yet.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::cell::{Cell, UnsafeCell};
use std::hash::Hash;

use rustc_hash::FxHashMap;
use selectors::matching::ElementSelectorFlags;
use servo_arc::Arc;
use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::properties::PropertyDeclarationBlock;
use style::selector_parser::RestyleDamage;
use style::shared_lock::Locked;
use stylo_dom::ElementState;

/// Per-node style entry.
///
/// Holds the cascade's per-element state. Layout reads the computed style
/// straight off `stylo_data` (via the box tree's `TaffyStyloStyle`), so
/// there is no separate owned `taffy::Style` cache — the cascade is the
/// single source of truth. Hand-built fixtures leave the fields at default.
pub struct StyleEntry {
    /// Stylo's `ElementData` storage. Empty until the cascade allocates +
    /// populates. Uses `UnsafeCell` matching Stylo's expectation that the
    /// cascade has exclusive access per node during traversal (the same
    /// pattern Blitz uses in `blitz-dom/src/node/stylo_data.rs`).
    ///
    /// # Safety
    ///
    /// Mutation through this field must happen during Stylo's cascade
    /// traversal, which guarantees one-thread-at-a-time access per node.
    /// Outside the cascade, only immutable borrow access is safe.
    pub stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    /// DOM element state (`:hover`, `:focus`, etc.). Static profile: empty.
    pub state: ElementState,

    /// Selector flags accumulated during selector matching.
    pub selector_flags: Cell<ElementSelectorFlags>,

    /// Atom-interned `id` attribute, if the element has one. Populated by
    /// `StylePlane::populate_for_elements` (which walks the DOM up-front
    /// and atom-interns once per element). Needed because Stylo's
    /// `TElement::id() -> Option<&style::Atom>` returns a borrowed atom
    /// reference — we can't return a freshly-interned atom from inside
    /// the method without a stable storage to anchor the borrow.
    pub id_atom: Option<style::Atom>,

    /// Parsed inline `style="…"` declarations (Author origin), if the element
    /// carries a non-empty `style` attribute. Populated before the cascade by
    /// [`crate::cascade`]'s inline-style pass (which has the `SharedRwLock` to
    /// wrap the block); the stylo adapter's `TElement::style_attribute` returns
    /// a borrow of it, so the cascade applies it at the inline-style level
    /// (above author stylesheet rules). `None` when there is no inline style.
    pub inline_style: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// Stylo's "dirty descendants" bit — set during incremental
    /// invalidation to mark that a descendant needs restyle, consulted by
    /// the traversal to decide whether to descend. No-op'd in the full
    /// cascade (nothing sets it; every element is styled because it has no
    /// `ElementData` yet). Backs `TElement::{set,unset,has}_dirty_descendants`.
    pub dirty_descendants: Cell<bool>,

    /// Whether the invalidator has processed this element's snapshot
    /// (Stylo's `handled_snapshot` bit). Set during incremental restyle so
    /// a snapshot is consumed once. Backs `TElement::{handled_snapshot,
    /// set_handled_snapshot}`.
    pub handled_snapshot: Cell<bool>,
}

// SAFETY: per the cascade's exclusive-access invariant during traversal,
// and immutable-only access outside it. Matches Blitz's same claim on its
// `StyloData` wrapper.
unsafe impl Send for StyleEntry {}
unsafe impl Sync for StyleEntry {}

impl StyleEntry {
    /// Whether Stylo's `ElementData` has been allocated for this entry.
    pub fn has_data(&self) -> bool {
        // SAFETY: read-only access; no aliasing.
        unsafe { (*self.stylo_data.get()).is_some() }
    }

    /// Immutable borrow of the `ElementData`, if present.
    pub fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // SAFETY: read-only access. The cascade's exclusive-access invariant
        // ensures no concurrent writer during traversal; outside the cascade
        // we only ever borrow immutably.
        unsafe { (*self.stylo_data.get()).as_ref().map(|w| w.borrow()) }
    }

    /// Mutable borrow of the `ElementData`. Cascade-time only.
    ///
    /// # Safety
    ///
    /// Caller must guarantee no other borrow exists. The Stylo cascade
    /// enforces this via its single-threaded-per-node invariant.
    pub unsafe fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: caller's responsibility per the # Safety doc above.
        unsafe { (*self.stylo_data.get()).as_mut().map(|w| w.borrow_mut()) }
    }

    /// Initialize the `ElementData` slot if empty, returning a mutable borrow.
    ///
    /// # Safety
    ///
    /// Same as `mutate_data`: caller must guarantee no other borrow exists.
    pub unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: caller's responsibility per the # Safety doc above.
        unsafe {
            let slot = &mut *self.stylo_data.get();
            if slot.is_none() {
                *slot = Some(ElementDataWrapper::default());
            }
            slot.as_mut().unwrap().borrow_mut()
        }
    }

    /// Clear the `ElementData` slot.
    ///
    /// # Safety
    ///
    /// Same as `mutate_data`.
    pub unsafe fn clear_data(&self) {
        // SAFETY: caller's responsibility.
        unsafe {
            *self.stylo_data.get() = None;
        }
    }
}

impl Clone for StyleEntry {
    fn clone(&self) -> Self {
        // ElementDataWrapper is not Clone in general; provide a default
        // (empty) for any cloning need. The probe doesn't clone style
        // entries; cascade-time work mutates in place.
        Self {
            stylo_data: UnsafeCell::new(None),
            state: self.state,
            selector_flags: Cell::new(self.selector_flags.get()),
            id_atom: self.id_atom.clone(),
            inline_style: self.inline_style.clone(),
            dirty_descendants: Cell::new(self.dirty_descendants.get()),
            handled_snapshot: Cell::new(self.handled_snapshot.get()),
        }
    }
}

impl Default for StyleEntry {
    fn default() -> Self {
        Self {
            stylo_data: UnsafeCell::new(None),
            state: ElementState::empty(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            id_atom: None,
            inline_style: None,
            dirty_descendants: Cell::new(false),
            handled_snapshot: Cell::new(false),
        }
    }
}

impl std::fmt::Debug for StyleEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleEntry")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Sparse storage of computed style keyed by `D::NodeId`. Sparse for the
/// probe; the eventual impl picks dense `IndexVec` storage when
/// `D::NodeId` is dense (per `NodeIdSpace` in the planes doc).
pub struct StylePlane<NodeId: Copy + Eq + Hash> {
    entries: FxHashMap<NodeId, StyleEntry>,
}

impl<NodeId: Copy + Eq + Hash> Default for StylePlane<NodeId> {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
        }
    }
}

impl<NodeId: Copy + Eq + Hash> StylePlane<NodeId> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: NodeId, entry: StyleEntry) {
        self.entries.insert(id, entry);
    }

    pub fn get(&self, id: NodeId) -> Option<&StyleEntry> {
        self.entries.get(&id)
    }

    /// Ensure a style entry exists for `id`, creating a default one if not.
    /// Returns a mutable reference. The Stylo cascade uses this to allocate
    /// `ElementData` storage before populating it.
    pub fn ensure_entry(&mut self, id: NodeId) -> &mut StyleEntry {
        self.entries.entry(id).or_default()
    }

    /// Populate empty StyleEntry slots for every element in the given DOM.
    /// The cascade calls `ensure_data` on each element it visits — that
    /// requires a StyleEntry to exist first (cascade orchestration's job,
    /// not the cascade's). This walks the DOM up-front and pre-allocates.
    ///
    /// Also atom-interns each element's `id` attribute into
    /// `StyleEntry::id_atom` — Stylo's rule indexer queries
    /// `TElement::id()` to prune `#foo` rules per element, and that
    /// method returns `&Atom`, so the atom needs a stable home (this
    /// pre-pass establishes it).
    pub fn populate_for_elements<D>(&mut self, dom: &D)
    where
        D: layout_dom_api::LayoutDom<NodeId = NodeId>,
    {
        use html5ever::{namespace_url, ns, LocalName, Namespace};
        let no_ns: Namespace = ns!();
        let id_local = LocalName::from("id");

        let mut queue = vec![dom.document()];
        while let Some(id) = queue.pop() {
            if matches!(dom.kind(id), layout_dom_api::NodeKind::Element) {
                let id_atom = dom
                    .attribute(id, &no_ns, &id_local)
                    .map(style::Atom::from);
                let entry = self.ensure_entry(id);
                entry.id_atom = id_atom;
            }
            queue.extend(dom.dom_children(id));
        }
    }

    /// Clear the per-element `RestyleDamage` across all entries. Called
    /// before an incremental restyle so that, afterward, only the elements
    /// Stylo actually restyled carry damage (the cascade leaves clean
    /// elements untouched). `RestyleDamage` is a per-restyle output, not
    /// persistent state, so clearing it is safe.
    ///
    /// Must be called outside a cascade (single-threaded, no live borrow).
    pub fn reset_damage(&self) {
        for entry in self.entries.values() {
            // SAFETY: not inside a cascade traversal — single-threaded
            // access, no other borrow of this entry's `ElementData`.
            if let Some(mut data) = unsafe { entry.mutate_data() } {
                data.damage = RestyleDamage::empty();
            }
        }
    }

    /// Set an element's interaction [`ElementState`] (`:hover`, `:focus`,
    /// `:active`, …), creating the entry if needed. The cascade reads this
    /// via `TElement::state`, so state-backed pseudo-class selectors match
    /// once a host interaction layer sets it. (Scaffold: serval has no
    /// input pipeline yet; incremental state-change restyle — snapshot the
    /// old state + invalidate — is the follow-on, parallel to the
    /// attribute path.)
    pub fn set_element_state(&mut self, id: NodeId, state: ElementState) {
        self.ensure_entry(id).state = state;
    }

    /// The union of `RestyleDamage` across all entries. After
    /// [`reset_damage`](Self::reset_damage) + an incremental restyle, this
    /// is exactly the damage of the elements that were restyled this pass.
    pub fn aggregate_damage(&self) -> RestyleDamage {
        let mut acc = RestyleDamage::empty();
        for entry in self.entries.values() {
            if let Some(data) = entry.borrow_data() {
                acc |= data.damage;
            }
        }
        acc
    }
}
