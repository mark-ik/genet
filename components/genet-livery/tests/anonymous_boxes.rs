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
            let fragment = fragments
                .get(id)
                .map(|fragment| fragment.physical_rect())
                .unwrap_or_default();
            (fragment.x, fragment.y)
        })
        .collect()
}

#[test]
fn live_layout_exposes_buckram_box_and_fragment_identity() {
    let document = StaticDocument::parse(
        "<html><body><div id=\"parent\"><div id=\"child\">child</div></div></body></html>",
    );
    let styles = resolve_styles(
        &document,
        &StyleSet::cambium(&[]),
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let layout = layout(&document, &styles, 800.0, 600.0).expect("layout");
    let parent_node = find(&document, document.document(), "parent").expect("parent");
    let child_node = find(&document, document.document(), "child").expect("child");
    let parent_box = layout
        .boxes()
        .principal_box(parent_node)
        .expect("parent box");
    let child_box = layout.boxes().principal_box(child_node).expect("child box");

    assert_eq!(layout.boxes()[child_box].parent(), Some(parent_box));

    let parent_fragment = layout
        .fragments_for_node(parent_node)
        .next()
        .expect("parent fragment");
    let child_fragment = layout
        .fragments_for_node(child_node)
        .next()
        .expect("child fragment");
    assert_eq!(parent_fragment.box_id(), parent_box);
    assert_eq!(child_fragment.box_id(), child_box);
    assert_eq!(child_fragment.parent(), Some(parent_fragment.id()));
    assert_eq!(
        child_fragment.containing_fragment(),
        Some(parent_fragment.id())
    );
    assert_eq!(
        layout.fragments().fragment_ids_for_box(child_box),
        &[child_fragment.id()]
    );
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
        fragments
            .get(id)
            .map(|fragment| fragment.physical_rect())
            .unwrap_or_default()
            .height
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

/// A `display: table` box lays its cells out as a grid.
///
/// Neither lane implements real table layout: taffy has no table algorithm
/// and `stylo_taffy` says so outright. The incumbent renders tables by
/// flattening the row-group and row nesting and giving every cell an explicit
/// grid position, and Livery now does the same. Before this a table mapped to
/// a flex row with no row or column structure, so every cell landed on one
/// line.
#[test]
fn table_cells_are_placed_on_a_grid() {
    // The UA sheet supplies the table display values; author CSS only sizes
    // the cells so the assertions can be exact.
    let html = "<html><body><table id=\"t\">        <tr><td id=\"a\">a</td><td id=\"b\">b</td></tr>        <tr><td id=\"c\">c</td><td id=\"d\">d</td></tr>        </table></body></html>";
    let css = "td { width: 40px; height: 20px; padding: 0; }";
    let cells = origins(html, css, &["a", "b", "c", "d"]);
    let [(ax, ay), (bx, by), (cx, cy), (dx, dy)] = [cells[0], cells[1], cells[2], cells[3]];
    assert_eq!(ay, by, "the first row shares one y");
    assert_eq!(cy, dy, "the second row shares one y");
    assert_eq!(ax, cx, "the first column shares one x");
    assert_eq!(bx, dx, "the second column shares one x");
    assert!(bx > ax, "the second column sits right of the first");
    assert!(cy > ay, "the second row sits below the first");
}

/// CSS 2.1 section 17.5.2.1: fixed table layout sizes columns from the first
/// row, not from content.
///
/// The case is WPT's `CSS2/tables/fixed-table-layout-003a01`: a 400px table
/// of three columns whose middle first-row cell asks for `width: 80px` with
/// `padding: 0 60px`. `width` is a content-box width, so that cell
/// establishes a 200px column, and the two auto columns split the remaining
/// 200px. Content never enters the calculation, which is what makes the
/// fixed algorithm computable before layout.
#[test]
fn fixed_table_layout_sizes_columns_from_the_first_row() {
    let html = "<html><body><table id=\"t\">        <tr><td id=\"a\"></td><td id=\"b\">A01</td><td id=\"c\"></td></tr>        </table></body></html>";
    let css = "table { table-layout: fixed; width: 400px; border-spacing: 0; }                td { padding: 0 60px; border: none; }                td#b { width: 80px; }";
    let cells = origins(html, css, &["a", "b", "c"]);
    let (ax, _) = cells[0];
    let (bx, _) = cells[1];
    let (cx, _) = cells[2];
    // Columns are 100 / 200 / 100 across the table's 400px content box.
    assert_eq!(bx - ax, 100.0, "the first auto column takes half the slack");
    assert_eq!(cx - bx, 200.0, "the sized cell establishes a 200px column");
}

/// `table-layout: auto` leaves the columns content-sized, so the same markup
/// must not pick up the fixed algorithm's widths.
#[test]
fn auto_table_layout_does_not_use_the_fixed_algorithm() {
    let html = "<html><body><table id=\"t\">        <tr><td id=\"a\"></td><td id=\"b\">A01</td><td id=\"c\"></td></tr>        </table></body></html>";
    let css = "table { width: 400px; border-spacing: 0; }                td { padding: 0 60px; border: none; }                td#b { width: 80px; }";
    let cells = origins(html, css, &["a", "b", "c"]);
    assert_ne!(
        cells[1].0 - cells[0].0,
        100.0,
        "auto tables must not take the fixed column widths",
    );
}
