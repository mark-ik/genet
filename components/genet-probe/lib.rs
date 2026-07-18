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

/// An element selector: a [`Match`] and optional text. The text (a substring)
/// matches either the element's own child text (a tab's `<div>Links</div>`) or
/// its `aria-label` (a graph-canvas node button, which carries its name there
/// rather than as text) — so one selector spans both the text-labelled and the
/// aria-labelled widgets without the caller knowing which a component used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub matcher: Match,
    pub text: Option<String>,
}

impl Selector {
    /// Select by a class token.
    pub fn class(class: impl Into<String>) -> Self {
        Self {
            matcher: Match::Class(class.into()),
            text: None,
        }
    }

    /// Select by the `role` attribute.
    pub fn role(role: impl Into<String>) -> Self {
        Self {
            matcher: Match::Role(role.into()),
            text: None,
        }
    }

    /// Narrow to elements whose child text or `aria-label` contains `text`.
    pub fn containing(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
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
    match &sel.text {
        None => true,
        Some(t) => {
            child_text(dom, node).contains(t.as_str())
                || attr(dom, node, "aria-label").is_some_and(|l| l.contains(t.as_str()))
        }
    }
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
        let layout =
            IncrementalLayout::new(surface.dom, &[surface.sheet], surface.rect[2], surface.rect[3]);
        for node in matching(surface.dom, sel) {
            if let Some((x, y, w, h)) = layout.absolute_rect(surface.dom, node) {
                return Some(Hit {
                    surface: surface.name,
                    point: (
                        surface.rect[0] + x + w / 2.0,
                        surface.rect[1] + y + h / 2.0,
                    ),
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
            dom.set_attribute(tab, qual("class"), if i == 0 { "tab selected" } else { "tab" });
            dom.set_attribute(tab, qual("role"), "tab");
            dom.set_attribute(
                tab,
                qual("style"),
                &format!("position:absolute;left:{}px;top:0px;width:80px;height:24px;", i * 80),
            );
            let t = dom.create_text(label);
            dom.append_child(tab, t);
            dom.append_child(root, tab);
        }
        // A node button labelled only by aria-label (no child text).
        let node = dom.create_element(qual("button"));
        dom.set_attribute(node, qual("class"), "graph-canvas-swatch-node");
        dom.set_attribute(node, qual("aria-label"), "example.com");
        dom.set_attribute(
            node,
            qual("style"),
            "position:absolute;left:200px;top:40px;width:20px;height:20px;",
        );
        dom.append_child(root, node);
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
    fn resolves_an_aria_labelled_node_the_same_way() {
        let dom = strip_dom();
        let s = surfaces(&dom);
        let hit = resolve(
            &s,
            &Selector::class("graph-canvas-swatch-node").containing("example.com"),
        )
        .expect("the aria-labelled node must resolve by the same selector shape");
        // left 200..220 centre 210 + 500 = 710; top 40..60 centre 50 + 10 = 60.
        assert_eq!(hit.point, (710.0, 60.0));
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
}
