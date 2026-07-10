//! The serval [`FlipDonor`] glass-box donor (was the `verso-serval` crate; the `serval-donor` feature).
//!
//! serval is a glass-box primary: a flip *out of* serval can export the full live
//! view-state (charter §3, asymmetric fidelity). This adapter bridges serval's
//! engine APIs to [`crate::api::PortableViewState`]. It reaches three sources, by
//! layer:
//!
//! * DOM + FORM come straight from the scripted DOM (`ScriptedDom::outer_html`,
//!   `ScriptedDom::form_values`), so the adapter can fill them itself.
//! * NAV (url, scroll) lives in serval's *script runtime* (`HostState.base_url`,
//!   `HostState.viewport_scroll`), not the DOM.
//! * SESSION (cookies) lives in the host's netfetcher / eidetic jar.
//! * VISUAL (the last frame) is a compositor handle.
//!
//! The last three are not DOM-derivable, so the host feeds them in via the
//! `with_*` setters. That keeps this module depending only on `serval-scripted-dom`
//! and the [`crate::api`] contract (never the runtime or the GPU layer), and mirrors
//! how the [`crate::scry`] receiver is host-fed. The [`crate::flip`] orchestrator builds a donor at
//! flip time, calls [`capture`](FlipDonor::capture) once, and drops it.

use layout_dom_api::LayoutDom;
use serval_scripted_dom::ScriptedDom;
use crate::api::{Cookie, FlipDonor, FormValues, FrameHandle, LayerSet, PortableViewState};

/// A one-shot flip donor over a serval scripted document.
///
/// Borrows the live DOM for the FORM and DOM layers; the NAV, SESSION, and VISUAL
/// layers are fed by the host (they live outside the DOM: the script runtime, the
/// cookie jar, the compositor). [`donates`](FlipDonor::donates) advertises FORM and
/// DOM unconditionally (a glass-box can always serialize and walk its tree) plus
/// whichever host layers were supplied.
pub struct ServalDonor<'a> {
    dom: &'a ScriptedDom,
    url: Option<String>,
    scroll: (f32, f32),
    cookies: Vec<Cookie>,
    visual: Option<FrameHandle>,
}

impl<'a> ServalDonor<'a> {
    /// A donor over `dom`'s primary document (its [`LayoutDom::document`] root). The
    /// DOM and form layers come from the tree; add the host-owned layers with the
    /// `with_*` setters before calling [`capture`](FlipDonor::capture).
    pub fn new(dom: &'a ScriptedDom) -> Self {
        Self {
            dom,
            url: None,
            scroll: (0.0, 0.0),
            cookies: Vec::new(),
            visual: None,
        }
    }

    /// The document URL (serval's `HostState.base_url`). Enables the NAV layer and
    /// the receiver's faithful re-fetch path.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// The viewport scroll offset (serval's `HostState.viewport_scroll`). Carried
    /// inside the NAV layer; defaults to the origin when no url is fed.
    pub fn with_scroll(mut self, scroll: (f32, f32)) -> Self {
        self.scroll = scroll;
        self
    }

    /// Origin cookies from the host's netfetcher / eidetic jar. A non-empty set
    /// enables the SESSION layer (one-shot at flip time; charter §3).
    pub fn with_cookies(mut self, cookies: Vec<Cookie>) -> Self {
        self.cookies = cookies;
        self
    }

    /// The donor's last rendered frame, for the cross-fade. Enables the VISUAL layer.
    pub fn with_visual(mut self, frame: FrameHandle) -> Self {
        self.visual = Some(frame);
        self
    }
}

impl FlipDonor for ServalDonor<'_> {
    fn donates(&self) -> LayerSet {
        // FORM and DOM are always available from the tree; the rest only when the
        // host fed them.
        let mut set = LayerSet::FORM | LayerSet::DOM;
        if self.url.is_some() {
            set = set | LayerSet::NAV;
        }
        if !self.cookies.is_empty() {
            set = set | LayerSet::SESSION;
        }
        if self.visual.is_some() {
            set = set | LayerSet::VISUAL;
        }
        set
    }

    fn capture(&self) -> PortableViewState {
        let root = self.dom.document();
        PortableViewState {
            url: self.url.clone(),
            scroll: self.scroll,
            form: Some(FormValues(self.dom.form_values(root))),
            cookies: self.cookies.clone(),
            dom_snapshot: Some(self.dom.outer_html(root)),
            visual: self.visual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_donor_donates_form_and_dom_only() {
        let dom = ScriptedDom::new();
        let set = ServalDonor::new(&dom).donates();
        assert!(set.contains(LayerSet::FORM));
        assert!(set.contains(LayerSet::DOM));
        assert!(!set.contains(LayerSet::NAV));
        assert!(!set.contains(LayerSet::SESSION));
        assert!(!set.contains(LayerSet::VISUAL));
    }

    #[test]
    fn fed_layers_widen_donates_and_fill_capture() {
        let dom = ScriptedDom::new();
        let donor = ServalDonor::new(&dom)
            .with_url("https://example.com/page")
            .with_scroll((0.0, 120.0))
            .with_cookies(vec![Cookie {
                name: "sid".into(),
                value: "abc".into(),
                ..Cookie::default()
            }])
            .with_visual(FrameHandle(7));

        let set = donor.donates();
        assert!(set.contains(LayerSet::NAV | LayerSet::SESSION | LayerSet::VISUAL));

        let state = donor.capture();
        assert_eq!(state.url.as_deref(), Some("https://example.com/page"));
        assert_eq!(state.scroll, (0.0, 120.0));
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.visual, Some(FrameHandle(7)));
        // A glass-box always offers the FORM and DOM layers, even on an empty page.
        assert!(state.form.is_some());
        assert!(state.dom_snapshot.is_some());
    }

    #[test]
    fn scroll_without_url_does_not_claim_nav() {
        let dom = ScriptedDom::new();
        // Scroll alone is degenerate without a document to re-root: NAV is gated on
        // the url (the receiver's faithful re-fetch key), not the scroll.
        let set = ServalDonor::new(&dom).with_scroll((10.0, 20.0)).donates();
        assert!(!set.contains(LayerSet::NAV));
    }
}
