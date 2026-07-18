//! The flip orchestrator (the carrier; was the `verso` crate).
//!
//! A *flip* re-presents the same page through a different engine, carrying the
//! user's place and session across (charter
//! `verso_docs/technical_architecture/2026-06-10_compatibility_view_charter.md`).
//! This module is the engine-agnostic carrier: it pairs a [`FlipDonor`] with a
//! [`FlipReceiver`] (forward) or a [`FlipBack`] source with a primary receiver
//! (back), masks the view-state down to the layers *both* sides support, and hands
//! the receiver the result. It owns no engine and no GPU layer (it reaches only [`crate::api`]), so a heavy engine only ever lands in the host variant that asked
//! for it, never here.
//!
//! Two invariants live in the type signatures, not in runtime checks:
//!
//! * **Degrade, never block** (charter §3.1): a layer the receiver does not accept
//!   is dropped, not a reason to refuse the flip. [`flip_forward`] / [`flip_back`]
//!   mask by `donor.donates() ∩ receiver.receives()` and always present.
//! * **One hop** (charter §4): [`flip_forward`] takes a [`FlipDonor`] (a glass-box
//!   primary) and [`flip_back`] takes a [`FlipBack`] (a black-box secondary). A
//!   secondary never implements `FlipDonor`, so there is no type path to forward a
//!   document from one secondary to another. A flip is always primary ↔ secondary.

use crate::api::{
    BackState, Carry, FlipBack, FlipDonor, FlipReceiver, LayerSet, PortableViewState,
};

/// Drop every forward layer outside `carried`. The intersection of what the donor
/// donates and what the receiver receives is the only state that crosses; the rest
/// degrades to absent (charter §3.1).
fn mask_forward(mut state: PortableViewState, carried: LayerSet) -> PortableViewState {
    if !carried.contains(LayerSet::NAV) {
        state.url = None;
        state.scroll = (0.0, 0.0);
    }
    if !carried.contains(LayerSet::FORM) {
        state.form = None;
    }
    if !carried.contains(LayerSet::SESSION) {
        state.cookies.clear();
    }
    if !carried.contains(LayerSet::DOM) {
        state.dom_snapshot = None;
    }
    if !carried.contains(LayerSet::VISUAL) {
        state.visual = None;
    }
    state
}

/// Drop every flip-back layer the primary receiver does not accept. A `BackState`
/// only ever carries NAV / FORM / SESSION (a black box has no live DOM to donate and
/// no frame worth carrying home), so the URL is the load-bearing one: without NAV the
/// primary cannot re-root, and `flip_back` declines (returns `false`).
fn mask_back(mut back: BackState, carried: LayerSet) -> BackState {
    if !carried.contains(LayerSet::FORM) {
        back.form = None;
    }
    if !carried.contains(LayerSet::SESSION) {
        back.cookies.clear();
    }
    if !carried.contains(LayerSet::NAV) {
        back.scroll = (0.0, 0.0);
    }
    back
}

/// What a `(donor, receiver)` pair will actually carry on a forward flip — the
/// intersection of the donor's offer and the receiver's appetite. Useful for the
/// host to preview a flip (e.g. warn that the session won't cross) before running it.
pub fn forward_carried(donor: &dyn FlipDonor, receiver: &dyn FlipReceiver) -> LayerSet {
    donor.donates().intersect(receiver.receives())
}

/// Run a forward flip: primary → secondary (the compatibility view). Captures the
/// donor's live state, masks it to [`forward_carried`], and presents it to the
/// receiver as a [`Carry::Forward`]. The receiver owns its own staged inject
/// (cookies before navigate, restore-on-load); the carrier only decides *what*
/// crosses, not the host-frame-loop *how*.
pub fn flip_forward(donor: &dyn FlipDonor, receiver: &mut dyn FlipReceiver) {
    let carried = forward_carried(donor, receiver);
    let state = mask_forward(donor.capture(), carried);
    receiver.present(Carry::Forward(state));
}

/// Run a flip-back: secondary → primary (re-root at the lossless source). Extracts
/// the black box's lean locator, masks it to what the primary accepts, and presents
/// it as a [`Carry::Back`] for the primary to re-fetch and re-render from source.
///
/// Returns `false` without presenting when the primary does not accept NAV: a
/// flip-back with no URL to re-root from is meaningless (the black box has no live
/// document to hand over), so there is nothing to do.
pub fn flip_back(source: &dyn FlipBack, primary: &mut dyn FlipReceiver) -> bool {
    let carried = primary.receives();
    if !carried.contains(LayerSet::NAV) {
        return false;
    }
    let back = mask_back(source.extract(), carried);
    primary.present(Carry::Back(back));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Cookie, FormValues};

    struct MockDonor {
        layers: LayerSet,
        state: PortableViewState,
    }
    impl FlipDonor for MockDonor {
        fn donates(&self) -> LayerSet {
            self.layers
        }
        fn capture(&self) -> PortableViewState {
            self.state.clone()
        }
    }

    struct MockReceiver {
        layers: LayerSet,
        got: Option<Carry>,
    }
    impl FlipReceiver for MockReceiver {
        fn receives(&self) -> LayerSet {
            self.layers
        }
        fn present(&mut self, carry: Carry) {
            self.got = Some(carry);
        }
    }

    struct MockBack(BackState);
    impl FlipBack for MockBack {
        fn extract(&self) -> BackState {
            self.0.clone()
        }
    }

    fn full_state() -> PortableViewState {
        PortableViewState {
            url: Some("https://example.com/p".into()),
            scroll: (0.0, 90.0),
            form: Some(FormValues(vec![("q".into(), "x".into())])),
            cookies: vec![Cookie {
                name: "sid".into(),
                ..Cookie::default()
            }],
            dom_snapshot: Some("<html></html>".into()),
            visual: None,
        }
    }

    #[test]
    fn forward_carries_only_the_intersection() {
        let donor = MockDonor {
            layers: LayerSet::all(),
            state: full_state(),
        };
        // Receiver takes navigation and session but not the DOM snapshot or forms.
        let mut receiver = MockReceiver {
            layers: LayerSet::NAV | LayerSet::SESSION,
            got: None,
        };
        flip_forward(&donor, &mut receiver);

        match receiver.got.unwrap() {
            Carry::Forward(s) => {
                assert_eq!(s.url.as_deref(), Some("https://example.com/p")); // NAV crossed
                assert_eq!(s.cookies.len(), 1); // SESSION crossed
                assert!(s.form.is_none()); // FORM dropped (degrade, not block)
                assert!(s.dom_snapshot.is_none()); // DOM dropped
            },
            Carry::Back(_) => panic!("expected a forward carry"),
        }
    }

    #[test]
    fn forward_carried_is_the_layer_intersection() {
        let donor = MockDonor {
            layers: LayerSet::NAV | LayerSet::FORM | LayerSet::DOM,
            state: full_state(),
        };
        let receiver = MockReceiver {
            layers: LayerSet::NAV | LayerSet::SESSION,
            got: None,
        };
        let carried = forward_carried(&donor, &receiver);
        assert!(carried.contains(LayerSet::NAV));
        assert!(!carried.contains(LayerSet::FORM)); // donor offers it, receiver doesn't take it
        assert!(!carried.contains(LayerSet::SESSION)); // receiver takes it, donor doesn't offer it
    }

    #[test]
    fn flip_back_masks_to_the_primary_appetite() {
        let source = MockBack(BackState {
            url: "https://example.com/after-login".into(),
            scroll: (0.0, 40.0),
            form: Some(FormValues(vec![("q".into(), "y".into())])),
            cookies: vec![Cookie {
                name: "sid".into(),
                ..Cookie::default()
            }],
        });
        // Primary re-roots with nav + session, but does not refill forms.
        let mut primary = MockReceiver {
            layers: LayerSet::NAV | LayerSet::SESSION,
            got: None,
        };
        assert!(flip_back(&source, &mut primary));

        match primary.got.unwrap() {
            Carry::Back(b) => {
                assert_eq!(b.url, "https://example.com/after-login");
                assert_eq!(b.cookies.len(), 1); // SESSION home (login made in scry comes back)
                assert!(b.form.is_none()); // FORM dropped
            },
            Carry::Forward(_) => panic!("expected a back carry"),
        }
    }

    #[test]
    fn flip_back_declines_a_primary_that_cannot_re_root() {
        let source = MockBack(BackState {
            url: "https://example.com/x".into(),
            ..BackState::default()
        });
        // A receiver that does not accept NAV has no URL to re-root from.
        let mut primary = MockReceiver {
            layers: LayerSet::FORM,
            got: None,
        };
        assert!(!flip_back(&source, &mut primary));
        assert!(primary.got.is_none()); // nothing presented
    }
}
