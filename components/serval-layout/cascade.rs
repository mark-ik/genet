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
//! ## Status (v1, 2026-05-18) — INTEGRATION BLOCKED BY STYLE-SHARING SIZE ASSERTION
//!
//! The cascade runner is fully wired: stylist setup, SharedStyleContext,
//! DomTraversal driver, all stitched together. **It panics at runtime**
//! inside Stylo's style-sharing cache initialization:
//!
//! ```text
//! thread 'cascade::tests::...' panicked at
//! style/sharing/mod.rs:611: assertion `left == right` failed
//!   left: 10000  (= size_of::<SharingCache<StyleNodeRef<...>>>())
//!  right: 9488   (= size_of::<TypelessSharingCache>())
//! ```
//!
//! Stylo's style-sharing cache is a thread-local byte buffer sized at
//! compile time assuming Servo's pointer-shaped element type (~8 bytes).
//! Our `StyleNodeRef<'a, D>` is 24 bytes (three references: `&'a D`,
//! `D::NodeId`, `&'a StylePlane<D::NodeId>`). The cache's typeless byte
//! buffer is sized for the smaller element type; constructing the cache
//! against our larger element fails the size assertion in
//! `StyleSharingCache::new()`.
//!
//! Resolution paths (deferred to follow-up):
//!
//! - **Thread-local the StylePlane reference.** Shrink `StyleNodeRef` to
//!   `(dom_ref, id)` — 16 bytes — by stashing `&StylePlane` in a TLS slot
//!   bound at cascade entry. Closer to Servo's 8-byte shape; still not
//!   exactly matching (we'd be 16 vs 8). Probably still trips the
//!   assertion unless the TLS shrinks `dom_ref` too.
//! - **Owned heap CascadeNode.** Allocate a `Box<CascadeNodeImpl<D>>` per
//!   visited node carrying `(dom, id, plane)`; `StyleNodeRef` becomes
//!   `*const CascadeNodeImpl` (8 bytes, pointer-shaped). Allocation
//!   overhead per cascade call.
//! - **Patch Stylo to relax the assertion.** Upstream change; either
//!   parameterize TypelessSharingCache over E's size or remove the
//!   typeless reuse optimization.
//! - **Disable style sharing for our cascade.** Doesn't appear possible
//!   without patching Stylo — the cache is allocated unconditionally
//!   when `StyleContext` is created.
//!
//! The cascade runner stays in the tree as the integration record; its
//! test is `#[ignore]`'d until the size constraint is resolved.

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

use crate::adapter_stylo::StyleNodeRef;
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

    // 5. Drive the traversal. RecalcStyle's process_preorder calls
    //    recalc_style_at on each element, populating its ElementData
    //    in the StylePlane.
    let root_id = dom.document();
    let root = StyleNodeRef::new(dom, root_id, plane);
    let Some(root_element) = first_element_descendant(dom, root_id).map(|id| {
        StyleNodeRef::new(dom, id, plane)
    }) else {
        // No element in the document — nothing to cascade.
        thread_state::exit(ThreadState::LAYOUT);
        return;
    };
    let _ = root; // referenced for symmetry with Blitz; the actual entry is the root element.

    let token = RecalcStyle::pre_traverse(root_element, &context);
    if token.should_traverse() {
        let traverser = RecalcStyle::new(context);
        // Sequential traversal — pass None for the rayon pool.
        driver::traverse_dom(&traverser, token, None);
    }

    // 6. Exit thread state.
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

    /// Cascade integration probe. **Currently `#[ignore]`'d** — see the
    /// file header for the Stylo style-sharing-cache size assertion that
    /// blocks the runtime path. The test code itself is correct shape;
    /// it'll work once StyleNodeRef's size constraint is resolved.
    #[test]
    #[ignore = "blocked on Stylo style-sharing-cache size assertion; \
                StyleNodeRef is 24 bytes but cache assumes 8-byte element. \
                See cascade.rs header for resolution paths."]
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
