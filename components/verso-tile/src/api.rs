//! The engine-agnostic flip contract (was the `verso-api` crate).
//!
//! A *flip* re-presents the same page through a different engine with the user's
//! place and session carried across (see
//! `verso_docs/technical_architecture/2026-06-10_compatibility_view_charter.md`).
//! This module is the engine-agnostic contract: the portable view-state moved across
//! a flip, plus the donor / back / receiver traits. It depends on no engine and no
//! GPU layer. Per-engine adapters ([`crate::scry`], [`crate::genet`], ...) bridge concrete
//! engines to these traits; the [`crate::flip`] orchestrator pairs a donor with a
//! receiver and runs the flip choreography.

/// Which layers of view-state a carrier moves. Layers degrade, never block: a
/// carrier moves `donor.donates() & receiver.receives()`, and any layer outside
/// that intersection is simply dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerSet(u8);

impl LayerSet {
    pub const NAV: Self = Self(1 << 0);
    pub const FORM: Self = Self(1 << 1);
    pub const SESSION: Self = Self(1 << 2);
    pub const DOM: Self = Self(1 << 3);
    pub const VISUAL: Self = Self(1 << 4);

    /// The empty set.
    pub const fn empty() -> Self {
        Self(0)
    }
    /// Every layer.
    pub const fn all() -> Self {
        Self(0b1_1111)
    }
    /// Does this set contain every layer in `other`?
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// Layers in both sets — what a `(donor, receiver)` pair can actually carry.
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    /// Layers in either set. The const companion to [`intersect`](Self::intersect),
    /// for building a fixed layer set (e.g. an adapter's `RECEIVES`) in a const.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Is the set empty?
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for LayerSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A cookie's `SameSite` attribute (RFC 6265bis). Carried so the gating semantics
/// survive a flip; `None` means unspecified (engines default it to `Lax`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A cookie in engine-agnostic terms, mirroring the RFC 6265bis record (the shape
/// the [`cookie`](https://crates.io/crates/cookie) crate uses, which both Mere's
/// netfetcher jar and Servo's net jar build on). Adapters convert to/from the
/// engine's own cookie type; carrying the full record keeps a flip's session lossless.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// `SameSite` gating, or `None` when unspecified.
    pub same_site: Option<SameSite>,
    /// Absolute expiry in Unix seconds (the normalized form of `Expires` / `Max-Age`),
    /// or `None` for a session cookie.
    pub expires: Option<f64>,
    /// `Partitioned` (CHIPS): the cookie is keyed to the top-level site.
    pub partitioned: bool,
}

/// Form field values keyed by a stable selector. Best-effort across engines.
#[derive(Clone, Debug, Default)]
pub struct FormValues(pub Vec<(String, String)>);

/// An opaque reference to the donor's last rendered frame, resolved by the host
/// compositor. Texture plumbing stays the compositor's (charter §2); verso only
/// *refers* to a frame so a flip can cross-fade instead of flashing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHandle(pub u64);

/// The rich state a glass-box primary (genet, nematic) can export. Layered so it
/// degrades gracefully (charter §3.1): a receiver takes the layers it supports and
/// ignores the rest.
#[derive(Clone, Debug, Default)]
pub struct PortableViewState {
    /// Current document URL.
    pub url: Option<String>,
    /// Document scroll offset in device px.
    pub scroll: (f32, f32),
    /// Form field values.
    pub form: Option<FormValues>,
    /// Cookies for the origin (one-shot at flip time; the substrate splits after).
    pub cookies: Vec<Cookie>,
    /// Serialized outerHTML — the degrade path when the URL is not refetchable.
    pub dom_snapshot: Option<String>,
    /// The donor's last frame, for the cross-fade.
    pub visual: Option<FrameHandle>,
}

/// The lean locator a black-box secondary (scry, weld, graft) can surface for a
/// flip-back. The receiver re-roots from this — it re-fetches the URL and renders
/// from source — and never reconstructs a live document out of the black box.
#[derive(Clone, Debug, Default)]
pub struct BackState {
    pub url: String,
    pub scroll: (f32, f32),
    pub form: Option<FormValues>,
    pub cookies: Vec<Cookie>,
}

/// What a receiver is asked to present.
pub enum Carry {
    /// Primary → secondary: the rich forward carry.
    Forward(PortableViewState),
    /// Secondary → primary: the lean locator to re-root from.
    Back(BackState),
}

/// A glass-box primary engine that can export its full live state. Implemented by
/// genet and nematic through their `verso-*` adapters.
pub trait FlipDonor {
    /// The layers this donor can export.
    fn donates(&self) -> LayerSet;
    /// Capture the current live state.
    fn capture(&self) -> PortableViewState;
}

/// A black-box secondary engine that can only surface a locator for flip-back.
///
/// Secondaries implement `FlipBack` and **never** [`FlipDonor`]. That is the
/// charter's no-chain invariant (§4) expressed in the type system: there is no path
/// to forward-donate a document to another secondary, so a flip is always one hop
/// (primary ↔ secondary), re-rooting at the lossless source rather than chaining.
pub trait FlipBack {
    /// Extract the cheap locator (URL, scroll, form, cookies) the black box exposes.
    fn extract(&self) -> BackState;
}

/// An engine that can host a flipped page. A secondary receiver consumes a
/// [`Carry::Forward`] by navigating; a primary receiver consumes a [`Carry::Back`]
/// by re-fetching the URL and re-rendering from source.
pub trait FlipReceiver {
    /// The layers this receiver can apply.
    fn receives(&self) -> LayerSet;
    /// Present the carried state.
    fn present(&mut self, carry: Carry);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layerset_intersection_is_the_carried_set() {
        let donor = LayerSet::all();
        let receiver = LayerSet::NAV | LayerSet::SESSION | LayerSet::VISUAL;
        let carried = donor.intersect(receiver);
        assert!(carried.contains(LayerSet::NAV));
        assert!(carried.contains(LayerSet::SESSION));
        assert!(!carried.contains(LayerSet::DOM));
    }

    #[test]
    fn empty_and_all_bound_the_lattice() {
        assert!(LayerSet::empty().is_empty());
        assert!(LayerSet::all().contains(LayerSet::DOM));
        assert!(LayerSet::all().contains(LayerSet::VISUAL));
    }
}
