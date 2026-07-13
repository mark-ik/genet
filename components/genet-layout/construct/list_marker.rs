// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! List-item marker resolution (counter styles, marker runs).

use super::*;

/// Map an element's cascaded `list-style-type` to a [`MarkerKind`]. `None` for
/// `list-style-type: none` or when the cascade hasn't run. Custom counter styles
/// (`symbols()`, string) and unrecognized names fall back to a disc bullet.
pub(crate) fn marker_kind<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<MarkerKind> {
    use style::counter_style::CounterStyle;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let cs = &data.styles.primary().get_list().list_style_type.0;
    match cs {
        CounterStyle::None => None,
        CounterStyle::Name(name) => Some(match name.0.to_string().as_str() {
            "disc" => MarkerKind::Bullet("\u{2022}"),
            "circle" => MarkerKind::Bullet("\u{25E6}"),
            "square" => MarkerKind::Bullet("\u{25AA}"),
            "decimal" => MarkerKind::Decimal,
            "lower-alpha" | "lower-latin" => MarkerKind::LowerAlpha,
            "upper-alpha" | "upper-latin" => MarkerKind::UpperAlpha,
            "lower-roman" => MarkerKind::LowerRoman,
            "upper-roman" => MarkerKind::UpperRoman,
            _ => MarkerKind::Bullet("\u{2022}"),
        }),
        _ => Some(MarkerKind::Bullet("\u{2022}")),
    }
}

/// Bijective base-26 alphabetic counter (`1→a`, `26→z`, `27→aa`, …), uppercased
/// when `upper`.
pub(crate) fn alpha_marker(mut n: usize, upper: bool) -> String {
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, (b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    if upper {
        s = s.to_uppercase();
    }
    s
}

/// Roman-numeral counter, uppercased when `upper`. Out-of-range values
/// (`0`, `>= 4000`) fall back to decimal.
pub(crate) fn roman_marker(mut n: usize, upper: bool) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    const VALUES: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (value, sym) in VALUES {
        while n >= value {
            s.push_str(sym);
            n -= value;
        }
    }
    if upper {
        s = s.to_uppercase();
    }
    s
}

/// The list marker string for a `<li>` element, or `None` for non-list-items
/// (and for `list-style-type: none`). The cascaded `list-style-type` chooses a
/// bullet (`disc` / `circle` / `square`) or a counter (`decimal`,
/// `lower`/`upper-alpha`, `lower`/`upper-roman`); counters use the item's 1-based
/// ordinal, counted from preceding `<li>` siblings under the same parent. The
/// `start` attr / `<li> value` and `list-style-image` are deferred.
pub(crate) fn list_marker_text<NodeId: Copy + Eq + Hash, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<String>
where
    D: LayoutDom<NodeId = NodeId>,
{
    if dom.element_name(id)?.local != html5ever::local_name!("li") {
        return None;
    }
    let kind = marker_kind(styles, id)?;
    if let MarkerKind::Bullet(glyph) = kind {
        return Some(glyph.to_string());
    }
    // Counter kinds: the item's ordinal, honoring `start`, `<ol reversed>`, and
    // any `<li value>` (HTML's ordinal algorithm). The counter steps by +1, or
    // -1 when reversed; it begins at `start`, defaulting to 1 (or, when reversed
    // without an explicit `start`, the item count). Each `<li value>` resets it
    // for that item and the ones after.
    let parent = dom.parent(id)?;
    let no_ns: html5ever::Namespace = html5ever::ns!();
    let is_li = |n| {
        dom.element_name(n)
            .is_some_and(|q| q.local == html5ever::local_name!("li"))
    };
    let reversed = dom
        .attribute(parent, &no_ns, &html5ever::LocalName::from("reversed"))
        .is_some();
    let step: i64 = if reversed { -1 } else { 1 };
    let start: i64 = dom
        .attribute(parent, &no_ns, &html5ever::LocalName::from("start"))
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            if reversed {
                dom.dom_children(parent).filter(|n| is_li(*n)).count() as i64
            } else {
                1
            }
        });
    let mut counter = start;
    let mut ordinal = start;
    for sib in dom.dom_children(parent) {
        if !is_li(sib) {
            continue;
        }
        if let Some(v) = dom
            .attribute(sib, &no_ns, &html5ever::LocalName::from("value"))
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            counter = v;
        }
        if sib == id {
            ordinal = counter;
            break;
        }
        counter += step;
    }
    // Alphabetic / roman counters are defined for positive ordinals; outside that
    // range (a 0 / negative `start` or `value`) they fall back to decimal.
    let positive = usize::try_from(ordinal).ok().filter(|n| *n >= 1);
    let body = match (kind, positive) {
        (MarkerKind::Decimal, _) | (_, None) => ordinal.to_string(),
        (MarkerKind::LowerAlpha, Some(n)) => alpha_marker(n, false),
        (MarkerKind::UpperAlpha, Some(n)) => alpha_marker(n, true),
        (MarkerKind::LowerRoman, Some(n)) => roman_marker(n, false),
        (MarkerKind::UpperRoman, Some(n)) => roman_marker(n, true),
        (MarkerKind::Bullet(_), _) => unreachable!("bullets returned above"),
    };
    Some(format!("{body}."))
}

/// Build a list item's marker run with `text`, styled by its `::marker` pseudo
/// when the cascade resolved one (`li::marker { color/font-* }`), else by the
/// item's own font + color. Decoration is always cleared (a marker is never
/// underlined / struck through by the item's `text-decoration`).
pub(crate) fn marker_run<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    text: String,
) -> InlineRun {
    let mut run = match styles.marker_style(id) {
        Some(cv) => run_from_computed(cv, text),
        None => run_for_element(styles, id, text),
    };
    run.underline = false;
    run.strikethrough = false;
    run
}

/// The marker for a list item as single-run [`InlineContent`], styled by its
/// `::marker` pseudo (or the `<li>`'s own font + color), ready to shape and hang
/// to the left of the item. `None` for non-list-items.
pub(crate) fn list_marker_content<NodeId, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<InlineContent<NodeId>>
where
    NodeId: Copy + Eq + Hash,
    D: LayoutDom<NodeId = NodeId>,
{
    let text = list_marker_text(dom, styles, id)?;
    let run = marker_run(styles, id, text);
    Some(InlineContent {
        runs: vec![run],
        boxes: Vec::new(),
        no_wrap: false,
    })
}

/// Whether an element's cascaded `list-style-position` is `inside` (the marker
/// flows as the item's first inline content rather than hanging outside).
pub(crate) fn list_marker_is_inside<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::computed_values::list_style_position::T as ListPosition;
    styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .is_some_and(|d| d.styles.primary().get_list().list_style_position == ListPosition::Inside)
}

/// The marker as an inline run (with a trailing space) for `list-style-position:
/// inside`, styled by the item's font + color. `None` for non-list-items and
/// `list-style-type: none`.
pub(crate) fn list_marker_inline_run<NodeId, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<InlineRun>
where
    NodeId: Copy + Eq + Hash,
    D: LayoutDom<NodeId = NodeId>,
{
    let text = list_marker_text(dom, styles, id)?;
    Some(marker_run(styles, id, format!("{text} ")))
}
