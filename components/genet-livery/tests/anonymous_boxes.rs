//! Whitespace between block siblings must generate no box.
//!
//! White-space processing removes a collapsible whitespace run that sits
//! between two block boxes. In block flow an extra empty box is invisible, so
//! this went unnoticed; a grid or flex container turns every in-flow child
//! into an item, where a stray whitespace box consumes a cell and shifts
//! every following item by one. Found 2026-07-26 as the cause of the
//! css-grid abspos cluster's residual paint delta.

use genet_livery::{Device, InteractionStates, StyleSet, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

fn find(
    dom: &StaticDocument,
    node: <StaticDocument as LayoutDom>::NodeId,
    needle: &str,
) -> Option<<StaticDocument as LayoutDom>::NodeId> {
    if dom.kind(node) == NodeKind::Element
        && dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(needle)
    {
        return Some(node);
    }
    dom.dom_children(node)
        .find_map(|child| find(dom, child, needle))
}

/// Lay out `html` with `css` and return each id's fragment origin.
fn origins(html: &str, css: &str, ids: &[&str]) -> Vec<(f32, f32)> {
    let document = StaticDocument::parse(html);
    let styles = resolve_styles(
        &document,
        &StyleSet::cambium(&[css]),
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &styles, 800.0, 600.0).expect("layout");
    ids.iter()
        .map(|name| {
            let id = find(&document, document.document(), name).expect(name);
            let fragment = fragments.get(id).copied().unwrap_or_default();
            (fragment.x, fragment.y)
        })
        .collect()
}

/// Newlines and indentation between grid items are how every real document is
/// written, so this is the common case rather than an edge one.
const SPACED: &str = r#"<html><body><div id="box">
        <div id="a">A</div>
        <div id="b">B</div>
        <div id="c">C</div>
        <div id="d">D</div>
      </div></body></html>"#;

/// The same markup with the whitespace removed.
const TIGHT: &str = r#"<html><body><div id="box"><div id="a">A</div><div id="b">B</div><div id="c">C</div><div id="d">D</div></div></body></html>"#;

const IDS: [&str; 4] = ["a", "b", "c", "d"];

#[test]
fn whitespace_between_grid_items_generates_no_item() {
    let css = "#box { display: grid; \
               grid-template-rows: 50px 50px; grid-template-columns: 100px 100px; \
               align-items: start; justify-items: start; \
               width: 200px; height: 100px; }";
    let spaced = origins(SPACED, css, &IDS);
    assert_eq!(
        spaced,
        origins(TIGHT, css, &IDS),
        "indentation changed grid placement",
    );
    // Four items in a 2x2 at the container origin (body margin 8).
    assert_eq!(
        spaced,
        vec![(8.0, 8.0), (108.0, 8.0), (8.0, 58.0), (108.0, 58.0)],
    );
}

#[test]
fn whitespace_between_flex_items_generates_no_item() {
    let css = "#box { display: flex; width: 400px; } #box > div { width: 100px; }";
    let spaced = origins(SPACED, css, &IDS);
    assert_eq!(
        spaced,
        origins(TIGHT, css, &IDS),
        "indentation changed flex placement",
    );
    // Four 100px items packed from the container's content origin.
    assert_eq!(
        spaced,
        vec![(8.0, 8.0), (108.0, 8.0), (208.0, 8.0), (308.0, 8.0)],
    );
}

#[test]
fn preserved_whitespace_still_generates_its_box() {
    // `white-space: pre` makes the run meaningful, so it is a real item and
    // the blank-run rule must not swallow it.
    let css = "#box { display: grid; grid-template-columns: 50px 50px; \
               white-space: pre; width: 100px; }";
    let spaced = origins(SPACED, css, &IDS);
    let tight = origins(TIGHT, css, &IDS);
    assert_ne!(
        spaced, tight,
        "preserved whitespace must still occupy a grid cell",
    );
}

/// `&nbsp;` is not collapsible white space, so it generates a line box.
///
/// Rust's `str::trim` treats U+00A0 as whitespace; CSS does not. Trimming it
/// away deletes the line a test built with `&nbsp;` was relying on, which is
/// how 143 CSS2 reftests broke on 2026-07-26 before the rule was narrowed to
/// css-text-3's actual set.
#[test]
fn a_no_break_space_still_generates_a_line_box() {
    let css = "#box { display: grid; grid-template-columns: 100px; } \
               #a { background: red; }";
    let with_nbsp = "<html><body><div id=\"box\">\
                     <div id=\"a\">\u{a0}</div></div></body></html>";
    let with_space = "<html><body><div id=\"box\">\
                      <div id=\"a\"> </div></div></body></html>";
    let heights = |html: &str| {
        let document = StaticDocument::parse(html);
        let styles = resolve_styles(
            &document,
            &StyleSet::cambium(&[css]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let fragments = layout(&document, &styles, 800.0, 600.0).expect("layout");
        let id = find(&document, document.document(), "a").expect("a");
        fragments.get(id).copied().unwrap_or_default().height
    };
    assert!(
        heights(with_nbsp) > 0.0,
        "a no-break space must generate a line box",
    );
    assert_eq!(
        heights(with_space),
        0.0,
        "an ordinary space between blocks collapses away",
    );
}

/// Block containers keep their current anonymous boxes.
///
/// The blank-run rule is scoped to flex and grid on purpose: the same change
/// in block flow measured -131 files on CSS2 (2026-07-26), because the extra
/// boxes are load-bearing for the current table and inline-formatting
/// emulation. This pins the scope so widening it is a deliberate act with a
/// measurement behind it, not an accident.
#[test]
fn block_containers_are_out_of_scope_for_now() {
    let css = "#box { display: block; width: 200px; }";
    let spaced = origins(SPACED, css, &IDS);
    let tight = origins(TIGHT, css, &IDS);
    assert_eq!(
        spaced, tight,
        "block flow stacks its children the same either way",
    );
}
