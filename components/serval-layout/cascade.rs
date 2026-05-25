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
use style::selector_parser::{RestyleDamage, SnapshotMap};
use servo_arc::Arc as ServoArc;
use style::media_queries::MediaList;
use style::shared_lock::{SharedRwLock, StylesheetGuards};
use style::stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
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
/// Sequential (no rayon pool). `stylesheets` is a slice of CSS source
/// strings to load as UA-origin sheets before the cascade runs;
/// pass `&[]` for empty-stylist behavior (every element receives
/// Stylo's default cascaded values).
///
/// `plane` must be pre-populated with empty `StyleEntry` slots for every
/// element via `StylePlane::populate_for_elements(dom)` before this call —
/// the cascade calls `ensure_data` on each element, which requires an
/// entry to exist.
pub fn run_cascade<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    viewport: euclid::default::Size2D<f32>,
    stylesheets: &[&str],
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    cascade_traverse(dom, plane, viewport, stylesheets, None);
}

/// Incremental restyle: re-cascade only the elements a batch of
/// `DomMutation`s actually affects, reusing the prior `plane`'s
/// `ElementData`.
///
/// Builds a Stylo [`SnapshotMap`](crate::snapshot::build_snapshot_map)
/// from the mutation stream (the pre-mutation state), marks the dirty
/// path from each changed element up to the root so Stylo's traversal
/// descends to reach it, then re-runs the cascade with the snapshots in
/// context. Stylo's `ElementData::invalidate_style_if_needed` (invoked
/// per element during the traversal) runs the actual
/// `StateAndAttrInvalidationProcessor` + `TreeStyleInvalidator` against
/// (snapshot, selector-dependency-map), setting `RestyleHint`s so only
/// the genuinely-affected elements recompute; clean subtrees keep their
/// prior `ComputedValues`.
///
/// `plane` must already hold the prior cascade's data. Non-attribute
/// mutations (structural / character-data) don't drive this path — they
/// go through the relayout scope, not the attribute/state invalidator.
///
/// Returns a [`RestyleOutcome`] reporting whether any restyled element's
/// `RestyleDamage` requires re-layout (vs repaint-only) — so the caller
/// can skip layout for paint-only changes (e.g. a `color` swap).
pub fn restyle_with_snapshots<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    viewport: euclid::default::Size2D<f32>,
    stylesheets: &[&str],
    mutations: &[layout_dom_api::DomMutation<D::NodeId>],
) -> RestyleOutcome
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use layout_dom_api::DomMutation;

    // Clear stale damage so the post-restyle aggregate reflects only what
    // this pass restyled.
    plane.reset_damage();

    let snapshots = crate::snapshot::build_snapshot_map(dom, mutations);

    // Mark the dirty path: for each attribute-changed element, set the
    // dirty-descendants bit on every ancestor up to the root, so the
    // traversal descends far enough for the element's parent to process
    // its snapshot (Stylo processes a child's snapshot while traversing
    // the parent — see `traversal.rs`). Cell access through `&` entries.
    for m in mutations {
        if let DomMutation::AttributeChanged { node, .. } = m {
            let mut cur = dom.parent(*node);
            while let Some(ancestor) = cur {
                if let Some(entry) = plane.get(ancestor) {
                    entry.dirty_descendants.set(true);
                }
                cur = dom.parent(ancestor);
            }
        }
    }

    cascade_traverse(dom, plane, viewport, stylesheets, Some(&snapshots));

    // Stylo stored each restyled element's RestyleDamage on its
    // ElementData during the traversal (via compute_style_difference).
    // RELAYOUT (the fully-saturated bit) means box geometry may have
    // changed → re-layout; lesser bits (REPAINT / stacking / overflow)
    // are paint-tier for serval's taffy-driven layout.
    let damage = plane.aggregate_damage();
    RestyleOutcome {
        needs_relayout: damage.contains(RestyleDamage::RELAYOUT),
    }
}

/// Result of [`restyle_with_snapshots`]: whether the restyle changed
/// anything that requires re-running layout, or was repaint-only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestyleOutcome {
    /// `true` if any restyled element's damage requires re-layout;
    /// `false` for a paint-only change (the prior `FragmentPlane` is
    /// still valid — skip layout, just re-emit paint).
    pub needs_relayout: bool,
}

/// Partial cascade for a **structural** change: re-cascade only the
/// mutation's affected subtrees (`roots`), reusing the prior `plane`.
///
/// Each root is hinted `RestyleHint::restyle_subtree()` (restyle self +
/// descendants) and the dirty-descendant path from its parent up to the
/// document root is marked, so Stylo's traversal descends to it and
/// re-cascades that subtree — covering the inserted/replaced nodes (no
/// `ElementData` yet → styled) and within-parent sibling / `:nth-child`
/// effects. Elements outside the affected subtrees keep their prior
/// `ComputedValues` (the cascade skips clean nodes).
///
/// Boundary (documented, same spirit as the `SubtreeView` scope): a
/// structural change whose selector reach crosses *outside* the affected
/// subtree (`~`/`+` reaching a different parent, `:has()`, ancestor
/// `:nth-child`) is not re-matched — those want full structural
/// invalidation. `IncrementalLayout` only takes this path for the common
/// within-subtree case.
pub fn restyle_structural<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    viewport: euclid::default::Size2D<f32>,
    stylesheets: &[&str],
    roots: &[D::NodeId],
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use style::invalidation::element::restyle_hints::RestyleHint;

    plane.reset_damage();

    for &root in roots {
        // Hint the root's subtree for restyle. The root existed before the
        // mutation (it's the container / replaced node), so it has data;
        // RESTYLE_DESCENDANTS propagates to its children — including any
        // newly-inserted ones, which get styled as no-data elements.
        if let Some(entry) = plane.get(root) {
            // SAFETY: not inside a cascade traversal (single-threaded, no
            // live borrow of this entry's ElementData).
            if let Some(mut data) = unsafe { entry.mutate_data() } {
                data.hint.insert(RestyleHint::restyle_subtree());
            }
        }
        // Mark the dirty path so the traversal descends to the root.
        let mut cur = dom.parent(root);
        while let Some(ancestor) = cur {
            if let Some(entry) = plane.get(ancestor) {
                entry.dirty_descendants.set(true);
            }
            cur = dom.parent(ancestor);
        }
    }

    cascade_traverse(dom, plane, viewport, stylesheets, None);
}

/// Shared cascade traversal. `snapshots = None` is a full cascade (every
/// element styled because none has `ElementData` yet); `Some` is the
/// incremental restyle path (existing data + snapshots drive Stylo's
/// invalidator to recompute only the affected elements).
fn cascade_traverse<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    viewport: euclid::default::Size2D<f32>,
    stylesheets: &[&str],
    snapshots: Option<&SnapshotMap>,
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

    // 3. Stylist setup. Prepend the baseline UA stylesheet
    //    (`ua_defaults::UA_DEFAULTS`) — `<html>`/`<body>` default to
    //    `display: block` and fill the viewport, structural block
    //    elements (div, p, headings, lists, etc.) default to
    //    `display: block`. Then append user-provided stylesheets
    //    (also UA-origin for now; engine vs author origin handling
    //    is a follow-up). The Stylist resolves rule indices during
    //    flush, so all sheets must be present first.
    let device = make_device(viewport);
    let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);
    let ua_sheet = parse_stylesheet(crate::ua_defaults::UA_DEFAULTS, &lock);
    stylist.append_stylesheet(ua_sheet, &read);
    for css in stylesheets {
        let sheet = parse_stylesheet(css, &lock);
        stylist.append_stylesheet(sheet, &read);
    }
    stylist.flush(&guards);

    // 4. SharedStyleContext bundles everything the cascade needs. For a
    //    full cascade the snapshot map is empty; for incremental restyle
    //    it carries the pre-mutation snapshots Stylo's invalidator reads.
    let empty_snapshots = SnapshotMap::new();
    let snapshot_map = snapshots.unwrap_or(&empty_snapshots);
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
        snapshot_map,
        registered_speculative_painters: &registered_painters,
    };

    // 5. Enter cascade TLS context. StyleNodeRef methods that need
    //    `dom`/`plane`/`shared_lock`/snapshot access read from this slot;
    //    outside the guard they panic. `has_snapshot` consults the same
    //    map (None ⇒ always false ⇒ full-cascade behavior).
    let plane_ref: &StylePlane<D::NodeId> = &*plane;
    let _guard = CascadeGuard::<D>::enter(dom, plane_ref, &lock, snapshots);

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

/// Parse a single CSS source string as a UA-origin `DocumentStyleSheet`.
/// No loader or error reporter (synthetic stylesheets don't @import; if
/// they did we'd plumb a real loader).
fn parse_stylesheet(css: &str, lock: &SharedRwLock) -> DocumentStyleSheet {
    let url = url::Url::parse("about:internal-stylesheet")
        .expect("about: URL parses");
    let url_data = UrlExtraData::from(url);
    let media = ServoArc::new(lock.wrap(MediaList::empty()));
    let sheet = Stylesheet::from_str(
        css,
        url_data,
        Origin::UserAgent,
        media,
        lock.clone(),
        None, // stylesheet loader
        None, // error reporter
        QuirksMode::NoQuirks,
        AllowImportRules::Yes,
    );
    DocumentStyleSheet(ServoArc::new(sheet))
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
            &[],
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

    /// Probe that loaded stylesheets actually apply to matched
    /// elements. The cascade runs with a UA-origin sheet that paints
    /// <body> red; we read `background_color` off the computed
    /// values and assert the sRGB components match.
    #[test]
    fn cascade_applies_loaded_stylesheet_to_matched_elements() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();

        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["body { background-color: rgb(255, 0, 0); }"],
        );

        let body_id = find_element(&document, local_name!("body")).expect("body exists");
        let entry = plane.get(body_id).expect("body StyleEntry exists");
        let data = entry.borrow_data().expect("body ElementData populated");
        let primary = data.styles.primary();
        let bg = &primary.get_background().background_color;
        // `color: currentcolor` resolution uses the inherited `color`,
        // which the cascade defaults to opaque black. For absolute
        // backgrounds (the `rgb(255,0,0)` literal in the sheet) the
        // current_color is unused.
        let current_color = primary.get_inherited_text().color;
        let absolute = bg.resolve_to_absolute(&current_color);
        let srgb = absolute.into_srgb_legacy();
        let [r, g, b, a] = *srgb.raw_components();
        assert!((r - 1.0).abs() < 0.001, "red channel: {r}");
        assert!(g < 0.001, "green channel: {g}");
        assert!(b < 0.001, "blue channel: {b}");
        assert!((a - 1.0).abs() < 0.001, "alpha: {a}");
    }

    /// Probe class + id selector matching. Two rules in the same
    /// sheet — `.highlight { background: blue }` and
    /// `#title { color: green }` — should both match their
    /// respective elements (`<p class="highlight">` and
    /// `<h1 id="title">`).
    #[test]
    fn cascade_matches_class_and_id_selectors() {
        let document = StaticDocument::parse(
            "<html><body>\
                <h1 id=\"title\">T</h1>\
                <p class=\"highlight\">P</p>\
            </body></html>",
        );
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &[
                ".highlight { background-color: rgb(0, 0, 255); } \
                 #title { color: rgb(0, 255, 0); }",
            ],
        );

        let h1_id = find_element(&document, local_name!("h1")).expect("h1 exists");
        let p_id = find_element(&document, local_name!("p")).expect("p exists");

        // <h1 id="title"> — color: green
        let h1_entry = plane.get(h1_id).expect("h1 StyleEntry");
        let h1_data = h1_entry.borrow_data().expect("h1 data");
        let h1_color = h1_data.styles.primary().get_inherited_text().color;
        let h1_srgb = h1_color.into_srgb_legacy();
        let [r, g, b, _] = *h1_srgb.raw_components();
        assert!(r < 0.001, "h1 red: {r}");
        assert!((g - 1.0).abs() < 0.001, "h1 green: {g}");
        assert!(b < 0.001, "h1 blue: {b}");

        // <p class="highlight"> — background-color: blue
        let p_entry = plane.get(p_id).expect("p StyleEntry");
        let p_data = p_entry.borrow_data().expect("p data");
        let p_primary = p_data.styles.primary();
        let bg = &p_primary.get_background().background_color;
        let current = p_primary.get_inherited_text().color;
        let absolute = bg.resolve_to_absolute(&current);
        let srgb = absolute.into_srgb_legacy();
        let [r, g, b, _] = *srgb.raw_components();
        assert!(r < 0.001, "p red: {r}");
        assert!(g < 0.001, "p green: {g}");
        assert!((b - 1.0).abs() < 0.001, "p blue: {b}");
    }

    /// Attribute selectors match against element attributes: an
    /// `[data-state="on"]` rule (value match) and a `[hidden]` rule
    /// (existence) each apply to the right element. This is the receipt
    /// that `SelectorsElement::attr_matches` is wired (it was stubbed to
    /// `false`, so `[attr]` selectors matched nothing).
    #[test]
    fn cascade_matches_attribute_selectors() {
        let document = StaticDocument::parse(
            "<html><body>\
                <p data-state=\"on\">A</p>\
                <p hidden>B</p>\
                <p data-state=\"off\">C</p>\
            </body></html>",
        );
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "[data-state=\"on\"] { color: rgb(0, 255, 0); } \
                 [hidden] { color: rgb(0, 0, 255); }",
            ],
        );

        let ps: Vec<_> = {
            let mut out = Vec::new();
            let mut q = vec![document.document()];
            while let Some(id) = q.pop() {
                if document.element_name(id).is_some_and(|n| n.local == local_name!("p")) {
                    out.push(id);
                }
                let mut kids: Vec<_> = document.dom_children(id).collect();
                kids.reverse();
                q.extend(kids);
            }
            out
        };
        assert_eq!(ps.len(), 3);

        let green = |c: [f32; 4]| c[1] > 0.99 && c[0] < 0.01 && c[2] < 0.01;
        let blue = |c: [f32; 4]| c[2] > 0.99 && c[0] < 0.01 && c[1] < 0.01;

        // p[0] = data-state=on → green; p[1] = hidden → blue; p[2] =
        // data-state=off → neither rule (default black).
        assert!(green(color_of::<StaticDocument>(&plane, ps[0])), "[data-state=on] → green");
        assert!(blue(color_of::<StaticDocument>(&plane, ps[1])), "[hidden] → blue");
        let c2 = color_of::<StaticDocument>(&plane, ps[2]);
        assert!(!green(c2) && !blue(c2), "data-state=off matches neither rule");
    }

    /// State-backed pseudo-classes match against the element's
    /// `ElementState`: a `p:hover { color: red }` rule applies to the `<p>`
    /// whose state has `HOVER` set, not its sibling. This is the scaffold
    /// receipt that `match_non_ts_pseudo_class` reads element state (it was
    /// stubbed `false`); the host interaction layer sets the state.
    #[test]
    fn cascade_matches_hover_pseudo_class() {
        use stylo_dom::ElementState;

        let document =
            StaticDocument::parse("<html><body><p>A</p><p>B</p></body></html>");
        let ps: Vec<_> = {
            let mut out = Vec::new();
            let mut q = vec![document.document()];
            while let Some(id) = q.pop() {
                if document.element_name(id).is_some_and(|n| n.local == local_name!("p")) {
                    out.push(id);
                }
                let mut kids: Vec<_> = document.dom_children(id).collect();
                kids.reverse();
                q.extend(kids);
            }
            out
        };
        assert_eq!(ps.len(), 2);

        let mut plane: StylePlane<_> = StylePlane::new();
        // Host sets :hover on the first <p> before the cascade.
        plane.set_element_state(ps[0], ElementState::HOVER);
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["p:hover { color: rgb(255, 0, 0); }"],
        );

        let hovered = color_of::<StaticDocument>(&plane, ps[0]);
        let plain = color_of::<StaticDocument>(&plane, ps[1]);
        assert!(hovered[0] > 0.99 && hovered[1] < 0.01, ":hover <p> should be red, got {hovered:?}");
        assert!(plain[0] < 0.01, "non-hovered <p> should stay default, got {plain:?}");
    }

    /// The text `color` an element's cascade resolved to, as straight RGBA.
    fn color_of<D>(plane: &StylePlane<D::NodeId>, id: D::NodeId) -> [f32; 4]
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + std::hash::Hash,
    {
        let entry = plane.get(id).expect("StyleEntry");
        let data = entry.borrow_data().expect("ElementData");
        let color = data.styles.primary().get_inherited_text().color;
        *color.into_srgb_legacy().raw_components()
    }

    /// Incremental restyle must produce the **same** computed styles as a
    /// full re-cascade. Toggle a `<p>`'s class from `a` (red) to `b`
    /// (blue): `restyle_with_snapshots` recomputes the `<p>` through
    /// Stylo's invalidator (snapshot: old class `a`), and the result
    /// matches a fresh full cascade of the mutated DOM. An untouched
    /// sibling keeps its color.
    #[test]
    fn incremental_restyle_matches_full_recascade_on_class_toggle() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] =
            &[".a { color: rgb(255,0,0); } .b { color: rgb(0,0,255); } .keep { color: rgb(0,255,0); }"];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());
        let attr = |l: &str| QualName::new(None, ns!(), l.into());

        // html > body > (p.a, span.keep)
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.set_attribute(p, attr("class"), "a");
        dom.append_child(body, p);
        let span = dom.create_element(html("span"));
        dom.set_attribute(span, attr("class"), "keep");
        dom.append_child(body, span);

        // Prior full cascade. <p> is red, <span> green.
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET);
        assert_eq!(color_of::<ScriptedDom>(&plane, p)[0], 1.0, "p starts red");

        // Mutate class a → b, drain only that mutation, then restyle.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(p, attr("class"), "b");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, &muts);

        // Oracle: a fresh full cascade of the mutated DOM.
        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut oracle, euclid::Size2D::new(800.0, 600.0), SHEET);

        let p_inc = color_of::<ScriptedDom>(&plane, p);
        let p_full = color_of::<ScriptedDom>(&oracle, p);
        assert_eq!(p_inc, p_full, "incremental <p> color must match full re-cascade");
        assert!((p_inc[2] - 1.0).abs() < 0.001, "<p> should be blue after a→b, got {p_inc:?}");

        // The untouched sibling matches too (still green).
        let span_inc = color_of::<ScriptedDom>(&plane, span);
        let span_full = color_of::<ScriptedDom>(&oracle, span);
        assert_eq!(span_inc, span_full, "untouched <span> must match full re-cascade");
        assert!((span_inc[1] - 1.0).abs() < 0.001, "<span> should stay green, got {span_inc:?}");
    }

    /// Invalidation must **propagate to descendants**, not just the
    /// changed element. A `.box p { color: blue }` rule: toggling the
    /// container's class to `box` recolors the *child* `<p>` (which
    /// didn't itself change). `restyle_with_snapshots` must reach + restyle
    /// it, matching a full re-cascade. This is the receipt that Stylo's
    /// invalidator sets descendant hints through serval's adapter.
    #[test]
    fn incremental_restyle_propagates_to_descendants() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &["p { color: rgb(0,0,0); } .box p { color: rgb(0,0,255); }"];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());
        let attr = |l: &str| QualName::new(None, ns!(), l.into());

        // html > body > div > p   (div initially has no class)
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let div = dom.create_element(html("div"));
        dom.append_child(body, div);
        let p = dom.create_element(html("p"));
        dom.append_child(div, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET);
        assert!(color_of::<ScriptedDom>(&plane, p)[2] < 0.001, "p starts black (no .box ancestor)");

        // Add class="box" to the div; the descendant <p> must recolor.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(div, attr("class"), "box");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, &muts);

        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut oracle, euclid::Size2D::new(800.0, 600.0), SHEET);

        let p_inc = color_of::<ScriptedDom>(&plane, p);
        assert_eq!(
            p_inc,
            color_of::<ScriptedDom>(&oracle, p),
            "descendant <p> must match full re-cascade after the container's class change"
        );
        assert!((p_inc[2] - 1.0).abs() < 0.001, "descendant <p> should be blue via `.box p`, got {p_inc:?}");
    }

    /// Incremental restyle handles **attribute-selector** dependencies:
    /// toggling `data-state` off→on makes `[data-state="on"]` match, and
    /// `restyle_with_snapshots` recolors the element to match a full
    /// re-cascade. (Exercises attr snapshots + `attr_matches` together.)
    #[test]
    fn incremental_restyle_handles_attribute_selectors() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &["p { color: rgb(0,0,0); } p[data-state=\"on\"] { color: rgb(0,255,0); }"];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());
        let attr = |l: &str| QualName::new(None, ns!(), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.set_attribute(p, attr("data-state"), "off");
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET);
        assert!(color_of::<ScriptedDom>(&plane, p)[1] < 0.01, "p starts black (data-state=off)");

        // Toggle data-state off → on.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(p, attr("data-state"), "on");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, &muts);

        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut oracle, euclid::Size2D::new(800.0, 600.0), SHEET);

        let inc = color_of::<ScriptedDom>(&plane, p);
        assert_eq!(inc, color_of::<ScriptedDom>(&oracle, p), "attr restyle must match full re-cascade");
        assert!(inc[1] > 0.99, "p should be green after data-state→on, got {inc:?}");
    }

    /// RestyleDamage drives the repaint-vs-relayout decision: a `color`-only
    /// change is repaint-only (`needs_relayout == false`), while a `width`
    /// change needs re-layout (`true`). This is the signal the live path
    /// uses to skip layout for paint-only mutations.
    #[test]
    fn restyle_outcome_distinguishes_repaint_from_relayout() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &[
            ".red { color: rgb(255,0,0); } .blue { color: rgb(0,0,255); } \
             .wide { width: 200px; } .narrow { width: 50px; }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());
        let attr = |l: &str| QualName::new(None, ns!(), l.into());

        let build = || {
            let mut dom = ScriptedDom::new();
            let root = dom.document();
            let h = dom.create_element(html("html"));
            dom.append_child(root, h);
            let body = dom.create_element(html("body"));
            dom.append_child(h, body);
            let p = dom.create_element(html("p"));
            dom.append_child(body, p);
            (dom, p)
        };

        // Color-only change → repaint-only.
        {
            let (mut dom, p) = build();
            dom.set_attribute(p, attr("class"), "red");
            let mut plane: StylePlane<_> = StylePlane::new();
            run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET);
            let mut sink = Vec::new();
            dom.drain_mutations(&mut sink);
            dom.set_attribute(p, attr("class"), "blue");
            let mut muts = Vec::new();
            dom.drain_mutations(&mut muts);
            let outcome =
                restyle_with_snapshots(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, &muts);
            assert!(!outcome.needs_relayout, "color swap should be repaint-only");
        }

        // Width change → relayout.
        {
            let (mut dom, p) = build();
            dom.set_attribute(p, attr("class"), "narrow");
            let mut plane: StylePlane<_> = StylePlane::new();
            run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET);
            let mut sink = Vec::new();
            dom.drain_mutations(&mut sink);
            dom.set_attribute(p, attr("class"), "wide");
            let mut muts = Vec::new();
            dom.drain_mutations(&mut muts);
            let outcome =
                restyle_with_snapshots(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, &muts);
            assert!(outcome.needs_relayout, "width change should require relayout");
        }
    }
}
