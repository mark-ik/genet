/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stylo cascade runner.
//!
//! Wires Stylo's cascade machinery over a `LayoutDom` + `StylePlane` pair.
//! Mirrors Blitz's `BaseDocument::resolve_stylist` (in
//! `blitz-dom/src/stylo.rs` ~lines 60-160) adapted for the planes
//! architecture where style state lives in `serval-layout`-owned planes
//! rather than embedded on DOM nodes.
//!
//! ## Status (v1.1, 2026-05-18) — END-TO-END, EMPTY-STYLIST
//!
//! Cascade runs and populates `ElementData` for every element in the test
//! document. Probe slice exercises an empty `Stylist` (no stylesheets
//! loaded), so every element receives Stylo's default cascaded values —
//! the real win is that the trait surface holds together end-to-end.
//!
//! The original integration blocker (Stylo's style-sharing cache size
//! assertion in `style/sharing/mod.rs:611`) was resolved by the
//! TLS-context refactor of `StyleNodeRef` — see `adapter_stylo.rs` for
//! the design. Briefly: Stylo's `TypelessSharingCache` is a leaked
//! thread-local sized for `FakeCandidate { _element: usize, … }`, which
//! bakes in the assumption that `E` is pointer-shaped. Blitz's
//! `BlitzNode<'a> = &'a Node` satisfies it by embedding style state on
//! the DOM node. We keep the planes split (style lives in `StylePlane`,
//! not on nodes) and shrunk `StyleNodeRef<'a, D>` to `{ id: D::NodeId }`
//! (8 bytes for `usize`-sized NodeIds); `(dom, plane)` is stashed in a
//! TLS slot for the cascade duration via `CascadeGuard`.
//!
//! Next steps:
//! - Load stylesheets into the `Stylist` so real CSS rules apply.
//! - Wire `SharedRwLock` exposure through `TDocument::shared_lock`
//!   (currently `unimplemented!()`, untouched because the empty-stylist
//!   path doesn't reach it).
//! - Replace `each_class` / `each_attr_name` / `id` skeletons with
//!   real impls once stylesheets exist to exercise them.

#![allow(unsafe_code)]

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use selectors::matching::QuirksMode;
use style::animation::DocumentAnimationSet;
use style::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
};
use style::device::Device;
use style::driver;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::properties::style_structs::Font;
use style::queries::values::PrefersColorScheme;
use style::selector_parser::SnapshotMap;
use style::shared_lock::{SharedRwLock, StylesheetGuards};
use style::stylist::Stylist;
use style::thread_state::{self, ThreadState};
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::traversal_flags::TraversalFlags;
use style::Atom;

use crate::adapter_stylo::{CascadeGuard, StyleNodeRef};
use crate::font_metrics::StubFontMetricsProvider;
use crate::style::StylePlane;

// =============================================================================
// Stub RegisteredSpeculativePainters
// =============================================================================

/// No-op registered-painter table. Static profile has no CSS Houdini
/// paint worklets; future profile facades that add them register here.
struct NoOpRegisteredPainters;

impl RegisteredSpeculativePainters for NoOpRegisteredPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

// =============================================================================
// RecalcStyle — DomTraversal driver
// =============================================================================

/// Mirror of Blitz's `RecalcStyle` driver. Holds the shared style context
/// for the duration of one cascade traversal.
pub struct RecalcStyle<'a> {
    context: SharedStyleContext<'a>,
}

impl<'a> RecalcStyle<'a> {
    pub fn new(context: SharedStyleContext<'a>) -> Self {
        Self { context }
    }
}

impl<E> DomTraversal<E> for RecalcStyle<'_>
where
    E: style::dom::TElement,
{
    fn process_preorder<F: FnMut(E::ConcreteNode)>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<E>,
        node: E::ConcreteNode,
        note_child: F,
    ) {
        if let Some(el) = <E::ConcreteNode as style::dom::TNode>::as_element(&node) {
            // SAFETY: Stylo's traversal guarantees exclusive per-node access.
            let mut data = unsafe { el.ensure_data() };
            recalc_style_at(self, traversal_data, context, el, &mut data, note_child);
            unsafe { el.unset_dirty_descendants() }
        }
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn process_postorder(&self, _ctx: &mut StyleContext<E>, _node: E::ConcreteNode) {
        unreachable!("postorder traversal not used in this driver")
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context
    }
}

// =============================================================================
// Cascade entry point
// =============================================================================

/// Build a default Stylo `Device` suitable for the cascade runner.
///
/// Uses screen media, no-quirks mode, the given viewport size at 1.0x
/// device-pixel ratio, the stub `FontMetricsProvider`, default initial
/// `ComputedValues`, and `Light` color-scheme preference.
fn make_device(viewport: euclid::default::Size2D<f32>) -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        euclid::Size2D::from_untyped(viewport),
        euclid::Scale::new(1.0),
        Box::new(StubFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
    )
}

/// Run Stylo's cascade over `dom`, populating `plane` with `ElementData`
/// for every element.
///
/// Sequential (no rayon pool). Empty stylist (no stylesheets loaded);
/// every element ends up with Stylo's default cascaded values. Real CSS
/// rule application requires loading stylesheets into the stylist; that
/// arrives in a follow-up.
///
/// `plane` must be pre-populated with empty `StyleEntry` slots for every
/// element via `StylePlane::populate_for_elements(dom)` before this call —
/// the cascade calls `ensure_data` on each element, which requires an
/// entry to exist.
pub fn run_cascade<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    viewport: euclid::default::Size2D<f32>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    // Pre-populate StylePlane entries for every element. The cascade's
    // ensure_data() requires entries to exist (cascade-orchestration
    // contract; see StyleNodeRef::ensure_data documentation).
    plane.populate_for_elements(dom);

    // 1. Enter Stylo's layout-thread state. Required by ThreadSafeBindings
    //    checks scattered through the cascade.
    thread_state::enter(ThreadState::LAYOUT);

    // 2. Lock + guard setup. SharedRwLock is the cross-thread lock for
    //    stylesheet contents and ElementData.
    let lock = SharedRwLock::new();
    let read = lock.read();
    let guards = StylesheetGuards {
        author: &read,
        ua_or_user: &read,
    };

    // 3. Stylist setup. Empty stylesheet set; cascade still runs and
    //    produces default cascaded values for every element.
    let device = make_device(viewport);
    let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);
    stylist.flush(&guards);

    // 4. SharedStyleContext bundles everything the cascade needs.
    let snapshots = SnapshotMap::new();
    let animations = DocumentAnimationSet::default();
    let registered_painters = NoOpRegisteredPainters;

    let context = SharedStyleContext {
        traversal_flags: TraversalFlags::empty(),
        stylist: &stylist,
        options: GLOBAL_STYLE_DATA.options.clone(),
        guards,
        visited_styles_enabled: false,
        animations,
        current_time_for_animations: 0.0,
        snapshot_map: &snapshots,
        registered_speculative_painters: &registered_painters,
    };

    // 5. Enter cascade TLS context. StyleNodeRef methods that need
    //    `dom`/`plane` access read from this slot; outside the guard they
    //    panic. See `adapter_stylo::CascadeGuard` for the invariant.
    let plane_ref: &StylePlane<D::NodeId> = &*plane;
    let _guard = CascadeGuard::<D>::enter(dom, plane_ref);

    // 6. Drive the traversal. RecalcStyle's process_preorder calls
    //    recalc_style_at on each element, populating its ElementData
    //    in the StylePlane (via UnsafeCell interior mutability per entry).
    if let Some(root_id) = first_element_descendant(dom, dom.document()) {
        let root_element: StyleNodeRef<'_, D> = StyleNodeRef::new(root_id);
        let token = RecalcStyle::pre_traverse(root_element, &context);
        if token.should_traverse() {
            let traverser = RecalcStyle::new(context);
            driver::traverse_dom(&traverser, token, None);
        }
    }

    // 7. Drop guard (clears TLS), then exit thread state.
    drop(_guard);
    thread_state::exit(ThreadState::LAYOUT);
}

/// Walk `dom`'s children of `from` and return the first element descendant.
/// Used to find the document's root element (`<html>`).
fn first_element_descendant<D: LayoutDom>(dom: &D, from: D::NodeId) -> Option<D::NodeId> {
    for child in dom.dom_children(from) {
        if matches!(dom.kind(child), layout_dom_api::NodeKind::Element) {
            return Some(child);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::StaticDocument;

    use super::*;
    use crate::adapter::NodeRef;

    fn find_element<'a, D: LayoutDom>(
        dom: &'a D,
        local: html5ever::LocalName,
    ) -> Option<D::NodeId> {
        let mut queue = vec![dom.document()];
        while let Some(id) = queue.pop() {
            if let Some(name) = dom.element_name(id) {
                if name.local == local {
                    return Some(id);
                }
            }
            queue.extend(dom.dom_children(id));
        }
        None
    }

    /// Cascade integration probe. After the TLS-context refactor of
    /// `StyleNodeRef` (now 8 bytes, NodeId-only), the cache size
    /// assertion passes and the cascade runs end-to-end.
    #[test]
    fn cascade_populates_element_data_for_every_element() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();

        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
        );

        // Every element should now have ElementData populated.
        let html_id = find_element(&document, local_name!("html")).expect("html exists");
        let body_id = find_element(&document, local_name!("body")).expect("body exists");
        let p_id = find_element(&document, local_name!("p")).expect("p exists");

        for (name, id) in [("html", html_id), ("body", body_id), ("p", p_id)] {
            let entry = plane.get(id).unwrap_or_else(|| panic!("{name}: no StyleEntry"));
            assert!(entry.has_data(), "{name}: no ElementData populated by cascade");
        }
    }
}
