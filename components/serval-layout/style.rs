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
use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use stylo_dom::ElementState;
use taffy::Style as TaffyStyle;

/// Per-node style entry.
///
/// Probe slice today only populates `taffy`. The remaining fields exist so
/// the Stylo trait impls on `StyleNodeRef` have somewhere to read/write
/// cascade-time state from. The cascade populates them in real usage;
/// hand-built probe fixtures leave them at default.
pub struct StyleEntry {
    /// Taffy layout style (populated by hand in the probe; derived from
    /// Stylo `ComputedValues` in the real cascade).
    pub taffy: TaffyStyle,

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
            taffy: self.taffy.clone(),
            stylo_data: UnsafeCell::new(None),
            state: self.state,
            selector_flags: Cell::new(self.selector_flags.get()),
            id_atom: self.id_atom.clone(),
        }
    }
}

impl Default for StyleEntry {
    fn default() -> Self {
        Self {
            taffy: TaffyStyle::default(),
            stylo_data: UnsafeCell::new(None),
            state: ElementState::empty(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            id_atom: None,
        }
    }
}

impl std::fmt::Debug for StyleEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleEntry")
            .field("taffy", &self.taffy)
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

    /// The Taffy style for a node, or Taffy's default style if no entry.
    /// Defaulting (rather than panicking) lets construct.rs handle nodes
    /// without explicit style entries (text nodes, anonymous boxes, etc.).
    pub fn taffy_style(&self, id: NodeId) -> TaffyStyle {
        self.entries
            .get(&id)
            .map(|e| e.taffy.clone())
            .unwrap_or_default()
    }

    /// Ensure a style entry exists for `id`, creating a default one if not.
    /// Returns a mutable reference. The Stylo cascade uses this to allocate
    /// `ElementData` storage before populating it.
    pub fn ensure_entry(&mut self, id: NodeId) -> &mut StyleEntry {
        self.entries.entry(id).or_default()
    }

    /// After the cascade has run + populated `stylo_data`, refresh each
    /// entry's Taffy style from its cascaded `ComputedValues`. Call this
    /// to switch from hand-rolled Taffy styles to cascade-driven layout.
    /// Entries with no cascade data are left untouched.
    ///
    /// Property coverage: display / size / margin / padding / border
    /// widths — the subset needed for box-model semantics. Cf.
    /// `cv_to_taffy::to_taffy_style` for the full mapping.
    pub fn refresh_taffy_from_cascade(&mut self) {
        for entry in self.entries.values_mut() {
            let new_taffy = entry.borrow_data().map(|data| {
                let primary = data.styles.primary();
                crate::cv_to_taffy::to_taffy_style(primary)
            });
            if let Some(t) = new_taffy {
                entry.taffy = t;
            }
        }
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

    /// Give each decoded `<img>` element its intrinsic pixel size on
    /// any axis CSS left `auto`. Explicit `width` / `height` rules win
    /// (only `auto` axes are overridden). Call after
    /// `refresh_taffy_from_cascade` and before `layout`/`construct` so
    /// the Taffy tree sees real image dimensions instead of the 0×0 an
    /// empty replaced element would otherwise get.
    pub fn apply_intrinsic_image_sizes(
        &mut self,
        images: &crate::image_decode::ImagePlane<NodeId>,
    ) {
        use taffy::prelude::TaffyAuto;
        use taffy::style::Dimension;

        for (id, decoded) in images.iter() {
            if let Some(entry) = self.entries.get_mut(id) {
                if entry.taffy.size.width == Dimension::AUTO {
                    entry.taffy.size.width = Dimension::length(decoded.width as f32);
                }
                if entry.taffy.size.height == Dimension::AUTO {
                    entry.taffy.size.height = Dimension::length(decoded.height as f32);
                }
            }
        }
    }
}
