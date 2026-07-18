// Copyright 2026 the genet-probe authors.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared automatability substrate for the genet apps.
//!
//! Every genet app is cambium-based, so every one emits the same thing: a
//! semantic, ARIA-attributed [`ScriptedDom`] laid out by genet-layout. That is
//! the substrate a script, test, or model drives an app through — "click the
//! element labelled X" instead of poking a pixel. This crate is the generic
//! part of that: resolving a **selector** (a role or class, plus optional text)
//! to a **window-space point**, across an app's retained surfaces, using only
//! genet-layout's hit geometry. The app-specific part — which surfaces it has,
//! its typed observation, how it routes a delivered point — stays in the app,
//! behind a small trait (added as consumers pull it; this first slice is the
//! resolver every one of those verbs stands on).
//!
//! A [`ProbeSurface`] is one retained runner's DOM plus where it sits in the
//! window and the sheet it lays out under. An app with several runners (merecat:
//! chrome, roster grid, gloss, trail) lists one each; [`resolve`] searches them
//! in order and returns the first match's centre. Because the resolution is one
//! function over all surfaces, an app stops needing a bespoke geometry lookup
//! per widget — the point the extraction *simplifies* the consumer, not just
//! shares code.
//!
//! [`ScriptedDom`]: genet_scripted_dom::ScriptedDom

use std::collections::BTreeMap;

use genet_layout::IncrementalLayout;
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{LayoutDom, LocalName, Namespace};

/// One retained cambium surface the driver can search and hit-test: its DOM,
/// where it sits in the window (`[x, y, w, h]`, window-space), and the sheet it
/// lays out under.
pub struct ProbeSurface<'a> {
    /// A stable name for diagnostics and hit attribution ("roster", "chrome").
    pub name: &'static str,
    pub dom: &'a ScriptedDom,
    /// Window-space `[x, y, w, h]`.
    pub rect: [f32; 4],
    pub sheet: &'a str,
}

/// What a selector matches an element by. Class matches a token in the element's
/// `class` (so `.tab` matches `class="tab selected"`); Role matches the `role`
/// attribute exactly. Both are the semantics cambium already emits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    Class(String),
    Role(String),
}

/// An element selector: a [`Match`] plus optional filters, all AND-ed. The text
/// (a substring) matches either the element's own child text (a tab's
/// `<div>Links</div>`) or its `aria-label` (a graph-canvas node button, which
/// carries its name there rather than as text) — so one selector spans both the
/// text-labelled and the aria-labelled widgets. The attribute filter matches a
/// named attribute's value: for a target whose visible label is not unique (two
/// graph nodes both titled "Example Domain"), the app puts a stable key in a
/// `data-*` attribute and the driver selects on it. Both filters are the same
/// principle — a target is only findable through identity the DOM carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub matcher: Match,
    pub text: Option<String>,
    /// `(name, value-substring)`: the element's `name` attribute must contain it.
    pub attr: Option<(String, String)>,
}

impl Selector {
    /// Select by a class token.
    pub fn class(class: impl Into<String>) -> Self {
        Self {
            matcher: Match::Class(class.into()),
            text: None,
            attr: None,
        }
    }

    /// Select by the `role` attribute.
    pub fn role(role: impl Into<String>) -> Self {
        Self {
            matcher: Match::Role(role.into()),
            text: None,
            attr: None,
        }
    }

    /// Narrow to elements whose child text or `aria-label` contains `text`.
    pub fn containing(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Narrow to elements whose `name` attribute value contains `value` — for
    /// targeting by a stable key the DOM carries (e.g. `data-key`) rather than by
    /// a visible label that may not be unique.
    pub fn with_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attr = Some((name.into(), value.into()));
        self
    }
}

/// A resolved hit: which surface it landed on and the window-space centre of the
/// matched element, ready to hand to the app's pointer delivery.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub surface: &'static str,
    pub point: (f32, f32),
}

fn ns_local(name: &str) -> (Namespace, LocalName) {
    (Namespace::from(""), LocalName::from(name))
}

fn attr(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<String> {
    let (ns, local) = ns_local(name);
    dom.attribute(node, &ns, &local).map(|s| s.to_string())
}

/// The element's own child text (direct text-node children, joined). Shallow on
/// purpose: cambium's leaf widgets put their label as direct text, and a shallow
/// read cannot accidentally match a deeply-nested sibling's text.
fn child_text(dom: &ScriptedDom, node: NodeId) -> String {
    dom.dom_children(node)
        .filter_map(|c| dom.text(c).map(str::to_string))
        .collect::<Vec<_>>()
        .join("")
}

fn matches(dom: &ScriptedDom, node: NodeId, sel: &Selector) -> bool {
    let by_kind = match &sel.matcher {
        Match::Class(c) => dom.has_class(node, c),
        Match::Role(r) => attr(dom, node, "role").as_deref() == Some(r.as_str()),
    };
    if !by_kind {
        return false;
    }
    let text_ok = match &sel.text {
        None => true,
        Some(t) => {
            child_text(dom, node).contains(t.as_str())
                || attr(dom, node, "aria-label").is_some_and(|l| l.contains(t.as_str()))
        },
    };
    let attr_ok = match &sel.attr {
        None => true,
        Some((name, value)) => attr(dom, node, name).is_some_and(|a| a.contains(value.as_str())),
    };
    text_ok && attr_ok
}

/// Every matching element in pre-order (document order). The caller takes the
/// first one that also has a laid-out box.
fn matching(dom: &ScriptedDom, sel: &Selector) -> Vec<NodeId> {
    fn walk(dom: &ScriptedDom, node: NodeId, sel: &Selector, out: &mut Vec<NodeId>) {
        if matches(dom, node, sel) {
            out.push(node);
        }
        for child in dom.dom_children(node) {
            walk(dom, child, sel, out);
        }
    }
    let mut out = Vec::new();
    walk(dom, dom.document(), sel, &mut out);
    out
}

/// Resolve `sel` to the window-space centre of the first matching, laid-out
/// element across `surfaces` (searched in the order given). `None` when nothing
/// matches, or every match is present in the DOM but has no box (a driver treats
/// that as a miss — the target is not on screen).
pub fn resolve(surfaces: &[ProbeSurface], sel: &Selector) -> Option<Hit> {
    for surface in surfaces {
        let layout = IncrementalLayout::new(
            surface.dom,
            &[surface.sheet],
            surface.rect[2],
            surface.rect[3],
        );
        for node in matching(surface.dom, sel) {
            if let Some((x, y, w, h)) = layout.absolute_rect(surface.dom, node) {
                return Some(Hit {
                    surface: surface.name,
                    point: (surface.rect[0] + x + w / 2.0, surface.rect[1] + y + h / 2.0),
                });
            }
        }
    }
    None
}

/// Whether `substr` appears in any text node across `surfaces` — the basis for
/// an `assert text` verb, independent of a surface's own layout.
pub fn text_present(surfaces: &[ProbeSurface], substr: &str) -> bool {
    fn walk(dom: &ScriptedDom, node: NodeId, substr: &str) -> bool {
        if dom.text(node).is_some_and(|t| t.contains(substr)) {
            return true;
        }
        dom.dom_children(node).any(|c| walk(dom, c, substr))
    }
    surfaces
        .iter()
        .any(|s| walk(s.dom, s.dom.document(), substr))
}

/// A typed read of app state the DOM cannot express — focus, counts, a mode.
/// Deliberately minimal (a focused label plus a string-keyed field map) and
/// grown by need: the event stream carries most app-specific assertions, and a
/// fat shared snapshot type would couple every app to one shape. An app fills
/// only what it can answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeSnapshot {
    /// The focused target's label, if the app has a singular focus.
    pub focused: Option<String>,
    /// Named observations as strings — `"node-count" -> "12"`, `"tab" ->
    /// "Nodes"`. An `assert snap <name> <op> <value>` verb reads these.
    pub fields: BTreeMap<String, String>,
}

impl ProbeSnapshot {
    /// A named field's value, if present.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Builder: set the focused label.
    #[must_use]
    pub fn with_focus(mut self, label: impl Into<String>) -> Self {
        self.focused = Some(label.into());
        self
    }

    /// Builder: add a named field.
    #[must_use]
    pub fn with_field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }
}

/// The small app-specific surface a genet app implements to become drivable. The
/// generic driver (the scenario parser, the verb loop, selector resolution)
/// lives in this crate and calls these; everything an app must supply is here:
/// its retained DOMs, a typed snapshot, its event stream, a named-command entry
/// point, and pointer delivery routed through its own surface plan.
///
/// Implementing it also grants the [`AutomatableExt`] provided methods
/// ([`resolve`](AutomatableExt::resolve), [`click`](AutomatableExt::click)) for
/// free — the point of the extraction: an app lists its surfaces and stops
/// owning per-widget geometry lookups.
pub trait Automatable {
    /// Provide the retained cambium surfaces to a visitor, for it to search and
    /// hit-test this frame. A visitor rather than a returned `Vec` because an
    /// app's DOMs are typically behind `RefCell` (cambium's `DomHandle` is
    /// `Rc<RefCell<ScriptedDom>>`): the borrow guards live only for the closure,
    /// so a returned `Vec<ProbeSurface>` borrowing through them could not
    /// compile. The generic driver reaches surfaces only through here. (This
    /// shape is the first correction the real consumer made to the abstract
    /// trait — the mock held a plain DOM and hid it.)
    fn with_surfaces<R>(&self, f: impl FnOnce(&[ProbeSurface<'_>]) -> R) -> R;

    /// A typed read of app state for assertions the DOM cannot express.
    fn snapshot(&self) -> ProbeSnapshot;

    /// Drain the semantic events emitted since the last call, as the
    /// grep-friendly describe-strings an `assert event` matches.
    fn drain_events(&mut self) -> Vec<String>;

    /// Run one app-named command (the `act <label>` verb). `false` if no such
    /// command, so the driver fails loudly rather than silently no-op.
    fn act(&mut self, label: &str) -> bool;

    /// Deliver a synthetic pointer press at window coords; the app routes it
    /// through its own surface plan (that routing is app-specific).
    fn press(&mut self, x: f32, y: f32);

    /// Deliver a synthetic pointer move (for drags).
    fn moved(&mut self, x: f32, y: f32);

    /// Deliver a synthetic pointer release.
    fn release(&mut self, x: f32, y: f32);
}

/// Provided methods every [`Automatable`] gets — the shared driving verbs built
/// on `with_surfaces` + pointer delivery, so no app writes them.
pub trait AutomatableExt: Automatable {
    /// Resolve `sel` to a window point across this app's surfaces.
    fn resolve(&self, sel: &Selector) -> Option<Hit> {
        self.with_surfaces(|surfaces| resolve(surfaces, sel))
    }

    /// Resolve `sel` and click it (press+release at its centre). `true` if it
    /// hit; `false` is the driver's attributable miss.
    fn click(&mut self, sel: &Selector) -> bool {
        // `resolve` returns an owned Option<Hit> and the surfaces borrow ends
        // with it, so the mutable pointer delivery below does not alias it.
        match self.resolve(sel) {
            Some(hit) => {
                self.press(hit.point.0, hit.point.1);
                self.release(hit.point.0, hit.point.1);
                true
            }
            None => false,
        }
    }
}

impl<A: Automatable> AutomatableExt for A {}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::{LayoutDomMut, QualName};

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    /// A tiny two-tab strip laid out at a known size: `.tab` elements side by
    /// side, one text-labelled, plus a node button that carries its name as
    /// `aria-label` (the graph-canvas shape) — the two labelling styles one
    /// selector must span.
    fn strip_dom() -> ScriptedDom {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        for (i, label) in ["Nodes", "Links"].iter().enumerate() {
            let tab = dom.create_element(qual("div"));
            dom.set_attribute(
                tab,
                qual("class"),
                if i == 0 { "tab selected" } else { "tab" },
            );
            dom.set_attribute(tab, qual("role"), "tab");
            dom.set_attribute(
                tab,
                qual("style"),
                &format!(
                    "position:absolute;left:{}px;top:0px;width:80px;height:24px;",
                    i * 80
                ),
            );
            let t = dom.create_text(label);
            dom.append_child(tab, t);
            dom.append_child(root, tab);
        }
        // Two node buttons sharing a display label ("Example Domain"), each with
        // a unique `data-key` (its url) — the ambiguous-label case that forces
        // attribute targeting.
        for (i, url) in ["https://example.com/", "https://example.org/"]
            .iter()
            .enumerate()
        {
            let node = dom.create_element(qual("button"));
            dom.set_attribute(node, qual("class"), "graph-canvas-swatch-node");
            dom.set_attribute(node, qual("aria-label"), "Example Domain");
            dom.set_attribute(node, qual("data-key"), url);
            dom.set_attribute(
                node,
                qual("style"),
                &format!(
                    "position:absolute;left:{}px;top:40px;width:20px;height:20px;",
                    200 + i * 30
                ),
            );
            dom.append_child(root, node);
        }
        dom
    }

    fn surfaces(dom: &ScriptedDom) -> Vec<ProbeSurface<'_>> {
        vec![ProbeSurface {
            name: "strip",
            dom,
            rect: [500.0, 10.0, 300.0, 200.0],
            sheet: "",
        }]
    }

    #[test]
    fn resolves_a_text_labelled_tab_to_its_window_centre() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        let hit = resolve(&s, &Selector::class("tab").containing("Links"))
            .expect("the Links tab must resolve");
        assert_eq!(hit.surface, "strip");
        // Second tab: left 80..160, centre x=120; + surface origin 500 = 620.
        // top 0..24, centre y=12; + surface origin 10 = 22.
        assert_eq!(hit.point, (620.0, 22.0));
    }

    #[test]
    fn resolves_an_aria_labelled_node_by_its_shared_label() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        // Both nodes share this label; the resolver returns the first in order.
        let hit = resolve(
            &s,
            &Selector::class("graph-canvas-swatch-node").containing("Example Domain"),
        )
        .expect("the aria-labelled node must resolve by the same selector shape");
        // First node: left 200..220 centre 210 + 500 = 710; top 40..60 centre 50 + 10 = 60.
        assert_eq!(hit.point, (710.0, 60.0));
    }

    #[test]
    fn an_attribute_selector_disambiguates_a_shared_label() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        // The two nodes share a label; only their `data-key` (url) tells them
        // apart. Selecting on it resolves the SECOND node, not the first.
        let hit = resolve(
            &s,
            &Selector::class("graph-canvas-swatch-node").with_attr("data-key", "example.org"),
        )
        .expect("the org node must resolve by its data-key");
        // Second node: left 230..250 centre 240 + 500 = 740.
        assert_eq!(hit.point.0, 740.0);
    }

    #[test]
    fn a_role_selector_finds_the_first_tab() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        let hit = resolve(&s, &Selector::role("tab")).expect("role=tab must resolve");
        assert_eq!(hit.point.0, 540.0, "first tab centre x = 40 + surface 500");
    }

    #[test]
    fn a_miss_returns_none() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        assert!(resolve(&s, &Selector::class("tab").containing("Nope")).is_none());
        assert!(resolve(&s, &Selector::role("separator")).is_none());
    }

    #[test]
    fn text_present_spans_the_surfaces() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        assert!(text_present(&s, "Links"));
        assert!(!text_present(&s, "Graphlets"));
    }

    /// A minimal `Automatable`: it supplies only its surfaces and pointer
    /// delivery (recording where a click landed), yet gets `click(sel)` for free
    /// and it lands on the resolved element's centre. This is the extraction's
    /// payoff — an app implements the small surface and the driving verbs come
    /// with it.
    #[test]
    fn a_mock_app_gets_click_for_free() {
        struct MockApp {
            dom: ScriptedDom,
            pressed: Option<(f32, f32)>,
            released: Option<(f32, f32)>,
        }
        impl Automatable for MockApp {
            fn with_surfaces<R>(&self, f: impl FnOnce(&[ProbeSurface<'_>]) -> R) -> R {
                f(&[ProbeSurface {
                    name: "strip",
                    dom: &self.dom,
                    rect: [500.0, 10.0, 300.0, 200.0],
                    sheet: "",
                }])
            }
            fn snapshot(&self) -> ProbeSnapshot {
                ProbeSnapshot::default().with_field("tabs", "2")
            }
            fn drain_events(&mut self) -> Vec<String> {
                Vec::new()
            }
            fn act(&mut self, _label: &str) -> bool {
                false
            }
            fn press(&mut self, x: f32, y: f32) {
                self.pressed = Some((x, y));
            }
            fn moved(&mut self, _x: f32, _y: f32) {}
            fn release(&mut self, x: f32, y: f32) {
                self.released = Some((x, y));
            }
        }

        let mut app = MockApp {
            dom: strip_dom(),
            pressed: None,
            released: None,
        };
        // The provided `click` resolves and delivers, with no app-written geometry.
        let hit = app.click(&Selector::class("tab").containing("Links"));
        assert!(hit, "the Links tab must be found and clicked");
        assert_eq!(app.pressed, Some((620.0, 22.0)));
        assert_eq!(app.released, Some((620.0, 22.0)));
        assert_eq!(app.snapshot().field("tabs"), Some("2"));

        // A miss returns false and delivers nothing new.
        let mut app2 = MockApp {
            dom: strip_dom(),
            pressed: None,
            released: None,
        };
        assert!(!app2.click(&Selector::class("tab").containing("Nope")));
        assert_eq!(app2.pressed, None);
    }
}
