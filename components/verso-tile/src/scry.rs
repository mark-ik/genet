//! The scry (system-WebView) flip receiver (was the `verso-scry` crate).
//!
//! scry is a black-box secondary (charter §3, asymmetric fidelity): the host cannot
//! inject a live document into a system WebView, only *navigate* it and then nudge it
//! by script. So a forward flip into scry re-fetches the URL faithfully (the WebView
//! runs the page's real JS) and carries the cheap layers across:
//!
//! * SESSION — cookies set on the WebView store *before* navigation, so the page
//!   loads already-authenticated (a login made in genet comes across).
//! * NAV — the scroll offset, restored by script once the load completes.
//! * FORM — field values, refilled by script, best-effort.
//!
//! It does **not** take the DOM snapshot (it re-fetches from source) or the visual
//! frame (the host compositor owns the cross-fade). The DOM-snapshot degrade path
//! (`navigate_to_string`) is for url-less documents and is left to the host.
//!
//! ## Two-phase, host-driven
//!
//! A WebView loads across frames (the §1 finding): the receiver cannot do its work in
//! one synchronous call. [`ScryForward`] is therefore a small state machine the host
//! frame loop drives: [`begin`](ScryForward::begin) sets cookies and navigates;
//! [`on_nav`](ScryForward::on_nav) runs the scroll/form restore when the host reports
//! the load `Completed`. The host bridges its concrete producer to the [`ScrySurface`]
//! seam and translates its nav-event stream into [`NavSignal`]s, so this module stays
//! free of the platform WebView dep and is unit-testable on its own.

use crate::api::{Carry, Cookie, FlipReceiver, FormValues, LayerSet, PortableViewState};

/// The handful of WebView operations a forward flip needs, abstracted so this crate
/// does not depend on the concrete (Windows-only) producer. The host implements it
/// over its WebView, mapping [`Cookie`] to the engine's cookie type and running script
/// synchronously (the engine's blocking, message-pumped execute-script is fine for the
/// one-shot restore).
pub trait ScrySurface {
    /// Set one cookie on the WebView's store. Called before navigation.
    fn set_cookie(&mut self, cookie: &Cookie) -> Result<(), String>;
    /// Begin a (non-blocking) navigation to `url`.
    fn navigate(&mut self, url: &str) -> Result<(), String>;
    /// Run `js` in the loaded document. The result is ignored by the restore, but the
    /// signature mirrors the producer's `execute_script_with_result` so the host need
    /// not invent a void variant.
    fn run_script(&mut self, js: &str) -> Result<String, String>;
}

/// A navigation signal the host pumps in from its producer's nav-event stream
/// (scrying's `NavigationEvent::Completed { success }` maps to
/// `NavSignal::Completed { success }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavSignal {
    /// A navigation started. Informational; the receiver waits for completion.
    Started,
    /// The navigation finished. `success` is the top-level load result.
    Completed { success: bool },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// Built, not yet begun. `begin` moves it to `Navigating`.
    Pending,
    /// Cookies set and navigation kicked off; waiting for `Completed`.
    Navigating,
    /// Restore script run (or nothing to restore). Terminal.
    Done,
}

/// The receiving side of a forward flip into a scry tile. Holds the carried view-state
/// and drives the cookies → navigate → restore choreography across host frames.
pub struct ScryForward {
    url: Option<String>,
    scroll: (f32, f32),
    form: Option<FormValues>,
    cookies: Vec<Cookie>,
    phase: Phase,
}

impl ScryForward {
    /// The layers scry can apply: SESSION (cookies), NAV (scroll; the URL is the
    /// navigation key), and FORM (refill). Not DOM (it re-fetches) nor VISUAL.
    pub const RECEIVES: LayerSet = LayerSet::NAV.union(LayerSet::SESSION).union(LayerSet::FORM);

    /// Build a receiver from the carried state. The carrier has already masked it to
    /// the intersection of the donor's offer and [`RECEIVES`](Self::RECEIVES), so this
    /// keeps whatever survived.
    pub fn new(state: PortableViewState) -> Self {
        Self {
            url: state.url,
            scroll: state.scroll,
            form: state.form,
            cookies: state.cookies,
            phase: Phase::Pending,
        }
    }

    /// Whether there is a URL to navigate to. A flip with no URL cannot use the
    /// faithful re-fetch path; the host should fall back to the DOM-snapshot degrade
    /// path (or skip the flip) rather than drive this receiver.
    pub fn has_target(&self) -> bool {
        self.url.is_some()
    }

    /// Phase 1: set the carried cookies on the WebView store, then navigate. Cookies
    /// go on *before* navigation so the page loads authenticated. A no-op once begun.
    /// Cookie failures are non-fatal (logged by the host); a missing cookie degrades
    /// the session, it does not block the flip.
    pub fn begin(&mut self, surface: &mut dyn ScrySurface) -> Result<(), String> {
        if self.phase != Phase::Pending {
            return Ok(());
        }
        for cookie in &self.cookies {
            if let Err(err) = surface.set_cookie(cookie) {
                // Degrade, never block: one cookie that won't set is not a reason to
                // abandon the flip. The host decides whether to surface it.
                tracing_warn(&format!("scry flip: set_cookie failed: {err}"));
            }
        }
        match &self.url {
            Some(url) => {
                surface.navigate(url)?;
                self.phase = Phase::Navigating;
                Ok(())
            }
            // No URL: nothing to navigate. Terminal; the host handles the degrade path.
            None => {
                self.phase = Phase::Done;
                Ok(())
            }
        }
    }

    /// Phase 2: feed a nav signal. On the first successful `Completed`, run the
    /// scroll + form restore by script and finish. A failed load also finishes (there
    /// is nothing to restore onto a failed page). Ignored unless navigating.
    pub fn on_nav(&mut self, signal: NavSignal, surface: &mut dyn ScrySurface) {
        if self.phase != Phase::Navigating {
            return;
        }
        match signal {
            NavSignal::Started => {}
            NavSignal::Completed { success } => {
                if success {
                    if let Some(js) = self.restore_script() {
                        if let Err(err) = surface.run_script(&js) {
                            tracing_warn(&format!("scry flip: restore script failed: {err}"));
                        }
                    }
                }
                self.phase = Phase::Done;
            }
        }
    }

    /// Whether the flip has run to completion (restored, failed, or had no URL).
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The post-load restore: scroll the viewport, then refill known form fields by
    /// name (falling back to id). Returns `None` when there is nothing to restore.
    fn restore_script(&self) -> Option<String> {
        let has_scroll = self.scroll != (0.0, 0.0);
        let fields = self.form.as_ref().map(|f| f.0.as_slice()).unwrap_or(&[]);
        if !has_scroll && fields.is_empty() {
            return None;
        }
        let mut js = String::from("(function(){");
        if has_scroll {
            js.push_str(&format!(
                "window.scrollTo({},{});",
                self.scroll.0, self.scroll.1
            ));
        }
        for (key, value) in fields {
            // getElementsByName takes the raw name (no CSS escaping); fall back to id.
            // Both strings are JSON-encoded for a safe JS string literal.
            js.push_str(&format!(
                "(function(k,v){{var es=document.getElementsByName(k);\
                 if(es.length){{for(var i=0;i<es.length;i++)es[i].value=v;}}\
                 else{{var e=document.getElementById(k);if(e)e.value=v;}}}})({},{});",
                js_string(key),
                js_string(value),
            ));
        }
        js.push_str("})();");
        Some(js)
    }
}

/// scry receives a [`Carry::Forward`] by storing it for the host-driven pump. A
/// [`Carry::Back`] is meaningless for a secondary (it never re-roots), so it is
/// ignored. `present` only stages the flip; [`begin`](ScryForward::begin) /
/// [`on_nav`](ScryForward::on_nav) do the work across frames.
impl FlipReceiver for ScryForward {
    fn receives(&self) -> LayerSet {
        Self::RECEIVES
    }

    fn present(&mut self, carry: Carry) {
        if let Carry::Forward(state) = carry {
            *self = ScryForward::new(state);
        }
    }
}

/// JSON-encode a string into a double-quoted JS string literal (escapes `"`, `\`,
/// and control characters). Enough for embedding form keys/values in a script.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// `tracing` is not a dep of this crate (the default build stays dependency-free); the host owns
// logging. This thin shim keeps the degrade-path warnings visible in debug builds
// without taking the dep. The host's nav pump is where real telemetry lives.
#[inline]
fn tracing_warn(msg: &str) {
    #[cfg(debug_assertions)]
    eprintln!("[verso-scry] {msg}");
    #[cfg(not(debug_assertions))]
    let _ = msg;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scriptable stand-in for the host's WebView: records the calls the flip makes.
    #[derive(Default)]
    struct MockSurface {
        cookies: Vec<Cookie>,
        navigated: Option<String>,
        scripts: Vec<String>,
    }
    impl ScrySurface for MockSurface {
        fn set_cookie(&mut self, cookie: &Cookie) -> Result<(), String> {
            self.cookies.push(cookie.clone());
            Ok(())
        }
        fn navigate(&mut self, url: &str) -> Result<(), String> {
            self.navigated = Some(url.to_string());
            Ok(())
        }
        fn run_script(&mut self, js: &str) -> Result<String, String> {
            self.scripts.push(js.to_string());
            Ok(String::new())
        }
    }

    fn forward_state() -> PortableViewState {
        PortableViewState {
            url: Some("https://example.com/app".into()),
            scroll: (0.0, 200.0),
            form: Some(FormValues(vec![("user".into(), "ada".into())])),
            cookies: vec![Cookie {
                name: "sid".into(),
                value: "tok".into(),
                ..Cookie::default()
            }],
            dom_snapshot: None,
            visual: None,
        }
    }

    #[test]
    fn begin_sets_cookies_before_navigating() {
        let mut surface = MockSurface::default();
        let mut flip = ScryForward::new(forward_state());
        flip.begin(&mut surface).unwrap();
        assert_eq!(surface.cookies.len(), 1); // session set
        assert_eq!(
            surface.navigated.as_deref(),
            Some("https://example.com/app")
        ); // then navigate
        assert!(!flip.is_done()); // waiting for the load
    }

    #[test]
    fn restore_runs_once_on_successful_completion() {
        let mut surface = MockSurface::default();
        let mut flip = ScryForward::new(forward_state());
        flip.begin(&mut surface).unwrap();
        flip.on_nav(NavSignal::Started, &mut surface);
        assert!(surface.scripts.is_empty()); // nothing restored mid-load
        flip.on_nav(NavSignal::Completed { success: true }, &mut surface);
        assert_eq!(surface.scripts.len(), 1);
        let js = &surface.scripts[0];
        assert!(js.contains("scrollTo(0,200)")); // NAV restored
        assert!(js.contains("\"user\"") && js.contains("\"ada\"")); // FORM refilled
        assert!(flip.is_done());
        // A second Completed does not re-run the restore.
        flip.on_nav(NavSignal::Completed { success: true }, &mut surface);
        assert_eq!(surface.scripts.len(), 1);
    }

    #[test]
    fn failed_load_finishes_without_restoring() {
        let mut surface = MockSurface::default();
        let mut flip = ScryForward::new(forward_state());
        flip.begin(&mut surface).unwrap();
        flip.on_nav(NavSignal::Completed { success: false }, &mut surface);
        assert!(surface.scripts.is_empty()); // nothing to restore onto a failed page
        assert!(flip.is_done());
    }

    #[test]
    fn no_url_is_terminal_after_begin() {
        let mut surface = MockSurface::default();
        let state = PortableViewState {
            url: None,
            ..forward_state()
        };
        let mut flip = ScryForward::new(state);
        assert!(!flip.has_target());
        flip.begin(&mut surface).unwrap();
        assert!(surface.navigated.is_none()); // nothing to navigate
        assert!(flip.is_done()); // host takes the degrade path
    }

    #[test]
    fn nothing_to_restore_skips_the_script() {
        let mut surface = MockSurface::default();
        let state = PortableViewState {
            url: Some("https://example.com/".into()),
            scroll: (0.0, 0.0),
            form: None,
            cookies: vec![],
            dom_snapshot: None,
            visual: None,
        };
        let mut flip = ScryForward::new(state);
        flip.begin(&mut surface).unwrap();
        flip.on_nav(NavSignal::Completed { success: true }, &mut surface);
        assert!(surface.scripts.is_empty()); // no scroll, no forms -> no script
        assert!(flip.is_done());
    }

    #[test]
    fn present_stages_a_forward_carry() {
        let mut flip = ScryForward::new(PortableViewState::default());
        flip.present(Carry::Forward(forward_state()));
        assert!(flip.has_target());
        assert_eq!(flip.receives(), ScryForward::RECEIVES);
    }
}
