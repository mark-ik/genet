/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layout entry point.
//!
//! Runs the minimum end-to-end pipeline: construct a Taffy tree from a
//! `LayoutDom` + `StylePlane`, ask Taffy to compute layout against a
//! viewport (with parley measuring text leaves), then read per-node
//! results back into a `FragmentPlane`.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use layout_dom_api::LayoutDom;

use crate::construct::{construct, ConstructedTree};
use crate::fragment::FragmentPlane;
use crate::style::StylePlane;
use crate::text_measure::{measure_inline_content, TextMeasureCtx};

/// Run the layout pipeline against a viewport.
///
/// Steps:
/// 1. `construct(dom, styles, viewport)` — DOM walk → Taffy tree with
///    text leaves carrying [`crate::text_measure::TextLeaf`] context.
/// 2. `taffy::compute_layout_with_measure(...)` with a parley-backed
///    measure closure that resolves text leaves to natural sizes and
///    caches the shaped `Layout` per text leaf.
/// 3. Walk the node_map → populate `FragmentPlane` with per-node rects.
///
/// Returns the `FragmentPlane`, the `ConstructedTree` (for tests +
/// emit's `node_map` lookup), and the `TextMeasureCtx` (which holds
/// the cached `parley::Layout` per text leaf — paint emission reads
/// from here to extract positioned glyphs without re-shaping).
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport: taffy::Size<taffy::AvailableSpace>,
) -> (
    FragmentPlane<D::NodeId>,
    ConstructedTree<D::NodeId>,
    TextMeasureCtx,
)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut built = construct(dom, styles, viewport);
    let mut text_ctx = TextMeasureCtx::new();

    built
        .tree
        .compute_layout_with_measure(
            built.root,
            viewport,
            |known, avail, taffy_id, ctx, _style| match ctx {
                Some(content) => {
                    measure_inline_content(&mut text_ctx, content, taffy_id, known, avail)
                },
                None => taffy::Size::ZERO,
            },
        )
        .expect("taffy compute_layout failed");

    let mut fragments = FragmentPlane::new();
    for (dom_id, taffy_id) in built.node_map.iter() {
        if let Ok(layout) = built.tree.layout(*taffy_id) {
            fragments.insert(*dom_id, *layout);
        }
    }

    (fragments, built, text_ctx)
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::style::StyleEntry;

    /// Find the first element descendant matching a local name, walking the
    /// subtree under `start`. Used to locate `<p>` etc. without depending on
    /// html5ever's auto-inserted `<head>` ordering.
    fn find_element<'a, D: LayoutDom>(
        start: NodeRef<'a, D>,
        local: html5ever::LocalName,
    ) -> Option<NodeRef<'a, D>> {
        let mut queue = vec![start];
        while let Some(node) = queue.pop() {
            if let Some(name) = node.dom().element_name(node.id()) {
                if name.local == local {
                    return Some(node);
                }
            }
            queue.extend(node.dom_children());
        }
        None
    }

    /// Build a minimal StylePlane by hand: assign every element a
    /// block-display Taffy style with a fixed width/height. Skips Stylo
    /// entirely for the probe.
    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            size: Size {
                                width: length(200.0),
                                height: length(50.0),
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
        plane
    }

    /// Same as `build_style_plane` but without forcing fixed sizes —
    /// elements get default style so Taffy computes their dimensions
    /// from the text children. Used by the parley-measurement test
    /// where hard-coded sizes would mask the text measurement.
    fn build_default_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
        plane
    }

    #[test]
    fn probe_layout_assigns_nonzero_rect_to_p() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _built, _ctx) = layout(&document, &styles, viewport);

        let root = NodeRef::document(&document);
        let p_node = find_element(root, local_name!("p")).expect("<p> exists");
        let rect = fragments.rect_of(p_node.id()).expect("<p> got laid out");

        assert!(
            rect.size.width > 0.0,
            "expected positive width, got {}",
            rect.size.width
        );
        assert!(
            rect.size.height > 0.0,
            "expected positive height, got {}",
            rect.size.height
        );

        // FragmentPlane should have entries for html, body, p — three elements,
        // plus the inline text leaf under <p>.
        assert!(
            fragments.len() >= 3,
            "expected at least 3 fragments, got {}",
            fragments.len()
        );
    }

    #[test]
    fn probe_layout_respects_height() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, viewport);

        let root = NodeRef::document(&document);
        let p = find_element(root, local_name!("p")).unwrap();
        let rect = fragments.rect_of(p.id()).unwrap();

        assert_eq!(
            rect.size.height, 50.0,
            "expected height 50.0, got {}",
            rect.size.height
        );
    }

    /// Cascade-driven font-size propagates from the parent element's
    /// ComputedValues into the text leaf. We size the parent
    /// dramatically (32px vs default 16px) and assert the measured
    /// text height roughly tracks the difference — parley's measured
    /// height is proportional to font-size for single-line text.
    #[test]
    fn parley_inherits_font_size_from_cascade() {
        use crate::cascade::run_cascade;

        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");

        // Run cascade with a font-size: 32px rule, then refresh Taffy
        // styles from the cascade so layout sees real cascaded values.
        let mut styles_big: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles_big,
            euclid::Size2D::new(800.0, 600.0),
            &["p { font-size: 32px; }"],
        );
        styles_big.refresh_taffy_from_cascade();

        // Baseline: cascade with no font-size rule (uses default 16px).
        let mut styles_default: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles_default,
            euclid::Size2D::new(800.0, 600.0),
            &[],
        );
        styles_default.refresh_taffy_from_cascade();

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (frags_big, _, _) = layout(&document, &styles_big, viewport);
        let (frags_default, _, _) = layout(&document, &styles_default, viewport);

        // `<p>` is an inline formatting context (its only child is
        // text), so it's the measured leaf — check its fragment
        // height. The 32px cascade should produce a taller box than
        // the 16px one.
        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let big_rect = frags_big
            .rect_of(p.id())
            .expect("big stylesheet: <p> fragment");
        let default_rect = frags_default
            .rect_of(p.id())
            .expect("default stylesheet: <p> fragment");

        // 32px should produce ~2x the height of 16px (line-height
        // defaults are font-size proportional in parley).
        assert!(
            big_rect.size.height > default_rect.size.height * 1.5,
            "expected big font (32px) to produce >1.5x default (16px) text height; \
             big={} default={}",
            big_rect.size.height,
            default_rect.size.height
        );
    }

    /// Cascaded `font-family` flows into the inline leaf's
    /// `InlineContent` run. Deterministic (inspects the leaf, not
    /// font-dependent pixels): a `p { font-family: monospace }` rule
    /// produces a `<p>` inline leaf whose run carries
    /// `FontFamilySpec::Generic(Monospace)`.
    #[test]
    fn cascade_font_family_flows_into_text_leaf() {
        use crate::cascade::run_cascade;
        use crate::text_measure::{FontFamilySpec, GenericFamilyKind};

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { font-family: monospace; }"],
        );
        styles.refresh_taffy_from_cascade();

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (_frags, built, _ctx) = layout(&document, &styles, viewport);

        // `<p>` is the inline-context leaf; its InlineContent's first
        // run carries the cascaded family.
        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let taffy_id = built.node_map.get(&p.id()).expect("<p> in node_map");
        let content = built
            .tree
            .get_node_context(*taffy_id)
            .expect("<p> carries an InlineContent context");
        let run = content.runs.first().expect("inline content has a run");

        assert!(
            matches!(
                run.font_family,
                FontFamilySpec::Generic(GenericFamilyKind::Monospace)
            ),
            "expected monospace from cascade, got {:?}",
            run.font_family
        );
    }

    /// Inline flow: `<p>Hello <b>world</b> !</p>` gathers into one
    /// inline-context leaf whose runs carry per-element styling — the
    /// `<b>` run is bold (weight 700 from the UA `b { font-weight: bold }`
    /// rule), the surrounding runs are normal (400). And it lays out
    /// on ONE line (height ≈ a single line), not stacked.
    #[test]
    fn inline_flow_gathers_styled_runs_on_one_line() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body><p>Hello <b>world</b> !</p></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
        );
        styles.refresh_taffy_from_cascade();

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _ctx) = layout(&document, &styles, viewport);

        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");

        // p is the inline-context leaf; its runs span the inline subtree.
        let taffy_id = built.node_map.get(&p.id()).expect("<p> in node_map");
        let content = built
            .tree
            .get_node_context(*taffy_id)
            .expect("<p> carries InlineContent");
        assert!(
            content.runs.len() >= 2,
            "expected multiple runs (text + <b>), got {}",
            content.runs.len()
        );
        let has_normal = content.runs.iter().any(|r| r.weight < 500.0);
        let has_bold = content.runs.iter().any(|r| r.weight >= 600.0);
        assert!(has_normal, "expected a normal-weight run");
        assert!(has_bold, "expected a bold run from <b>");

        // One line: p's height is about a single line (~16 * 1.2 ≈ 19),
        // certainly under two lines — i.e. the runs flowed inline
        // rather than stacking as separate blocks.
        let h = fragments.rect_of(p.id()).expect("<p> fragment").size.height;
        assert!(
            h < 40.0,
            "expected single-line height (<40px), got {h} — runs may be stacking"
        );
    }

    /// Probe the parley measure path: a `<p>` whose only child is text
    /// establishes an inline formatting context, so it's the measured
    /// leaf. Its fragment should get a non-zero size derived from
    /// parley's measurement of the gathered text.
    #[test]
    fn parley_measures_inline_text() {
        let document =
            StaticDocument::parse("<html><body><p>Hello, world!</p></body></html>");
        let styles = build_default_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _ctx) = layout(&document, &styles, viewport);

        // <p> is the inline-context leaf; it's in node_map and the
        // text node is not (gathered into <p>'s InlineContent).
        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        assert!(
            built.node_map.contains_key(&p.id()),
            "expected <p> in Taffy node_map after construct"
        );

        let rect = fragments.rect_of(p.id()).expect("<p> has a fragment");
        assert!(
            rect.size.width > 0.0,
            "expected positive width from parley measurement, got {}",
            rect.size.width
        );
        assert!(
            rect.size.height > 0.0,
            "expected positive height from parley measurement, got {}",
            rect.size.height
        );
    }
}
