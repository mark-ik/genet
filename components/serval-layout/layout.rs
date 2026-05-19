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
use crate::text_measure::{measure_text_leaf, TextMeasureCtx};

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
                Some(leaf) => measure_text_leaf(&mut text_ctx, leaf, taffy_id, known, avail),
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

        // Find the text node — its measured rect height should differ
        // between the two cascades.
        let mut text_id = None;
        let mut queue = vec![document.document()];
        while let Some(id) = queue.pop() {
            if matches!(document.kind(id), layout_dom_api::NodeKind::Text) {
                text_id = Some(id);
                break;
            }
            queue.extend(document.dom_children(id));
        }
        let text_id = text_id.expect("document contains a text node");

        let big_rect = frags_big
            .rect_of(text_id)
            .expect("big stylesheet: text fragment");
        let default_rect = frags_default
            .rect_of(text_id)
            .expect("default stylesheet: text fragment");

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

    /// Cascaded `font-family` flows into the text leaf's `TextLeaf`
    /// context. Deterministic (inspects the leaf, not font-dependent
    /// pixel output): a `p { font-family: monospace }` rule produces a
    /// text leaf carrying `FontFamilySpec::Generic(Monospace)`.
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

        // Find the text node + its Taffy leaf context.
        let mut text_id = None;
        let mut queue = vec![document.document()];
        while let Some(id) = queue.pop() {
            if matches!(document.kind(id), layout_dom_api::NodeKind::Text) {
                text_id = Some(id);
                break;
            }
            queue.extend(document.dom_children(id));
        }
        let text_id = text_id.expect("document contains a text node");
        let taffy_id = built.node_map.get(&text_id).expect("text node in node_map");
        let leaf = built
            .tree
            .get_node_context(*taffy_id)
            .expect("text leaf carries a TextLeaf context");

        assert!(
            matches!(
                leaf.font_family,
                FontFamilySpec::Generic(GenericFamilyKind::Monospace)
            ),
            "expected monospace from cascade, got {:?}",
            leaf.font_family
        );
    }

    /// Probe the parley measure path: with default-sized elements, a
    /// text node should give its containing `<p>` a non-zero width
    /// derived from parley's measurement of the text.
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

        // The text leaf isn't keyed in our node_map directly under the
        // <p>'s DOM id — it gets its own. The text node's fragment
        // should have positive dimensions. Find a text node by DOM
        // kind and check its fragment.
        let mut text_id = None;
        let mut queue = vec![document.document()];
        while let Some(id) = queue.pop() {
            if matches!(document.kind(id), layout_dom_api::NodeKind::Text) {
                text_id = Some(id);
                break;
            }
            queue.extend(document.dom_children(id));
        }
        let text_id = text_id.expect("document contains a text node");

        // Confirm the text leaf is in the node_map (construct.rs adds it).
        assert!(
            built.node_map.contains_key(&text_id),
            "expected text node in Taffy node_map after construct"
        );

        // Confirm the text rect has positive width — parley measured it.
        let rect = fragments.rect_of(text_id).expect("text node has a fragment");
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
