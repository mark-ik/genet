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
//! ## Status (2026-06-03) — LIVE, REAL STYLESHEETS
//!
//! `run_cascade` builds a `Stylist` from the caller's author sheets and
//! cascades real CSS over every element, populating `ElementData` in the
//! `StylePlane`. This is the live path behind pelt-live,
//! meerkat, and the orrery. Selector matching (`each_class` /
//! `each_attr_name` / `id`) and `SharedRwLock` exposure via
//! `TDocument::shared_lock` are wired (see `adapter_stylo.rs`). The one
//! intentional gap is Shadow DOM, which the static/scripted profile does
//! not support (`unimplemented!()` in the `TElement` impl).
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

#![allow(unsafe_code)]

use std::hash::Hash;

use engine_observables_api::{InteractionState, SourceNodeId};
use layout_dom_api::LayoutDom;
use rustc_hash::FxHashMap;
use selectors::matching::QuirksMode;
use cssparser::{Parser, ParserInput};
use servo_arc::Arc as ServoArc;
use style::Atom;
use style::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
};
use style::device::Device;
use style::parser::ParserContext;
use style::driver;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::media_queries::MediaList;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::properties::declaration_block::parse_style_attribute;
use style::properties::style_structs::Font;
use style::queries::values::{MediaEnvironment, PrefersColorScheme, PrefersReducedMotion};
use style::selector_parser::{RestyleDamage, SnapshotMap};
use style::servo::media_features::PointerCapabilities;
use style::shared_lock::{SharedRwLock, StylesheetGuards};
use style::stylesheets::{
    AllowImportRules, CssRuleType, CustomMediaEvaluator, DocumentStyleSheet, Origin, Stylesheet,
    UrlExtraData,
};
use style::stylist::Stylist;
use style::thread_state::{self, ThreadState};
use style_traits::{ParsingMode, ToCss};
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::traversal_flags::TraversalFlags;
use stylo_dom::ElementState;

use crate::adapter_stylo::{CascadeGuard, StyleNodeRef, selectors_quirks_mode};
use crate::font_metrics::SkrifaFontMetricsProvider;
use crate::style::{ContainIntrinsicOverride, StylePlane};

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
/// Uses screen media, the document's `quirks` mode, the given viewport size at
/// 1.0x device-pixel ratio, the live skrifa `FontMetricsProvider`, default
/// initial `ComputedValues`, and `Light` color-scheme preference.
fn make_device(viewport: euclid::default::Size2D<f32>, quirks: QuirksMode) -> Device {
    make_device_with_scheme(viewport, quirks, PrefersColorScheme::Light)
}

fn make_device_with_scheme(
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    scheme: PrefersColorScheme,
) -> Device {
    make_device_with_prefs(
        viewport,
        quirks,
        MediaEnvironment {
            prefers_color_scheme: scheme,
            ..MediaEnvironment::default()
        },
    )
}

/// Build a `Device` carrying the host's full media-query environment (the
/// preference + capability feature values serval's `mark-ik/stylo` fork exposes
/// to `@media` evaluation beyond the upstream Servo set). The whole
/// [`MediaEnvironment`] is set atomically, so individual features never clobber
/// each other (see the parity plan's M2).
fn make_device_with_prefs(
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    env: MediaEnvironment,
) -> Device {
    make_device_with_environment(
        viewport,
        quirks,
        env,
        PointerCapabilities::default(),
        PointerCapabilities::default(),
    )
}

fn make_device_with_environment(
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    env: MediaEnvironment,
    primary_pointer_capabilities: PointerCapabilities,
    all_pointer_capabilities: PointerCapabilities,
) -> Device {
    let mut device = Device::new(
        MediaType::screen(),
        quirks,
        euclid::Size2D::from_untyped(viewport),
        euclid::Size2D::from_untyped(viewport),
        euclid::Scale::new(1.0),
        Box::new(SkrifaFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        env.prefers_color_scheme,
        primary_pointer_capabilities,
        all_pointer_capabilities,
    );
    device.set_media_environment(env);
    device
}

/// Re-evaluate the Stylist's media queries under a new [`MediaEnvironment`]: swap
/// the Device, mark the origins whose applicable rules changed dirty, and flush.
/// The rule TREE is untouched — only rule applicability recomputes — so
/// `StrongRuleNode`s held by a persistent `StylePlane` stay valid, which is what
/// lets a preference flip restyle a live session instead of rebuilding it (the
/// persistent-Stylist invariant). This is the atomic setter; the per-preference
/// helpers below are read-modify-write shims over it.
pub fn set_stylist_media_env(
    stylist: &mut Stylist,
    lock: &SharedRwLock,
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    env: MediaEnvironment,
) {
    let primary_pointer_capabilities = stylist.device().primary_pointer_capabilities();
    let all_pointer_capabilities = stylist.device().all_pointer_capabilities();
    let device = make_device_with_environment(
        viewport,
        quirks,
        env,
        primary_pointer_capabilities,
        all_pointer_capabilities,
    );
    replace_stylist_device(stylist, lock, device);
}

/// Re-evaluate the Stylist's media queries under new pointer capabilities while
/// preserving every preference and non-pointer capability in the current
/// [`MediaEnvironment`]. v0.19 owns pointer/hover state directly on `Device`.
pub fn set_stylist_pointer_capabilities(
    stylist: &mut Stylist,
    lock: &SharedRwLock,
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    primary_pointer_capabilities: PointerCapabilities,
    all_pointer_capabilities: PointerCapabilities,
) {
    let env = stylist.device().media_environment();
    let device = make_device_with_environment(
        viewport,
        quirks,
        env,
        primary_pointer_capabilities,
        all_pointer_capabilities,
    );
    replace_stylist_device(stylist, lock, device);
}

fn replace_stylist_device(stylist: &mut Stylist, lock: &SharedRwLock, device: Device) {
    let read = lock.read();
    let guards = StylesheetGuards {
        author: &read,
        ua_or_user: &read,
    };
    let origins = stylist.set_device(device, &guards);
    stylist.force_stylesheet_origins_dirty(origins);
    stylist.flush(&guards);
}

/// Re-evaluate under a new `prefers-color-scheme` (W3C adoption plan P3).
/// Read-modify-write over the Stylist's current [`MediaEnvironment`], so the
/// other preferences (reduced-motion, …) are preserved, not clobbered.
pub fn set_stylist_color_scheme(
    stylist: &mut Stylist,
    lock: &SharedRwLock,
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    dark: bool,
) {
    let mut env = stylist.device().media_environment();
    env.prefers_color_scheme = if dark {
        PrefersColorScheme::Dark
    } else {
        PrefersColorScheme::Light
    };
    set_stylist_media_env(stylist, lock, viewport, quirks, env);
}

/// Re-evaluate under a new `prefers-reduced-motion` preference. Read-modify-write
/// over the current [`MediaEnvironment`], preserving the color scheme and other
/// preferences.
pub fn set_stylist_reduced_motion(
    stylist: &mut Stylist,
    lock: &SharedRwLock,
    viewport: euclid::default::Size2D<f32>,
    quirks: QuirksMode,
    reduce: bool,
) {
    let mut env = stylist.device().media_environment();
    env.prefers_reduced_motion = if reduce {
        PrefersReducedMotion::Reduce
    } else {
        PrefersReducedMotion::NoPreference
    };
    set_stylist_media_env(stylist, lock, viewport, quirks, env);
}

/// Parse a CSS media query string and evaluate it against `stylist`'s device —
/// the engine side of `window.matchMedia`. Returns the *serialized* (normalized)
/// query and whether it currently matches. An invalid or unknown query
/// serializes to `"not all"` and never matches (CSS media-query error handling),
/// which is exactly what `matchMedia().media` / `.matches` expose to script and
/// what the WPT `query_should_be_known` / `query_should_be_unknown` helpers key
/// on. `@custom-media` is not resolved (`CustomMediaEvaluator::none`).
pub fn evaluate_media_query(stylist: &Stylist, query: &str) -> (String, bool) {
    let url_data = make_url_data(None);
    let quirks_mode = stylist.quirks_mode();
    let context = ParserContext::new(
        Origin::Author,
        &url_data,
        None,
        ParsingMode::DEFAULT,
        quirks_mode,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    let mut input = ParserInput::new(query);
    let media_list = MediaList::parse(&context, &mut Parser::new(&mut input));
    let serialized = media_list.to_css_string();
    let mut custom = CustomMediaEvaluator::none();
    let matches = media_list.evaluate(stylist.device(), quirks_mode, &mut custom);
    (serialized, matches)
}

/// A self-contained media-query evaluator: owns a `Stylist` at a fixed viewport
/// with the default media environment, for hosts that need `matchMedia` without
/// a full layout session (e.g. the WPT testharness runner). Build once, evaluate
/// many.
pub struct MediaQueryEvaluator {
    stylist: Stylist,
    // Keeps the stylist's `SharedRwLock` alive alongside it.
    _lock: SharedRwLock,
}

impl MediaQueryEvaluator {
    /// Build an evaluator with a `width`x`height` (CSS px) viewport and no
    /// author sheets (only the device matters for media evaluation).
    pub fn new(width: f32, height: f32) -> Self {
        let lock = SharedRwLock::new();
        let stylist = build_stylist(
            euclid::Size2D::new(width, height),
            &[],
            None,
            &lock,
            QuirksMode::NoQuirks,
        );
        Self { stylist, _lock: lock }
    }

    /// Evaluate a media query string; returns the serialized query and whether
    /// it matches. See [`evaluate_media_query`].
    pub fn evaluate(&self, query: &str) -> (String, bool) {
        evaluate_media_query(&self.stylist, query)
    }
}

/// Run Stylo's cascade over `dom`, populating `plane` with `ElementData`
/// for every element.
///
/// Sequential (no rayon pool). `stylesheets` is a slice of CSS source
/// strings to load as UA-origin sheets before the cascade runs;
/// pass `&[]` for empty-stylist behavior (every element receives
/// Stylo's default cascaded values).
///
/// `base_url` is the document's URL, which relative `url()` references
/// in the stylesheets resolve against (e.g. `Some("file:///…/page.html")`
/// so `url(support/x.png)` resolves to a real file). Pass `None` when
/// the document has no base (sheet-less or data-URI-only content); under
/// `None`, relative `url()`s do not resolve.
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
    base_url: Option<&str>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    // §0 cascade phase-timing instrumentation (env-gated, native-only; off by
    // default, so it never touches the wasm path where `Instant` panics). Set
    // SERVAL_LAYOUT_TIMING to split the cascade cost into its sub-phases —
    // mirrors the `[layout-timing]` split in `box_tree::layout_via_box_tree`,
    // drilling the ~30 ms cascade phase to bound the parallel-cascade win (which
    // can only touch the Rayon-parallelizable `traverse_dom`, not the serial
    // Stylist build / snapshot setup). See mere design_docs
    // `2026-06-19_cross_platform_parallelism_strategy.md` §0 and serval
    // `docs/2026-06-13_parallel_cascade_scope.md`.
    let timing = std::env::var_os("SERVAL_LAYOUT_TIMING").is_some();

    // One-shot: a throwaway Stylist (its rule tree dies with this call). Sound
    // because a full cascade builds fresh rule nodes and never reuses a prior
    // pass's — only the incremental replacement path needs a persistent tree.
    let lock = plane.shared_lock().clone();
    let quirks = selectors_quirks_mode(dom.quirks_mode());

    // Sub-phase (1): parse the UA + author sheets and build/flush the `Stylist`
    // (device + rule tree). Serial setup — Stylo's parallel mode would NOT touch
    // it, so this slice is outside any thread-parallelism ceiling.
    let t = timing.then(std::time::Instant::now);
    let stylist = build_stylist(viewport, stylesheets, base_url, &lock, quirks);
    if let Some(t) = t {
        eprintln!(
            "[cascade-timing] stylist-build  {:>9.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    // Sub-phases (2)-(5): populate + inline-style parse + the `traverse_dom`
    // recalc + lazy `::marker` resolution + rule-tree GC. `cascade_traverse`
    // prints each under the same env var.
    cascade_traverse(dom, plane, &stylist, base_url, None);
    populate_contain_intrinsic_overrides(dom, plane, stylesheets);
}

/// Build + flush a [`Stylist`] for `viewport`, the baseline UA stylesheet
/// (`ua_defaults::UA_DEFAULTS`), and the given author `stylesheets`, all wrapped
/// under `lock`.
///
/// The returned `Stylist` owns its `Device` and `RuleTree`. Reuse it across
/// cascade passes ([`IncrementalLayout`] keeps one for its whole life) — do NOT
/// rebuild it per pass: `ElementData` holds `StrongRuleNode`s into its tree, and
/// dropping the `Stylist` tears down the tree's free list, so any surviving rule
/// node becomes a use-after-free.
///
/// `lock` must be the same `SharedRwLock` the plane wraps its inline-style blocks
/// under (the plane's `shared_lock()`), so the cascade's guards can read both the
/// sheets here and those inline blocks (`same_lock_as`).
/// Enable the CSS properties Stylo gates behind servo's `layout.unimplemented`
/// pref. That pref is servo's catch-all for properties *its* layout never did;
/// serval has its own, more complete layout, so the gate is servo's policy, not
/// serval's. Enabling lets the cascade *parse* them (`text-overflow`,
/// `user-select`, `backdrop-filter`, `contain`, counters, `mask-image`, `zoom`,
/// …); serval only changes rendering where it actually reads one
/// (`text-overflow` today), so the rest are computed-but-unused until serval
/// grows support. Set once — the pref store is a process-global shared with
/// Stylo, and the parse-time check reads it.
fn enable_serval_properties() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        stylo_static_prefs::set_pref!("layout.unimplemented", true);
        // CSS Grid track properties (`grid-template-columns/rows`, `grid-auto-*`,
        // `grid-row/column`) are gated behind servo's `layout.grid.enabled`,
        // off by default because *servo's* layout never implemented grid. serval
        // dispatches `display: grid` to Taffy's grid algorithm (`box_tree`), so
        // the gate is servo's policy, not serval's — enable it so the cascade
        // keeps the authored track lists instead of dropping them to `None`.
        stylo_static_prefs::set_pref!("layout.grid.enabled", true);
    });
}

pub fn build_stylist(
    viewport: euclid::default::Size2D<f32>,
    stylesheets: &[&str],
    base_url: Option<&str>,
    lock: &SharedRwLock,
    quirks: QuirksMode,
) -> Stylist {
    enable_serval_properties();
    let url_data = make_url_data(base_url);
    let device = make_device(viewport, quirks);
    let mut stylist = Stylist::new(device, quirks);
    let read = lock.read();
    // Prepend the baseline UA stylesheet (`<html>`/`<body>` → block + fill the
    // viewport; structural block elements default to `display:block`) at
    // UserAgent origin, then the page sheets at Author origin, so the cascade
    // orders origins correctly (Author wins over UA for normal declarations; UA
    // `!important` wins over Author normal, per CSS 2.1 §6.4.1). The Stylist
    // resolves rule indices during flush, so all sheets must be present first.
    let ua_sheet = parse_stylesheet(
        crate::ua_defaults::UA_DEFAULTS,
        Origin::UserAgent,
        lock,
        &url_data,
        quirks,
    );
    stylist.append_stylesheet(ua_sheet, &read);
    for css in stylesheets {
        let sheet = parse_stylesheet(css, Origin::Author, lock, &url_data, quirks);
        stylist.append_stylesheet(sheet, &read);
    }
    let guards = StylesheetGuards {
        author: &read,
        ua_or_user: &read,
    };
    stylist.flush(&guards);
    stylist
}

/// Initial full cascade over a caller-owned (persistent) [`Stylist`].
///
/// [`IncrementalLayout::new`] uses this for its first cascade so the rule tree
/// the incremental passes later reuse is the one already referenced by the
/// `ElementData` this populates. `base_url` is `None` (incremental sessions have
/// no document base yet; same as the prior behaviour).
pub fn run_cascade_with_stylist<D>(dom: &D, plane: &mut StylePlane<D::NodeId>, stylist: &Stylist)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    cascade_traverse(dom, plane, stylist, None, None);
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
    stylist: &Stylist,
    mutations: &[layout_dom_api::DomMutation<D::NodeId>],
) -> RestyleOutcome
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use html5ever::local_name;
    use layout_dom_api::DomMutation;
    use style::invalidation::element::restyle_hints::RestyleHint;

    // Clear stale damage so the post-restyle aggregate reflects only what
    // this pass restyled.
    plane.reset_damage();

    let snapshots = crate::snapshot::build_snapshot_map(dom, mutations);

    // Per attribute-changed element, before the traversal:
    //
    // 1. **Reset `handled_snapshot`.** Stylo treats this bit as per-traversal
    //    state (a snapshot is consumed once), but the entry persists it across
    //    `apply()` calls. A stale `true` from a prior frame makes the invalidator
    //    skip this pass's snapshot → no invalidation → no restyle → the change is
    //    dropped. Clearing it each pass is what makes *repeated* incremental
    //    restyle work (prereq B).
    // 2. **Hint a `style`-attribute change with `RESTYLE_STYLE_ATTRIBUTE`**
    //    (the cheap replacement path). Snapshot invalidation only covers
    //    selector-matching attrs (id/class/`[attr]`); an inline-`style` change is
    //    otherwise never re-applied on the incremental path (the full
    //    `run_cascade` re-parses inline styles, so this is purely an
    //    incremental-path gap). This hint drives Stylo's
    //    `CascadeWithReplacements`: it reuses the element's prior primary rule
    //    node (held on its persistent `ElementData`) and swaps only the
    //    style-attribute cascade level — re-reading the re-parsed inline block via
    //    `style_attribute()` — instead of re-matching selectors (prereq A).
    //
    //    This is sound ONLY because `stylist` is persistent across passes
    //    ([`IncrementalLayout`] owns it): the reused node and
    //    `stylist.rule_tree()` are the same tree, so `update_rule_at_level` walks
    //    a live node. (An earlier cut built a fresh `Stylist` per pass and used
    //    this hint — the reused node dangled into the dropped prior tree, a
    //    use-after-free that surfaced as parallel-only heap corruption; the
    //    persistent Stylist is exactly what makes the cheap path safe.)
    //
    //    The hint MUST be set alone — no `RESTYLE_SELF`/`RESTYLE_DESCENDANTS`, or
    //    Stylo's `restyle_kind` routes to `MatchAndCascade` (re-incurring per-frame
    //    selector matching) and a `debug_assert` in `replace_rules_internal` fires.
    // 3. **Mark the dirty path** on every ancestor so the traversal descends far
    //    enough for the element's parent to process its snapshot (Stylo processes
    //    a child's snapshot while traversing the parent — see `traversal.rs`).
    //
    // Cell access through `&` entries; the hint needs `mutate_data` (see SAFETY).
    for m in mutations {
        if let DomMutation::AttributeChanged { node, name, .. } = m {
            if let Some(entry) = plane.get(*node) {
                entry.handled_snapshot.set(false);
                if name.local == local_name!("style") {
                    // SAFETY: not inside a cascade traversal (single-threaded, no
                    // live borrow of this entry's `ElementData`) — same invariant
                    // as `restyle_structural`.
                    if let Some(mut data) = unsafe { entry.mutate_data() } {
                        data.hint.insert(RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
                    }
                }
            }
            let mut cur = dom.parent(*node);
            while let Some(ancestor) = cur {
                if let Some(entry) = plane.get(ancestor) {
                    entry.dirty_descendants.set(true);
                }
                cur = dom.parent(ancestor);
            }
        }
    }

    // base_url None: incremental restyle reuses the prior cascade's
    // resolved url()s; re-resolving relative refs here is a follow-up.
    cascade_traverse(dom, plane, stylist, None, Some(&snapshots));

    // Stylo stored each restyled element's RestyleDamage on its
    // ElementData during the traversal (via compute_style_difference).
    // RELAYOUT (the fully-saturated bit) means box geometry may have
    // changed → re-layout; lesser bits (REPAINT / stacking / overflow)
    // are paint-tier for serval's taffy-driven layout.
    let damage = plane.aggregate_damage();
    RestyleOutcome {
        needs_relayout: damage.contains(RestyleDamage::RELAYOUT),
        damage,
        restyled_elements: plane.damaged_entry_count(),
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
    /// The aggregate `RestyleDamage` union across every element restyled this
    /// batch. `needs_relayout` is `damage.contains(RELAYOUT)`; the full union
    /// lets a caller confirm *which* paint-tier bits were seen — e.g. that a
    /// `transform` change registered `RECALCULATE_OVERFLOW` rather than being a
    /// silent no-op that would also produce a (misleading) repaint-only result.
    pub damage: RestyleDamage,
    /// How many elements Stylo actually restyled in this pass.
    pub restyled_elements: usize,
}

// =============================================================================
// Interaction-state restyle (`:hover` / `:active` / `:focus` / `:focus-within`)
// =============================================================================

/// Set `bits` on `from` and every ancestor up to the document root.
fn add_interaction_chain<D>(
    dom: &D,
    desired: &mut FxHashMap<D::NodeId, ElementState>,
    from: D::NodeId,
    bits: ElementState,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut cur = Some(from);
    while let Some(id) = cur {
        *desired.entry(id).or_insert_with(ElementState::empty) |= bits;
        cur = dom.parent(id);
    }
}

/// Reverse-map a [`SourceNodeId`] (an opaque node id) to a `D::NodeId`.
/// O(n) over the DOM; the interaction snapshot resolves at most three ids per
/// input event, off the hot path.
fn resolve_source<D>(dom: &D, source: SourceNodeId) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        if dom.opaque_id(id) == source.0 {
            return Some(id);
        }
        queue.extend(dom.dom_children(id));
    }
    None
}

/// Resolve a host [`InteractionState`] to the per-node interaction
/// [`ElementState`] bits it implies, with CSS scoping: `:hover` / `:active` on
/// the target and every ancestor, `:focus` on the focused node only, and
/// `:focus-within` on the focused node and every ancestor.
fn interaction_desired<D>(dom: &D, state: &InteractionState) -> FxHashMap<D::NodeId, ElementState>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut desired: FxHashMap<D::NodeId, ElementState> = FxHashMap::default();
    if let Some(node) = state.hovered.and_then(|s| resolve_source(dom, s)) {
        add_interaction_chain(dom, &mut desired, node, ElementState::HOVER);
    }
    if let Some(node) = state.active.and_then(|s| resolve_source(dom, s)) {
        add_interaction_chain(dom, &mut desired, node, ElementState::ACTIVE);
    }
    if let Some(node) = state.focused.and_then(|s| resolve_source(dom, s)) {
        *desired.entry(node).or_insert_with(ElementState::empty) |= ElementState::FOCUS;
        add_interaction_chain(dom, &mut desired, node, ElementState::FOCUS_WITHIN);
    }
    desired
}

/// Apply a host [`InteractionState`] to the plane's element state without
/// restyling. Use before an initial [`run_cascade`] (the cascade reads the
/// state as it matches selectors); use [`restyle_for_interaction`] for later
/// changes. Returns `(node, old_state)` for each node whose state changed.
pub fn apply_interaction<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    state: &InteractionState,
) -> Vec<(D::NodeId, ElementState)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let desired = interaction_desired(dom, state);
    plane.apply_interaction_bits(&desired)
}

/// Restyle for a host interaction change: apply the [`InteractionState`] to
/// element state, then run Stylo's state-change invalidation so only the
/// elements whose state-dependent selectors (`:hover` / `:active` / `:focus` /
/// `:focus-within`) are affected re-cascade. Reuses the persistent `stylist`
/// exactly like [`restyle_with_snapshots`]; returns whether the change needs a
/// relayout or is paint-only.
pub fn restyle_for_interaction<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    stylist: &Stylist,
    state: &InteractionState,
) -> RestyleOutcome
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    plane.reset_damage();

    let changed = apply_interaction(dom, plane, state);
    if changed.is_empty() {
        return RestyleOutcome::default();
    }

    let snapshots = crate::snapshot::state_snapshot_map(dom, &changed);

    // Same per-changed-element prep as the attribute path: clear the stale
    // `handled_snapshot` so this pass's snapshot is consumed, and mark the dirty
    // path up every ancestor so the traversal descends to the element's parent
    // (Stylo processes a child's snapshot while traversing the parent).
    for (node, _) in &changed {
        if let Some(entry) = plane.get(*node) {
            entry.handled_snapshot.set(false);
        }
        let mut cur = dom.parent(*node);
        while let Some(ancestor) = cur {
            if let Some(entry) = plane.get(ancestor) {
                entry.dirty_descendants.set(true);
            }
            cur = dom.parent(ancestor);
        }
    }

    cascade_traverse(dom, plane, stylist, None, Some(&snapshots));

    let damage = plane.aggregate_damage();
    RestyleOutcome {
        needs_relayout: damage.contains(RestyleDamage::RELAYOUT),
        damage,
        restyled_elements: plane.damaged_entry_count(),
    }
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
    stylist: &Stylist,
    roots: &[D::NodeId],
) -> RestyleOutcome
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use style::invalidation::element::restyle_hints::RestyleHint;

    plane.reset_damage();
    let mut restyled_elements = 0usize;

    for &root in roots {
        restyled_elements += count_element_subtree(dom, root);
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

    // base_url None: structural restyle reuses prior resolved url()s
    // (same follow-up as the snapshot path).
    cascade_traverse(dom, plane, stylist, None, None);

    let damage = plane.aggregate_damage();
    RestyleOutcome {
        needs_relayout: damage.contains(RestyleDamage::RELAYOUT),
        damage,
        restyled_elements,
    }
}

/// Animation tick: re-cascade every element with a live CSS transition against
/// the plane's current animation clock, so `transition_rule` re-reads the
/// interpolated declarations (`CascadeOrigin::Transitions`) at the new time.
///
/// No state flipping: `PropertyAnimation::calculate_value` clamps progress to
/// `[0, 1]`, so a transition already past its end interpolates to the final
/// (after-change) value without being marked `Finished`. The lifecycle states
/// and event emission live entirely in
/// [`crate::transition_events::harvest_transition_events`], which the host runs
/// after this tick; keeping them out of the cascade avoids Stylo's
/// `process_animations` silently dropping a just-finished transition before its
/// `transitionend` is harvested. `RESTYLE_SELF`, not `RESTYLE_CSS_TRANSITIONS`:
/// the animation-only replacement hint assumes Gecko's separate animation-only
/// traversal, which serval does not run.
///
/// Call after [`StylePlane::set_animation_clock`]. A cheap no-op (empty damage)
/// when nothing is animating. Per-owner by construction: the animating set is
/// this plane's (this document's); ticking one document never touches another.
pub fn restyle_for_animation_tick<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    stylist: &Stylist,
) -> RestyleOutcome
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use style::invalidation::element::restyle_hints::RestyleHint;

    let animating: std::collections::HashSet<usize> = {
        let sets = plane.animations().sets.read();
        sets.keys().map(|k| k.node.0).collect()
    };
    plane.reset_damage();
    if animating.is_empty() {
        return RestyleOutcome {
            needs_relayout: false,
            damage: RestyleDamage::empty(),
            restyled_elements: 0,
        };
    }

    // Walk the document, hinting each animating element for re-match+cascade
    // (ancestors get `dirty_descendants` so the traversal descends to them).
    // O(document) per tick; comparable to the interaction restyle's walk.
    let mut restyled_elements = 0usize;
    let mut stack = vec![dom.document()];
    while let Some(node) = stack.pop() {
        for child in dom.dom_children(node) {
            stack.push(child);
        }
        if !matches!(dom.kind(node), layout_dom_api::NodeKind::Element) {
            continue;
        }
        if !animating.contains(&(dom.opaque_id(node) as usize)) {
            continue;
        }
        if let Some(entry) = plane.get(node) {
            // SAFETY: not inside a cascade traversal (single-threaded, no live
            // borrow of this entry's ElementData).
            if let Some(mut data) = unsafe { entry.mutate_data() } {
                data.hint.insert(RestyleHint::RESTYLE_SELF);
                restyled_elements += 1;
            }
        }
        let mut cur = dom.parent(node);
        while let Some(ancestor) = cur {
            if let Some(entry) = plane.get(ancestor) {
                entry.dirty_descendants.set(true);
            }
            cur = dom.parent(ancestor);
        }
    }

    cascade_traverse(dom, plane, stylist, None, None);

    let damage = plane.aggregate_damage();
    RestyleOutcome {
        needs_relayout: damage.contains(RestyleDamage::RELAYOUT),
        damage,
        restyled_elements,
    }
}

/// Shared cascade traversal over a caller-owned [`Stylist`]. `snapshots =
/// None` is a full cascade (every element styled because none has
/// `ElementData` yet); `Some` is the incremental restyle path (existing
/// data + snapshots drive Stylo's invalidator to recompute only the
/// affected elements).
///
/// `stylist` is borrowed, not built: it carries the device + UA/author
/// sheets + the rule tree. The rule tree must be the SAME instance across
/// every pass over a given plane — `ElementData` holds `StrongRuleNode`s
/// into it, and the incremental replacement path
/// ([`RestyleHint::RESTYLE_STYLE_ATTRIBUTE`]) reuses them; a rule node from
/// a dropped tree is a use-after-free. Callers therefore hand in a
/// persistent `Stylist` ([`IncrementalLayout`] owns one) or a throwaway one
/// for a one-shot full cascade ([`run_cascade`]).
fn cascade_traverse<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    stylist: &Stylist,
    base_url: Option<&str>,
    snapshots: Option<&SnapshotMap>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    // §0 cascade phase-timing (env-gated, native-only; see `run_cascade`). Steps
    // 1-4 below are the serial cascade *setup* — entry population, inline-style
    // parse, lock/guard + context wiring — that Stylo's parallel mode does NOT
    // touch; timed as one "setup" slice against the parallelizable `traverse_dom`.
    let timing = std::env::var_os("SERVAL_LAYOUT_TIMING").is_some();
    let t_setup = timing.then(std::time::Instant::now);

    let url_data = make_url_data(base_url);
    // Pre-populate StylePlane entries for every element. The cascade's
    // ensure_data() requires entries to exist (cascade-orchestration
    // contract; see StyleNodeRef::ensure_data documentation).
    plane.populate_for_elements(dom);

    // 1. Enter Stylo's layout-thread state. Required by ThreadSafeBindings
    //    checks scattered through the cascade.
    thread_state::enter(ThreadState::LAYOUT);

    // 2. Lock + guard setup. The plane's STABLE SharedRwLock (cloned shares the
    //    same lock) — not a fresh one per pass. `parse_inline_styles` (below)
    //    wraps each element's inline declaration block under this lock and stashes
    //    it on the plane, so the guards the cascade reads them back through must
    //    come from the *same* lock (else Stylo's `same_lock_as` assertion fires).
    //    The plane owning one lock for all the `Locked` data it holds keeps that
    //    invariant trivially, and is the precondition for a future
    //    persistent-Stylist optimization (rule-node reuse across passes; see the
    //    `restyle_subtree` note in `restyle_with_snapshots`).
    let lock = plane.shared_lock().clone();
    let read = lock.read();
    let guards = StylesheetGuards {
        author: &read,
        ua_or_user: &read,
    };

    // Parse inline `style="…"` attributes into the plane now that the lock
    // exists (to wrap each block) and before the traversal reads them back via
    // the adapter's `style_attribute()`. Re-run every pass: the replacement path
    // reads `style_attribute()` fresh, so the (re-parsed) block must be current,
    // and it is re-wrapped under the plane's stable lock so the guards' read
    // matches (`same_lock_as`). The stylesheets, by contrast, live on the
    // caller-owned `stylist` and are NOT re-parsed here.
    parse_inline_styles(dom, plane, &lock, &url_data);

    // 3. SharedStyleContext bundles everything the cascade needs. For a
    //    full cascade the snapshot map is empty; for incremental restyle
    //    it carries the pre-mutation snapshots Stylo's invalidator reads.
    let empty_snapshots = SnapshotMap::new();
    let snapshot_map = snapshots.unwrap_or(&empty_snapshots);
    // The plane's persistent animation set (clone shares the Arc) + clock, so
    // transitions started by this pass survive into the next and interpolated
    // declarations resolve against the session's time. Stylo's Servo-mode
    // `finish_restyle` -> `process_animations` drives start/cancel per element
    // inside the traversal; nothing here needs to know about individual
    // transitions.
    let animations = plane.animations().clone();
    let registered_painters = NoOpRegisteredPainters;

    let context = SharedStyleContext {
        traversal_flags: TraversalFlags::empty(),
        stylist,
        options: GLOBAL_STYLE_DATA.options.clone(),
        guards,
        visited_styles_enabled: false,
        animations,
        current_time_for_animations: plane.animation_clock(),
        snapshot_map,
        registered_speculative_painters: &registered_painters,
    };

    // 4. Enter cascade TLS context. StyleNodeRef methods that need
    //    `dom`/`plane`/`shared_lock`/snapshot access read from this slot;
    //    outside the guard they panic. `has_snapshot` consults the same
    //    map (None ⇒ always false ⇒ full-cascade behavior).
    let plane_ref: &StylePlane<D::NodeId> = &*plane;
    let _guard = CascadeGuard::<D>::enter(dom, plane_ref, &lock, snapshots);

    if let Some(t) = t_setup {
        eprintln!(
            "[cascade-timing] setup         {:>9.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    // 5. Drive the traversal. RecalcStyle's process_preorder calls
    //    recalc_style_at on each element, populating its ElementData
    //    in the StylePlane (via UnsafeCell interior mutability per entry).
    //    This is the selector-matching + cascade + computed-value application —
    //    the slice Stylo CAN parallelize (its Rayon pool / parallel traversal),
    //    so the parallel-cascade win is bounded by *this* phase, not the cascade
    //    as a whole. We pass `None` for the pool (serial) — measurement only.
    let t_recalc = timing.then(std::time::Instant::now);
    // Traverse from EVERY document-level element, not just the first: a
    // host-built synthetic DOM (merecat's chrome layer, widget pools) has no
    // `<html>` wrapper and hangs several roots off the document; styling only
    // the first left its siblings at initial values (unstyled, unpositioned).
    // Parsed HTML has exactly one root element, so this loop runs once there.
    {
        let traverser = RecalcStyle::new(context);
        for root_id in dom.dom_children(dom.document()) {
            if !matches!(dom.kind(root_id), layout_dom_api::NodeKind::Element) {
                continue;
            }
            let root_element: StyleNodeRef<'_, D> = StyleNodeRef::new(root_id);
            let token = RecalcStyle::pre_traverse(root_element, &traverser.context);
            if token.should_traverse() {
                driver::traverse_dom(&traverser, token, None);
            }
        }
    }
    if let Some(t) = t_recalc {
        eprintln!(
            "[cascade-timing] traverse_dom   {:>9.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    // 5b. Resolve lazy `::marker` pseudo styles (not in the eager pseudo map)
    //     for list items, against each item's just-cascaded primary style, while
    //     the TLS guard + Stylist are live. `context`'s guards moved into the
    //     traversal, so rebuild them from the still-open `read`. Collect here
    //     (the plane is borrowed immutably under the guard) and write after it
    //     drops.
    let marker_guards = StylesheetGuards {
        author: &read,
        ua_or_user: &read,
    };
    let t_marker = timing.then(std::time::Instant::now);
    let resolved_markers = collect_marker_styles(dom, &*plane, stylist, &marker_guards);
    if let Some(t) = t_marker {
        eprintln!(
            "[cascade-timing] marker-resolve {:>9.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    // 6. Drop guard (clears TLS), then exit thread state.
    drop(_guard);
    thread_state::exit(ThreadState::LAYOUT);

    // 6b. Write the resolved `::marker` styles (the plane is mutable again).
    //     Clear first so a removed `::marker` rule does not linger across passes.
    plane.clear_marker_styles();
    for (id, style) in resolved_markers {
        plane.set_marker_style(id, style);
    }

    // 7. GC the rule tree's free list. A persistent Stylist accumulates
    //    dropped rule nodes (e.g. each replaced style-attribute level) on a
    //    free list rather than freeing them eagerly; `maybe_gc` reclaims them
    //    once the count crosses Stylo's threshold. Safe here: the traversal is
    //    done and we are single-threaded (no other accessor of the tree). A
    //    no-op on a throwaway one-shot Stylist (nothing freed yet).
    let t_gc = timing.then(std::time::Instant::now);
    stylist.rule_tree().maybe_gc();
    if let Some(t) = t_gc {
        eprintln!(
            "[cascade-timing] rule-tree-gc   {:>9.3} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }
}

/// Probe-resolve the lazy `::marker` pseudo style for every `<li>`, against its
/// just-cascaded primary style, returning `(id, style)` for those that match a
/// `::marker` rule. Must be called inside the cascade's TLS [`CascadeGuard`]
/// scope (the lazy resolution invokes `TElement` methods on the element, which
/// read `dom`/`plane` from TLS) with guards rebuilt from the cascade's lock.
fn collect_marker_styles<D>(
    dom: &D,
    plane: &StylePlane<D::NodeId>,
    stylist: &Stylist,
    guards: &StylesheetGuards,
) -> Vec<(D::NodeId, ServoArc<ComputedValues>)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    use style::selector_parser::PseudoElement;
    use style::stylist::RuleInclusion;

    let mut out = Vec::new();
    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        let is_li = dom
            .element_name(id)
            .is_some_and(|n| n.local == html5ever::local_name!("li"));
        if is_li {
            // The item's primary style; clone the `Arc` out so the `borrow_data`
            // guard drops before the TLS-using lazy resolution below.
            let primary = plane
                .get(id)
                .and_then(|e| e.borrow_data())
                .map(|d| d.styles.primary().clone());
            if let Some(primary) = primary {
                let el = StyleNodeRef::<D>::new(id);
                if let Some(marker) = stylist.lazily_compute_pseudo_element_style(
                    guards,
                    el,
                    &PseudoElement::Marker,
                    RuleInclusion::All,
                    &primary,
                    true, // is_probe: returns None when no `::marker` rule matches
                    None,
                ) {
                    out.push((id, marker));
                }
            }
        }
        queue.extend(dom.dom_children(id));
    }
    out
}

/// Parse each element's inline `style="…"` attribute into an Author-origin
/// [`PropertyDeclarationBlock`](style::properties::PropertyDeclarationBlock),
/// wrap it under the cascade's `SharedRwLock`, and stash it on the element's
/// [`StyleEntry`](crate::style::StyleEntry). The stylo adapter's
/// `TElement::style_attribute` returns a borrow of it, so the cascade applies
/// inline declarations at the inline-style level (above author stylesheet
/// rules), matching the browser. Elements with no / empty `style` attribute are
/// left untouched. Walks the same DOM as `populate_for_elements`; kept a
/// separate pass because parsing needs the lock, which is created after that.
fn parse_inline_styles<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    lock: &SharedRwLock,
    url_data: &UrlExtraData,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::{LocalName, Namespace, ns};
    let no_ns: Namespace = ns!();
    let style_local = LocalName::from("style");
    let quirks = selectors_quirks_mode(dom.quirks_mode());

    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        if matches!(dom.kind(id), layout_dom_api::NodeKind::Element) {
            if let Some(css) = dom.attribute(id, &no_ns, &style_local) {
                if !css.trim().is_empty() {
                    let pdb = parse_style_attribute(
                        css,
                        url_data,
                        None, // no error reporter
                        quirks,
                        CssRuleType::Style,
                    );
                    plane.ensure_entry(id).inline_style = Some(ServoArc::new(lock.wrap(pdb)));
                }
            }
        }
        queue.extend(dom.dom_children(id));
    }
}

#[derive(Clone, Debug)]
struct SupplementalRule {
    selector: SimpleSelector,
    value: ContainIntrinsicOverride,
    order: usize,
}

#[derive(Clone, Debug)]
struct SimpleSelector {
    local: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    specificity: u16,
}

#[derive(Clone, Copy)]
struct AxisCandidate {
    value: f32,
    specificity: u16,
    order: usize,
}

#[derive(Default)]
struct CascadedContainIntrinsic {
    width: Option<AxisCandidate>,
    height: Option<AxisCandidate>,
}

fn populate_contain_intrinsic_overrides<D>(
    dom: &D,
    plane: &mut StylePlane<D::NodeId>,
    stylesheets: &[&str],
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::{LocalName, Namespace, ns};

    plane.clear_contain_intrinsic_overrides();
    let no_ns: Namespace = ns!();
    let style_local = LocalName::from("style");
    let has_inline = {
        let mut queue = vec![dom.document()];
        let mut found = false;
        while let Some(id) = queue.pop() {
            if matches!(dom.kind(id), layout_dom_api::NodeKind::Element)
                && dom
                    .attribute(id, &no_ns, &style_local)
                    .is_some_and(|css| css.contains("contain-intrinsic"))
            {
                found = true;
                break;
            }
            queue.extend(dom.dom_children(id));
        }
        found
    };
    if !has_inline
        && !stylesheets
            .iter()
            .any(|css| css.contains("contain-intrinsic"))
    {
        return;
    }

    let mut rules = Vec::new();
    let mut order = 0;
    for css in stylesheets {
        collect_contain_intrinsic_rules(css, &mut order, &mut rules);
    }

    let mut queue = vec![dom.document()];
    while let Some(id) = queue.pop() {
        if matches!(dom.kind(id), layout_dom_api::NodeKind::Element) {
            let mut cascaded = CascadedContainIntrinsic::default();
            for rule in &rules {
                if rule.selector.matches(dom, id) {
                    cascaded.apply(rule.value, rule.selector.specificity, rule.order);
                }
            }
            if let Some(css) = dom.attribute(id, &no_ns, &style_local) {
                if css.contains("contain-intrinsic") {
                    cascaded.apply(parse_contain_intrinsic_declarations(css), u16::MAX, order);
                    order += 1;
                }
            }
            let value = cascaded.finish();
            if value.width.is_some() || value.height.is_some() {
                plane.set_contain_intrinsic_override(id, value);
            }
        }
        queue.extend(dom.dom_children(id));
    }
}

fn collect_contain_intrinsic_rules(
    css: &str,
    order: &mut usize,
    rules: &mut Vec<SupplementalRule>,
) {
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let selector_text = &rest[..open];
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let body = &after_open[..close];
        rest = &after_open[close + 1..];
        if selector_text.trim_start().starts_with('@') || !body.contains("contain-intrinsic") {
            continue;
        }
        let value = parse_contain_intrinsic_declarations(body);
        if value.width.is_none() && value.height.is_none() {
            continue;
        }
        for selector in selector_text.split(',') {
            if let Some(selector) = SimpleSelector::parse(selector) {
                rules.push(SupplementalRule {
                    selector,
                    value,
                    order: *order,
                });
                *order += 1;
            }
        }
    }
}

fn parse_contain_intrinsic_declarations(css: &str) -> ContainIntrinsicOverride {
    let mut value = ContainIntrinsicOverride::default();
    for decl in css.split(';') {
        let Some((name, raw_value)) = decl.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "contain-intrinsic-width" => {
                value.width = first_px_length(raw_value);
            },
            "contain-intrinsic-height" => {
                value.height = first_px_length(raw_value);
            },
            "contain-intrinsic-size" => {
                let lengths = px_lengths(raw_value);
                if let Some(width) = lengths.first().copied() {
                    value.width = Some(width);
                    value.height = Some(lengths.get(1).copied().unwrap_or(width));
                }
            },
            _ => {},
        }
    }
    value
}

fn first_px_length(value: &str) -> Option<f32> {
    px_lengths(value).into_iter().next()
}

fn px_lengths(value: &str) -> Vec<f32> {
    use cssparser::{Parser, ParserInput, Token};

    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut out = Vec::new();
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        match token {
            Token::Dimension { value, unit, .. } if unit.eq_ignore_ascii_case("px") => {
                if *value >= 0.0 && value.is_finite() {
                    out.push(*value);
                }
            },
            Token::Number { value, .. } if *value == 0.0 => out.push(0.0),
            _ => {},
        }
    }
    out
}

impl SimpleSelector {
    fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty()
            || raw
                .chars()
                .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '>' | '+' | '~' | '[' | ':'))
        {
            return None;
        }

        let mut local = None;
        let mut id = None;
        let mut classes = Vec::new();
        let mut i = 0;
        let bytes = raw.as_bytes();

        if bytes.get(i) == Some(&b'*') {
            i += 1;
        } else if bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'-')
        {
            let start = i;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
            {
                i += 1;
            }
            local = Some(raw[start..i].to_ascii_lowercase());
        }

        while i < raw.len() {
            let marker = bytes[i];
            if marker != b'.' && marker != b'#' {
                return None;
            }
            i += 1;
            let start = i;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
            {
                i += 1;
            }
            if start == i {
                return None;
            }
            let ident = raw[start..i].to_string();
            if marker == b'.' {
                classes.push(ident);
            } else if id.replace(ident).is_some() {
                return None;
            }
        }

        let specificity = u16::from(local.is_some())
            + (classes.len() as u16 * 10)
            + u16::from(id.is_some()) * 100;
        Some(Self {
            local,
            id,
            classes,
            specificity,
        })
    }

    fn matches<D>(&self, dom: &D, id: D::NodeId) -> bool
    where
        D: LayoutDom,
    {
        use html5ever::{LocalName, Namespace, ns};
        let no_ns: Namespace = ns!();
        if let Some(local) = &self.local {
            let Some(name) = dom.element_name(id) else {
                return false;
            };
            if !name.local.as_ref().eq_ignore_ascii_case(local) {
                return false;
            }
        }
        if let Some(expected_id) = &self.id {
            let id_local = LocalName::from("id");
            if dom.attribute(id, &no_ns, &id_local) != Some(expected_id.as_str()) {
                return false;
            }
        }
        if !self.classes.is_empty() {
            let class_local = LocalName::from("class");
            let Some(class_attr) = dom.attribute(id, &no_ns, &class_local) else {
                return false;
            };
            for class in &self.classes {
                if !class_attr
                    .split_ascii_whitespace()
                    .any(|token| token == class)
                {
                    return false;
                }
            }
        }
        true
    }
}

impl CascadedContainIntrinsic {
    fn apply(&mut self, value: ContainIntrinsicOverride, specificity: u16, order: usize) {
        if let Some(width) = value.width {
            self.width = choose_axis(self.width, width, specificity, order);
        }
        if let Some(height) = value.height {
            self.height = choose_axis(self.height, height, specificity, order);
        }
    }

    fn finish(self) -> ContainIntrinsicOverride {
        ContainIntrinsicOverride {
            width: self.width.map(|axis| axis.value),
            height: self.height.map(|axis| axis.value),
        }
    }
}

fn choose_axis(
    old: Option<AxisCandidate>,
    value: f32,
    specificity: u16,
    order: usize,
) -> Option<AxisCandidate> {
    let new = AxisCandidate {
        value,
        specificity,
        order,
    };
    match old {
        Some(old) if old.specificity > specificity => Some(old),
        Some(old) if old.specificity == specificity && old.order > order => Some(old),
        _ => Some(new),
    }
}

/// Build the stylesheet base [`UrlExtraData`] that relative `url()`
/// references in CSS resolve against. `base_url` is the document's URL
/// (e.g. a `file://` URL for a local page, so `url(support/x.png)`
/// resolves to a real file); `None` falls back to an `about:`
/// placeholder, under which relative `url()`s do not resolve (the
/// pre-base-URL behavior — fine for data-URI-only / sheet-less tests).
fn make_url_data(base_url: Option<&str>) -> UrlExtraData {
    let url = base_url
        .and_then(|b| url::Url::parse(b).ok())
        .unwrap_or_else(|| {
            url::Url::parse("about:internal-stylesheet").expect("about: URL parses")
        });
    UrlExtraData::from(url)
}

/// Parse a single CSS source string into a `DocumentStyleSheet` at the given
/// cascade `origin` (UA defaults are `UserAgent`, page sheets are `Author`).
/// `url_data` is the base URL relative `url()`s resolve against (see
/// [`make_url_data`]). No loader or error reporter (synthetic
/// stylesheets don't @import; if they did we'd plumb a real loader).
fn parse_stylesheet(
    css: &str,
    origin: Origin,
    lock: &SharedRwLock,
    url_data: &UrlExtraData,
    quirks: QuirksMode,
) -> DocumentStyleSheet {
    let media = ServoArc::new(lock.wrap(MediaList::empty()));
    let sheet = Stylesheet::from_str(
        css,
        url_data.clone(),
        origin,
        media,
        lock.clone(),
        None, // stylesheet loader
        None, // error reporter
        quirks,
        AllowImportRules::Yes,
    );
    DocumentStyleSheet(ServoArc::new(sheet))
}

fn count_element_subtree<D: LayoutDom>(dom: &D, root: D::NodeId) -> usize {
    let mut count = usize::from(matches!(dom.kind(root), layout_dom_api::NodeKind::Element));
    for child in dom.dom_children(root) {
        count += count_element_subtree(dom, child);
    }
    count
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
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();

        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );

        // Every element should now have ElementData populated.
        let html_id = find_element(&document, local_name!("html")).expect("html exists");
        let body_id = find_element(&document, local_name!("body")).expect("body exists");
        let p_id = find_element(&document, local_name!("p")).expect("p exists");

        for (name, id) in [("html", html_id), ("body", body_id), ("p", p_id)] {
            let entry = plane
                .get(id)
                .unwrap_or_else(|| panic!("{name}: no StyleEntry"));
            assert!(
                entry.has_data(),
                "{name}: no ElementData populated by cascade"
            );
        }
    }

    /// Probe that loaded stylesheets actually apply to matched
    /// elements. The cascade runs with a UA-origin sheet that paints
    /// <body> red; we read `background_color` off the computed
    /// values and assert the sRGB components match.
    #[test]
    fn cascade_applies_loaded_stylesheet_to_matched_elements() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();

        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["body { background-color: rgb(255, 0, 0); }"],
            None,
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

    /// `:root` matches the document's root element. serval's root element
    /// (`<html>`) has the document NODE as its parent, not `None`, so the old
    /// `is_root = parent().is_none()` never fired and `:root` matched nothing
    /// (custom properties or rules put on `:root` silently did nothing — the
    /// strophe/woodshed "palette on `:root` renders black" bug). Cascading
    /// `:root { background-color: rgb(0,255,0) }` must colour `<html>`
    /// (background-color does not inherit, so only a real `:root` match paints it).
    #[test]
    fn root_pseudo_matches_the_document_root_element() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &[":root { background-color: rgb(0, 255, 0); }"],
            None,
        );
        let html_id = find_element(&document, local_name!("html")).expect("html exists");
        let entry = plane.get(html_id).expect("html StyleEntry exists");
        let data = entry.borrow_data().expect("html ElementData populated");
        let primary = data.styles.primary();
        let current_color = primary.get_inherited_text().color;
        let srgb = primary
            .get_background()
            .background_color
            .resolve_to_absolute(&current_color)
            .into_srgb_legacy();
        let [r, g, b, _a] = *srgb.raw_components();
        assert!(g > 0.99, ":root must match <html> and paint it green: g={g}");
        assert!(
            r < 0.01 && b < 0.01,
            ":root background is pure green: r={r} b={b}"
        );
    }

    /// Cascade origin ordering: an **author** declaration beats a UA default even
    /// at lower specificity. The UA sheet sets `strong { font-weight: bold }` (a
    /// type selector, specificity 0,0,1); an author `* { font-weight: normal }`
    /// (the universal selector, 0,0,0) still wins, because author origin outranks
    /// UA origin before specificity is consulted. (Before author sheets carried
    /// `Origin::Author`, both were UA-origin and the higher-specificity UA rule
    /// won — this test would have read `bold`.)
    #[test]
    fn author_origin_beats_ua_default_below_specificity() {
        let document = StaticDocument::parse("<html><body><strong>x</strong></body></html>");
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["* { font-weight: normal; }"],
            None,
        );
        let strong_id = find_element(&document, local_name!("strong")).expect("strong exists");
        let entry = plane.get(strong_id).expect("strong StyleEntry exists");
        let data = entry.borrow_data().expect("strong ElementData populated");
        let weight = data.styles.primary().get_font().font_weight.value();
        assert!(
            (weight - 400.0).abs() < 0.5,
            "author `* {{ font-weight: normal }}` beats UA `strong {{ bold }}`: got {weight}"
        );
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
            &[".highlight { background-color: rgb(0, 0, 255); } \
                 #title { color: rgb(0, 255, 0); }"],
            None,
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
            &["[data-state=\"on\"] { color: rgb(0, 255, 0); } \
                 [hidden] { color: rgb(0, 0, 255); }"],
            None,
        );

        let ps: Vec<_> = {
            let mut out = Vec::new();
            let mut q = vec![document.document()];
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("p"))
                {
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
        assert!(
            green(color_of::<StaticDocument>(&plane, ps[0])),
            "[data-state=on] → green"
        );
        assert!(
            blue(color_of::<StaticDocument>(&plane, ps[1])),
            "[hidden] → blue"
        );
        let c2 = color_of::<StaticDocument>(&plane, ps[2]);
        assert!(
            !green(c2) && !blue(c2),
            "data-state=off matches neither rule"
        );
    }

    /// State-backed pseudo-classes match against the element's
    /// `ElementState`: a `p:hover { color: red }` rule applies to the `<p>`
    /// whose state has `HOVER` set, not its sibling. This is the scaffold
    /// receipt that `match_non_ts_pseudo_class` reads element state (it was
    /// stubbed `false`); the host interaction layer sets the state.
    #[test]
    fn cascade_matches_hover_pseudo_class() {
        use stylo_dom::ElementState;

        let document = StaticDocument::parse("<html><body><p>A</p><p>B</p></body></html>");
        let ps: Vec<_> = {
            let mut out = Vec::new();
            let mut q = vec![document.document()];
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("p"))
                {
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
            None,
        );

        let hovered = color_of::<StaticDocument>(&plane, ps[0]);
        let plain = color_of::<StaticDocument>(&plane, ps[1]);
        assert!(
            hovered[0] > 0.99 && hovered[1] < 0.01,
            ":hover <p> should be red, got {hovered:?}"
        );
        assert!(
            plain[0] < 0.01,
            "non-hovered <p> should stay default, got {plain:?}"
        );
    }

    /// A host [`InteractionState`] hover drives a `:hover` restyle, and moving
    /// the hover reverts the old element and recolors the new one — through the
    /// minimal snapshot path, not a full re-cascade. (Item 1 done-condition.)
    #[test]
    fn interaction_hover_drives_restyle() {
        use engine_observables_api::{InteractionState, SourceNodeId};
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &["p:hover { color: rgb(255, 0, 0); }"];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        // html > body > (p, p)
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p0 = dom.create_element(html("p"));
        dom.append_child(body, p0);
        let p1 = dom.create_element(html("p"));
        dom.append_child(body, p1);

        let mut plane: StylePlane<_> = StylePlane::new();
        let stylist = cascade_persistent(&dom, &mut plane, SHEET);
        assert!(
            color_of::<ScriptedDom>(&plane, p0)[0] < 0.01,
            "p0 starts non-red"
        );

        // Hover p0 → red; p1 untouched.
        let hover0 = InteractionState {
            hovered: Some(SourceNodeId(dom.opaque_id(p0))),
            ..Default::default()
        };
        restyle_for_interaction(&dom, &mut plane, &stylist, &hover0);
        assert!(
            color_of::<ScriptedDom>(&plane, p0)[0] > 0.99,
            "hovered p0 → red"
        );
        assert!(
            color_of::<ScriptedDom>(&plane, p1)[0] < 0.01,
            "p1 stays default"
        );

        // Move hover to p1 → p1 red, p0 reverts.
        let hover1 = InteractionState {
            hovered: Some(SourceNodeId(dom.opaque_id(p1))),
            ..Default::default()
        };
        restyle_for_interaction(&dom, &mut plane, &stylist, &hover1);
        assert!(
            color_of::<ScriptedDom>(&plane, p1)[0] > 0.99,
            "now-hovered p1 → red"
        );
        assert!(
            color_of::<ScriptedDom>(&plane, p0)[0] < 0.01,
            "p0 reverts to default"
        );
    }

    /// The `prefers-reduced-motion` media feature (serval's `mark-ik/stylo`
    /// fork adds it to the Servo set) evaluates against the `Device` and
    /// re-evaluates live: at the default `no-preference` the no-preference rule
    /// wins; after `set_stylist_reduced_motion(true)` the `reduce` rule wins,
    /// with the rule tree left intact.
    #[test]
    fn prefers_reduced_motion_media_query_evaluates_and_reevaluates() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &[
            "@media (prefers-reduced-motion: reduce) { p { color: rgb(255, 0, 0); } } \
             @media (prefers-reduced-motion: no-preference) { p { color: rgb(0, 0, 255); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);

        // Default device: no-preference → blue.
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(
            c[2] > 0.99 && c[0] < 0.01,
            "no-preference default → blue, got {c:?}"
        );

        // Flip to reduce and re-evaluate media queries → red.
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        set_stylist_reduced_motion(
            &mut stylist,
            &lock,
            euclid::Size2D::new(800.0, 600.0),
            quirks,
            true,
        );
        // Force the elements to re-cascade under the swapped device (the media
        // re-evaluation dirties the stylist, but the persistent plane's cached
        // `ElementData` needs a restyle hint to actually recompute).
        restyle_structural(&dom, &mut plane, &stylist, &[h]);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(
            c[0] > 0.99 && c[2] < 0.01,
            "reduce → red after re-evaluation, got {c:?}"
        );
    }

    /// `evaluate_media_query` (the engine side of `matchMedia`) parses a query
    /// string, evaluates it against the stylist device, and serializes it:
    /// valid queries match or not per the viewport/env; an unknown feature
    /// serializes to `"not all"` and never matches.
    #[test]
    fn evaluate_media_query_matches_and_serializes() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        let html = |l: &str| QualName::new(None, ns!(html), l.into());
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let mut plane: StylePlane<_> = StylePlane::new();
        // cascade_persistent builds the stylist at an 800x600 viewport.
        let stylist = cascade_persistent(&dom, &mut plane, &[]);

        // Valid, matching (800 >= 500).
        let (media, matches) = evaluate_media_query(&stylist, "(min-width: 500px)");
        assert!(matches, "800px viewport matches min-width: 500px");
        assert_ne!(media, "not all", "valid query is not 'not all', got {media:?}");

        // Valid, not matching.
        let (_, m) = evaluate_media_query(&stylist, "(min-width: 900px)");
        assert!(!m, "800px viewport does not match min-width: 900px");

        // A fork-added feature evaluates too (default env is no-preference).
        let (_, m) = evaluate_media_query(&stylist, "(prefers-reduced-motion: no-preference)");
        assert!(m, "default env matches prefers-reduced-motion: no-preference");

        // An unknown feature is valid <general-enclosed> syntax: preserved on
        // serialization but evaluates to false (never matches).
        let (_, m) = evaluate_media_query(&stylist, "(totally-bogus-feature: 5)");
        assert!(!m, "unknown feature never matches");
    }

    /// Tier E app/engine-state media features (parity plan M5): display-mode
    /// and scripting evaluate against the `MediaEnvironment`. Neither matches
    /// the default (browser + enabled); each flips when the host reports a
    /// different state.
    #[test]
    fn tier_e_state_media_features_evaluate() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;
        use style::queries::values::{DisplayMode, Scripting};

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (display-mode: standalone) { p { color: rgb(255, 0, 0); } } \
             @media (scripting: none) { p { color: rgb(0, 255, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        // Default: display-mode browser, scripting enabled → neither rule → black.
        assert!(color_of::<ScriptedDom>(&plane, p)[0] < 0.01, "default → black");

        let check = |stylist: &mut Stylist, plane: &mut StylePlane<_>, env: MediaEnvironment| {
            set_stylist_media_env(stylist, &lock, vp, quirks, env);
            restyle_structural(&dom, plane, stylist, &[h]);
            color_of::<ScriptedDom>(plane, p)
        };

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            display_mode: DisplayMode::Standalone,
            ..MediaEnvironment::default()
        });
        assert!(c[0] > 0.99 && c[1] < 0.01, "display-mode: standalone → red, got {c:?}");

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            scripting: Scripting::None,
            ..MediaEnvironment::default()
        });
        assert!(c[1] > 0.99 && c[0] < 0.01, "scripting: none → green, got {c:?}");
    }

    /// Tier D device-capability media features (parity plan M4): pointer, hover,
    /// color-gamut (PartialOrd match), and update each evaluate against the
    /// `MediaEnvironment`. Flipping away from the default screen (fine pointer +
    /// hover + srgb + fast) proves the defaults too, so the responsive idiom
    /// `(hover: hover) and (pointer: fine)` holds by construction at the default.
    #[test]
    fn tier_d_capability_media_features_evaluate() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;
        use style::queries::values::{ColorGamut, Update};
        use style::servo::media_features::PointerCapabilities;

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (pointer: coarse) { p { color: rgb(255, 0, 0); } } \
             @media (hover: none) { p { color: rgb(0, 255, 0); } } \
             @media (color-gamut: p3) { p { color: rgb(0, 0, 255); } } \
             @media (update: slow) { p { color: rgb(255, 255, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        // Default screen: none of the "unusual capability" rules match → black.
        // (Confirms fine pointer, hover, srgb gamut, fast update.)
        assert!(color_of::<ScriptedDom>(&plane, p)[0] < 0.01, "default screen → black");

        let check = |stylist: &mut Stylist, plane: &mut StylePlane<_>, env: MediaEnvironment| {
            set_stylist_media_env(stylist, &lock, vp, quirks, env);
            restyle_structural(&dom, plane, stylist, &[h]);
            color_of::<ScriptedDom>(plane, p)
        };

        let check_pointers = |stylist: &mut Stylist,
                              plane: &mut StylePlane<_>,
                              primary: PointerCapabilities| {
            set_stylist_pointer_capabilities(stylist, &lock, vp, quirks, primary, primary);
            restyle_structural(&dom, plane, stylist, &[h]);
            color_of::<ScriptedDom>(plane, p)
        };

        // Primary pointer is coarse but can still hover (COARSE|HOVER) →
        // (pointer: coarse) matches; (hover: none) does not.
        let c = check_pointers(
            &mut stylist,
            &mut plane,
            PointerCapabilities::COARSE | PointerCapabilities::HOVER,
        );
        assert!(c[0] > 0.99 && c[1] < 0.01, "pointer: coarse → red, got {c:?}");

        // Primary pointer is fine but can't hover → (hover: none) matches.
        let c = check_pointers(&mut stylist, &mut plane, PointerCapabilities::FINE);
        assert!(c[1] > 0.99 && c[0] < 0.01, "hover: none → green, got {c:?}");

        // color-gamut: p3 matches when the device gamut is at least p3.
        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            color_gamut: ColorGamut::P3,
            ..MediaEnvironment::default()
        });
        assert!(c[2] > 0.99 && c[0] < 0.01, "color-gamut ≥ p3 → blue, got {c:?}");

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            update: Update::Slow,
            ..MediaEnvironment::default()
        });
        assert!(c[0] > 0.99 && c[1] > 0.99 && c[2] < 0.01, "update: slow → yellow, got {c:?}");
    }

    /// Multi-capability `any-pointer`: a hybrid device (touchscreen + mouse)
    /// reports BOTH coarse and fine for `any-pointer`, so a rule gated on
    /// `(any-pointer: coarse) and (any-pointer: fine)` matches — something a
    /// single-value pointer model could never express. The primary pointer
    /// stays the fine mouse, so `(pointer: coarse)` does not match.
    #[test]
    fn multi_capability_any_pointer_matches_coarse_and_fine() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;
        use style::servo::media_features::PointerCapabilities;

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (any-pointer: coarse) and (any-pointer: fine) { p { color: rgb(255, 0, 0); } } \
             @media (pointer: coarse) { p { color: rgb(0, 255, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        // Default (mouse only): any-pointer is fine, not coarse → combined rule
        // fails → black.
        assert!(color_of::<ScriptedDom>(&plane, p)[0] < 0.01, "mouse-only → black");

        // Hybrid: any-pointer is coarse + fine + hover; primary stays the fine
        // mouse. The combined any-pointer rule matches (red); (pointer: coarse)
        // does not (primary is fine).
        set_stylist_pointer_capabilities(
            &mut stylist,
            &lock,
            vp,
            quirks,
            PointerCapabilities::FINE | PointerCapabilities::HOVER,
            PointerCapabilities::COARSE
                | PointerCapabilities::FINE
                | PointerCapabilities::HOVER,
        );
        restyle_structural(&dom, &mut plane, &stylist, &[h]);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(
            c[0] > 0.99 && c[1] < 0.01,
            "hybrid device matches (any-pointer: coarse) AND (any-pointer: fine) → red, got {c:?}"
        );
    }

    /// Tier C accessibility media features (parity plan M3): prefers-contrast,
    /// prefers-reduced-transparency, inverted-colors each evaluate against the
    /// `MediaEnvironment`. Each feature drives a distinct color, so flipping one
    /// env field at a time isolates it. (forced-colors is covered separately,
    /// since its color-reverting confounds a color observable.)
    #[test]
    fn tier_c_accessibility_media_features_evaluate() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;
        use style::queries::values::{InvertedColors, PrefersContrast, PrefersReducedTransparency};

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (prefers-contrast: more) { p { color: rgb(255, 0, 0); } } \
             @media (prefers-reduced-transparency: reduce) { p { color: rgb(0, 255, 0); } } \
             @media (inverted-colors: inverted) { p { color: rgb(0, 0, 255); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        assert!(color_of::<ScriptedDom>(&plane, p)[0] < 0.01, "default → black");

        let check = |stylist: &mut Stylist, plane: &mut StylePlane<_>, env: MediaEnvironment| {
            set_stylist_media_env(stylist, &lock, vp, quirks, env);
            restyle_structural(&dom, plane, stylist, &[h]);
            color_of::<ScriptedDom>(plane, p)
        };

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            prefers_contrast: PrefersContrast::More,
            ..MediaEnvironment::default()
        });
        assert!(c[0] > 0.99 && c[1] < 0.01, "prefers-contrast: more → red, got {c:?}");

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            prefers_reduced_transparency: PrefersReducedTransparency::Reduce,
            ..MediaEnvironment::default()
        });
        assert!(c[1] > 0.99 && c[0] < 0.01, "reduced-transparency → green, got {c:?}");

        let c = check(&mut stylist, &mut plane, MediaEnvironment {
            inverted_colors: InvertedColors::Inverted,
            ..MediaEnvironment::default()
        });
        assert!(c[2] > 0.99 && c[0] < 0.01, "inverted-colors → blue, got {c:?}");
    }

    /// The `forced-colors` media feature evaluates. Observed via the `none`
    /// state (which upstream stylo's shared cascade does not color-revert): a
    /// `@media (forced-colors: none)` rule applies at the default env and stops
    /// applying once the host reports `active`. (Servo mode reverts colors when
    /// active but does *not* honor `forced-color-adjust: none` — that opt-out is
    /// `cfg(gecko)` in stylo — so the full forced-colors behavior stays deferred
    /// per the parity plan; only the query is wired here.)
    #[test]
    fn forced_colors_media_feature_evaluates() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;
        use style::values::specified::color::ForcedColors;

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (forced-colors: none) { p { color: rgb(0, 255, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        // Default env: forced-colors is none → the rule matches → green.
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(c[1] > 0.99, "forced-colors: none matches at default → green, got {c:?}");

        // Host reports active → the (forced-colors: none) rule no longer matches.
        set_stylist_media_env(&mut stylist, &lock, vp, quirks, MediaEnvironment {
            forced_colors: ForcedColors::Active,
            ..MediaEnvironment::default()
        });
        restyle_structural(&dom, &mut plane, &stylist, &[h]);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(c[1] < 0.99, "active → (forced-colors: none) stops matching, got {c:?}");
    }

    /// The `MediaEnvironment` consolidation (parity plan M2): setting one
    /// preference no longer clobbers another. Flip reduced-motion, then flip
    /// color-scheme, each through its own read-modify-write setter, and both
    /// `@media` queries still respond — reduced-motion survives the later
    /// color-scheme flip.
    #[test]
    fn media_environment_preferences_do_not_clobber() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        // Red iff reduce, green channel iff dark. A pixel that is red AND has a
        // green tint means both preferences are active at once.
        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (prefers-reduced-motion: reduce) { p { color: rgb(255, 0, 0); } } \
             @media (prefers-color-scheme: dark) and (prefers-reduced-motion: reduce) \
             { p { color: rgb(255, 128, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);

        let mut plane: StylePlane<_> = StylePlane::new();
        let mut stylist = cascade_persistent(&dom, &mut plane, SHEET);
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let vp = euclid::Size2D::new(800.0, 600.0);

        // Default (light, no-preference): black.
        assert!(color_of::<ScriptedDom>(&plane, p)[0] < 0.01, "default black");

        // Flip reduced-motion → red.
        set_stylist_reduced_motion(&mut stylist, &lock, vp, quirks, true);
        restyle_structural(&dom, &mut plane, &stylist, &[h]);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(c[0] > 0.99 && c[1] < 0.01, "reduce → pure red, got {c:?}");

        // Now flip color-scheme to dark. If reduced-motion were clobbered back to
        // no-preference, the combined rule would not apply and we'd fall back to
        // black. Instead both are active → orange (red with a green tint).
        set_stylist_color_scheme(&mut stylist, &lock, vp, quirks, true);
        restyle_structural(&dom, &mut plane, &stylist, &[h]);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(
            c[0] > 0.99 && c[1] > 0.4,
            "dark + reduce both active → orange (reduce not clobbered), got {c:?}"
        );
    }

    /// Tier A geometry media features (`height`, `orientation`, `aspect-ratio`)
    /// — all absent from upstream Servo, added in `mark-ik/stylo` — evaluate
    /// against the viewport and flip with it. A combined query proves all three
    /// at once: it matches a landscape 800x600 viewport and fails a portrait one.
    #[test]
    fn tier_a_geometry_media_features_evaluate() {
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] = &[
            "p { color: rgb(0, 0, 0); } \
             @media (min-height: 500px) and (orientation: landscape) and (min-aspect-ratio: 5/4) \
             { p { color: rgb(255, 0, 0); } }",
        ];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());
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

        // Landscape 800x600: height 600 ≥ 500, landscape, 4/3 ≥ 5/4 → red.
        let (dom, p) = build();
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut plane, euclid::Size2D::new(800.0, 600.0), SHEET, None);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(c[0] > 0.99, "landscape 800x600 → red, got {c:?}");

        // Portrait 600x800: orientation fails → default black.
        let (dom, p) = build();
        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(&dom, &mut plane, euclid::Size2D::new(600.0, 800.0), SHEET, None);
        let c = color_of::<ScriptedDom>(&plane, p);
        assert!(c[0] < 0.01, "portrait 600x800 → black (query fails), got {c:?}");
    }

    /// `:focus` matches only the focused element while `:focus-within` matches
    /// it *and its ancestors* — the host snapshot resolves both with correct
    /// CSS scoping.
    #[test]
    fn interaction_focus_within_walks_ancestors() {
        use engine_observables_api::{InteractionState, SourceNodeId};
        use html5ever::ns;
        use layout_dom_api::{LayoutDomMut, QualName};
        use serval_scripted_dom::ScriptedDom;

        const SHEET: &[&str] =
            &["div:focus-within { color: rgb(0, 255, 0); } p:focus { color: rgb(255, 0, 0); }"];
        let html = |l: &str| QualName::new(None, ns!(html), l.into());

        // html > body > div > p
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
        let stylist = cascade_persistent(&dom, &mut plane, SHEET);

        let focus = InteractionState {
            focused: Some(SourceNodeId(dom.opaque_id(p))),
            ..Default::default()
        };
        restyle_for_interaction(&dom, &mut plane, &stylist, &focus);

        let p_c = color_of::<ScriptedDom>(&plane, p);
        let div_c = color_of::<ScriptedDom>(&plane, div);
        assert!(
            p_c[0] > 0.99 && p_c[1] < 0.01,
            "focused <p> → red (:focus), got {p_c:?}"
        );
        assert!(
            div_c[1] > 0.99 && div_c[0] < 0.01,
            "ancestor <div> → green (:focus-within), got {div_c:?}"
        );
    }

    /// `:checked` matches a checked checkbox `<input>` (from its `checked`
    /// content attribute) but not an unchecked sibling.
    #[test]
    fn checked_attribute_matches_checked_pseudo() {
        let document = StaticDocument::parse(
            "<html><body><input type=\"checkbox\" checked><input type=\"checkbox\"></body></html>",
        );
        let inputs: Vec<_> = {
            let mut out = Vec::new();
            let mut q = vec![document.document()];
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("input"))
                {
                    out.push(id);
                }
                let mut kids: Vec<_> = document.dom_children(id).collect();
                kids.reverse();
                q.extend(kids);
            }
            out
        };
        assert_eq!(inputs.len(), 2);

        let mut plane: StylePlane<_> = StylePlane::new();
        run_cascade(
            &document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["input:checked { color: rgb(255, 0, 0); }"],
            None,
        );
        let checked = color_of::<StaticDocument>(&plane, inputs[0]);
        let unchecked = color_of::<StaticDocument>(&plane, inputs[1]);
        assert!(
            checked[0] > 0.99 && checked[1] < 0.01,
            "checked input → red, got {checked:?}"
        );
        assert!(
            unchecked[0] < 0.01,
            "unchecked input stays default, got {unchecked:?}"
        );
    }

    /// The parser's quirks-mode selection flows through `LayoutDom::quirks_mode`
    /// into the cascade: a no-doctype document is quirks mode, a `<!DOCTYPE html>`
    /// one is standards, and `build_stylist` carries it into the `Stylist`.
    #[test]
    fn quirks_mode_flows_from_parser_to_stylist() {
        // `StaticDocument` has an inherent `quirks_mode() -> StaticQuirksMode`,
        // so reach the `LayoutDom` trait method (the cascade's source) explicitly.
        use layout_dom_api::LayoutDom;

        // No doctype → quirks mode.
        let quirks_doc = StaticDocument::parse("<html><body><table></table></body></html>");
        let qm = LayoutDom::quirks_mode(&quirks_doc);
        assert_eq!(qm, layout_dom_api::QuirksMode::Quirks);

        let lock = SharedRwLock::new();
        let stylist = build_stylist(
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
            &lock,
            selectors_quirks_mode(qm),
        );
        assert_eq!(
            stylist.quirks_mode(),
            QuirksMode::Quirks,
            "stylist carries quirks mode"
        );

        // `<!DOCTYPE html>` → standards mode.
        let std_doc = StaticDocument::parse("<!DOCTYPE html><html><body></body></html>");
        assert_eq!(
            LayoutDom::quirks_mode(&std_doc),
            layout_dom_api::QuirksMode::NoQuirks
        );
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

    /// Build a persistent Stylist + run the initial full cascade over `plane`,
    /// returning the Stylist to thread into later `restyle_with_snapshots`
    /// calls. The incremental replacement path reuses the rule nodes this
    /// populates, so the restyle must run against the SAME (persistent) rule
    /// tree — mirroring how `IncrementalLayout` owns one Stylist for its life.
    /// (A fresh Stylist per pass is the use-after-free the persistent design
    /// fixes; these tests must therefore share one, exactly like production.)
    fn cascade_persistent<D>(dom: &D, plane: &mut StylePlane<D::NodeId>, sheets: &[&str]) -> Stylist
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + std::hash::Hash + 'static,
    {
        let lock = plane.shared_lock().clone();
        let quirks = selectors_quirks_mode(dom.quirks_mode());
        let stylist = build_stylist(
            euclid::Size2D::new(800.0, 600.0),
            sheets,
            None,
            &lock,
            quirks,
        );
        run_cascade_with_stylist(dom, plane, &stylist);
        stylist
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

        const SHEET: &[&str] = &[
            ".a { color: rgb(255,0,0); } .b { color: rgb(0,0,255); } .keep { color: rgb(0,255,0); }",
        ];
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
        let stylist = cascade_persistent(&dom, &mut plane, SHEET);
        assert_eq!(color_of::<ScriptedDom>(&plane, p)[0], 1.0, "p starts red");

        // Mutate class a → b, drain only that mutation, then restyle.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(p, attr("class"), "b");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, &stylist, &muts);

        // Oracle: a fresh full cascade of the mutated DOM.
        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(
            &dom,
            &mut oracle,
            euclid::Size2D::new(800.0, 600.0),
            SHEET,
            None,
        );

        let p_inc = color_of::<ScriptedDom>(&plane, p);
        let p_full = color_of::<ScriptedDom>(&oracle, p);
        assert_eq!(
            p_inc, p_full,
            "incremental <p> color must match full re-cascade"
        );
        assert!(
            (p_inc[2] - 1.0).abs() < 0.001,
            "<p> should be blue after a→b, got {p_inc:?}"
        );

        // The untouched sibling matches too (still green).
        let span_inc = color_of::<ScriptedDom>(&plane, span);
        let span_full = color_of::<ScriptedDom>(&oracle, span);
        assert_eq!(
            span_inc, span_full,
            "untouched <span> must match full re-cascade"
        );
        assert!(
            (span_inc[1] - 1.0).abs() < 0.001,
            "<span> should stay green, got {span_inc:?}"
        );
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
        let stylist = cascade_persistent(&dom, &mut plane, SHEET);
        assert!(
            color_of::<ScriptedDom>(&plane, p)[2] < 0.001,
            "p starts black (no .box ancestor)"
        );

        // Add class="box" to the div; the descendant <p> must recolor.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(div, attr("class"), "box");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, &stylist, &muts);

        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(
            &dom,
            &mut oracle,
            euclid::Size2D::new(800.0, 600.0),
            SHEET,
            None,
        );

        let p_inc = color_of::<ScriptedDom>(&plane, p);
        assert_eq!(
            p_inc,
            color_of::<ScriptedDom>(&oracle, p),
            "descendant <p> must match full re-cascade after the container's class change"
        );
        assert!(
            (p_inc[2] - 1.0).abs() < 0.001,
            "descendant <p> should be blue via `.box p`, got {p_inc:?}"
        );
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

        const SHEET: &[&str] =
            &["p { color: rgb(0,0,0); } p[data-state=\"on\"] { color: rgb(0,255,0); }"];
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
        let stylist = cascade_persistent(&dom, &mut plane, SHEET);
        assert!(
            color_of::<ScriptedDom>(&plane, p)[1] < 0.01,
            "p starts black (data-state=off)"
        );

        // Toggle data-state off → on.
        let mut sink = Vec::new();
        dom.drain_mutations(&mut sink);
        dom.set_attribute(p, attr("data-state"), "on");
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        restyle_with_snapshots(&dom, &mut plane, &stylist, &muts);

        let mut oracle: StylePlane<_> = StylePlane::new();
        run_cascade(
            &dom,
            &mut oracle,
            euclid::Size2D::new(800.0, 600.0),
            SHEET,
            None,
        );

        let inc = color_of::<ScriptedDom>(&plane, p);
        assert_eq!(
            inc,
            color_of::<ScriptedDom>(&oracle, p),
            "attr restyle must match full re-cascade"
        );
        assert!(
            inc[1] > 0.99,
            "p should be green after data-state→on, got {inc:?}"
        );
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
            let stylist = cascade_persistent(&dom, &mut plane, SHEET);
            let mut sink = Vec::new();
            dom.drain_mutations(&mut sink);
            dom.set_attribute(p, attr("class"), "blue");
            let mut muts = Vec::new();
            dom.drain_mutations(&mut muts);
            let outcome = restyle_with_snapshots(&dom, &mut plane, &stylist, &muts);
            assert!(!outcome.needs_relayout, "color swap should be repaint-only");
        }

        // Width change → relayout.
        {
            let (mut dom, p) = build();
            dom.set_attribute(p, attr("class"), "narrow");
            let mut plane: StylePlane<_> = StylePlane::new();
            let stylist = cascade_persistent(&dom, &mut plane, SHEET);
            let mut sink = Vec::new();
            dom.drain_mutations(&mut sink);
            dom.set_attribute(p, attr("class"), "wide");
            let mut muts = Vec::new();
            dom.drain_mutations(&mut muts);
            let outcome = restyle_with_snapshots(&dom, &mut plane, &stylist, &muts);
            assert!(
                outcome.needs_relayout,
                "width change should require relayout"
            );
        }
    }
}
