/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Accessibility capability contract — the invariant every content surface
//! declares about what it can expose to the accessibility tree.
//!
//! Adopted from the donor `graphshell` `SUBSYSTEM_ACCESSIBILITY.md` (per the
//! [docs harvest](../../../../design_docs/mere_docs/research/2026-05-27_graphshell_docs_full_harvest.md)
//! §1 and the [adoption roadmap](../../../../design_docs/mere_docs/implementation_strategy/2026-05-27_adoption_roadmap.md)
//! R0). This is the **contract** (the rule), not the full a11y implementation;
//! the host's AccessKit bridge (platen domains → uxtree) consumes it when the
//! a11y slice lands.
//!
//! ## The three invariants
//!
//! 1. **Capability-declaration** — every engine/surface declares its
//!    [`A11yCapability`] in *one* place ([`crate::Engine::a11y_capability`] /
//!    [`crate::SurfaceEngine::a11y_capability`]). The host never guesses a
//!    surface's accessibility from its kind; it reads the declaration.
//! 2. **Non-silent-degradation** — a surface that cannot expose its content
//!    *must* declare a lower capability ([`A11yCapability::Partial`] /
//!    [`A11yCapability::Opaque`]). It must never present as [`A11yCapability::Full`]
//!    while silently dropping semantics. Degradation is *declared*, never silent —
//!    so the host can surface "you can't inspect inside this" honestly (cf. the
//!    [scrying DOM-bridge brief](../../../../genet/docs/2026-05-26_scrying_dom_bridge.md),
//!    which lifts a WebView tile from `Opaque` toward `Partial`).
//! 3. **Cross-surface-parity** — every engine speaks this *one* vocabulary, so
//!    the host treats accessibility uniformly regardless of which engine backs a
//!    tile (a nematic document, a Genet page, a scrying WebView).

use serde::{Deserialize, Serialize};

/// What a content surface can expose to the accessibility tree. The single
/// vocabulary all engines/surfaces speak (invariant 3). Ordered worst-to-best
/// so capability can be compared / clamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum A11yCapability {
    /// No semantic content the host can expose — a raw GPU surface or an opaque
    /// system WebView. The host surfaces this honestly (a labelled region with
    /// "contents not inspectable"), never as if it were [`Self::Full`]. Default
    /// for [`crate::SurfaceEngine`] (frame-streaming surfaces are opaque until
    /// they bridge their content).
    Opaque,
    /// Some structure available — e.g. a bridged WebView exposing a DOM
    /// projection (links / headings / text) but not full ARIA, or a
    /// partially-modelled document.
    Partial,
    /// A complete semantic tree (headings, links, roles, text). Default for
    /// document [`crate::Engine`]s: their [`crate::EngineDocument`] blocks *are*
    /// the semantic content, so they are accessible by construction.
    Full,
}

impl A11yCapability {
    /// Whether the host can build any semantic accessibility nodes from this
    /// surface (everything but [`Self::Opaque`]).
    pub fn is_inspectable(self) -> bool {
        self != Self::Opaque
    }
}
