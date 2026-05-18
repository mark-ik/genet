/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Producer-side: emit [`ServalPaintList`] from `FragmentPlane` +
//! `StylePlane` + DOM.
//!
//! Walks the DOM in paint order (pre-order traversal — normal-flow
//! paint order matches DOM order; positioned descendants would
//! reorder via z-index, but the probe doesn't exercise positioning).
//! Reads per-node layout from `FragmentPlane`, reads per-node style
//! from `StylePlane`, and produces a closed-set [`PaintCmd`] stream.
//!
//! ## Probe v1 scope (2026-05-18)
//!
//! - `DrawRect` per element with non-default background. The probe
//!   currently emits an opaque white rect per element since the
//!   cascade runs against an empty stylist; once real stylesheets
//!   apply, [`background_color_of`] becomes the place that reads
//!   `ComputedValues::background.background_color`.
//! - `DrawText` per text leaf with **empty glyph runs**. Real glyph
//!   shaping requires either (a) re-shaping in the emit phase or (b)
//!   caching the parley `Layout` from measure. Both are reasonable —
//!   deferred to a follow-up that picks one based on profile-data;
//!   for the trait-surface probe, empty glyphs is enough to validate
//!   that emission produces the right command structure.
//! - Coordinates are absolute (pre-order accumulated offsets), no
//!   `PushTransform`/`PopTransform` yet. The compositor model fits
//!   nicely with `taffy::Layout.location` being parent-relative, but
//!   emitting it requires `<element>` ↔ `<transform>` bookkeeping
//!   that's deferred until a renderer pulls on it.
//!
//! Cf. `docs/2026-05-17_paintlist_polyglot_renderer.md` (PM-3).

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use malloc_size_of_derive::MallocSizeOf;
use paint_list_api::{
    ColorF, CommonPlacement, DeviceIntSize, EngineId, FontInstanceKey, LayoutPoint, LayoutRect,
    PaintCmd, PaintList, RectItem, TextOptions, TextRunItem,
};
use serde::{Deserialize, Serialize};

use crate::fragment::FragmentPlane;
use crate::style::StylePlane;

/// Serval's concrete [`PaintList`] impl. Built by [`emit_paint_list`].
#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, Serialize)]
pub struct ServalPaintList {
    viewport: DeviceIntSize,
    commands: Vec<PaintCmd>,
    generation: u64,
}

impl ServalPaintList {
    /// Construct an empty paint list. Mainly used by tests.
    pub fn new(viewport: DeviceIntSize) -> Self {
        Self {
            viewport,
            commands: Vec::new(),
            generation: 0,
        }
    }
}

impl PaintList for ServalPaintList {
    fn engine_id(&self) -> EngineId {
        EngineId::SERVAL
    }
    fn viewport(&self) -> DeviceIntSize {
        self.viewport
    }
    fn generation_id(&self) -> u64 {
        self.generation
    }
    fn commands(&self) -> &[PaintCmd] {
        &self.commands
    }
}

/// Walk the DOM in pre-order, emitting paint commands for each
/// element + text leaf with a fragment. Coordinates are absolute
/// (parent-relative `taffy::Layout.location` accumulated through the
/// recursion).
pub fn emit_paint_list<D>(
    dom: &D,
    _styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut commands = Vec::new();
    walk(
        dom,
        fragments,
        dom.document(),
        LayoutPoint::new(0.0, 0.0),
        &mut commands,
    );
    ServalPaintList {
        viewport,
        commands,
        generation: 0,
    }
}

/// Recursive paint-order walk. `parent_origin` is the parent's
/// absolute origin (origin of *its* fragment); children's locations
/// are added to it. Nodes without fragments inherit the parent's
/// origin (they're synthetic / skipped, but children still descend).
fn walk<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    parent_origin: LayoutPoint,
    commands: &mut Vec<PaintCmd>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let layout = fragments.rect_of(id);
    let origin = if let Some(l) = layout {
        LayoutPoint::new(parent_origin.x + l.location.x, parent_origin.y + l.location.y)
    } else {
        parent_origin
    };

    if let Some(l) = layout {
        let bounds = LayoutRect::new(
            origin,
            LayoutPoint::new(origin.x + l.size.width, origin.y + l.size.height),
        );
        match dom.kind(id) {
            NodeKind::Element => {
                commands.push(PaintCmd::DrawRect(RectItem {
                    placement: CommonPlacement::new(bounds),
                    color: background_color_of(id),
                }));
            }
            NodeKind::Text => {
                commands.push(PaintCmd::DrawText(TextRunItem {
                    placement: CommonPlacement::new(bounds),
                    font_instance: FontInstanceKey::default(),
                    color: ColorF::BLACK,
                    glyphs: Vec::new(),
                    options: TextOptions::default(),
                }));
            }
            _ => {}
        }
    }

    for child in dom.dom_children(id) {
        walk(dom, fragments, child, origin, commands);
    }
}

/// Default background color for an element. Probe stage: every
/// element gets opaque white so the trait-surface probe produces
/// visible rects; cascade-driven real color extraction lands when
/// stylesheets apply.
fn background_color_of<NodeId>(_id: NodeId) -> ColorF {
    ColorF::WHITE
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use paint_list_api::PaintList;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::layout::layout;
    use crate::style::StyleEntry;

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

    #[test]
    fn emit_produces_drawrect_for_each_element_and_drawtext_for_text() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _built) = layout(&document, &styles, viewport);

        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        // Trait accessor sanity.
        assert_eq!(plist.engine_id(), EngineId::SERVAL);
        assert_eq!(plist.viewport(), DeviceIntSize::new(800, 600));

        let mut rect_count = 0;
        let mut text_count = 0;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::DrawRect(_) => rect_count += 1,
                PaintCmd::DrawText(_) => text_count += 1,
                _ => {}
            }
        }

        // html, body, p — at least three element rects.
        assert!(
            rect_count >= 3,
            "expected at least 3 DrawRects (html/body/p), got {rect_count}"
        );
        // "Hello" — at least one text run.
        assert!(
            text_count >= 1,
            "expected at least 1 DrawText, got {text_count}"
        );
    }

    #[test]
    fn emit_round_trips_through_serde() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, _) = layout(
            &document,
            &styles,
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        let json = serde_json::to_string(&plist).expect("serialize ServalPaintList");
        let parsed: ServalPaintList =
            serde_json::from_str(&json).expect("deserialize ServalPaintList");
        assert_eq!(parsed.commands().len(), plist.commands().len());
        assert_eq!(parsed.viewport(), plist.viewport());
    }

    #[test]
    fn emit_paint_order_is_pre_order() {
        // Sanity-check that children paint after parents (so they
        // appear later in the command list), matching pre-order DOM
        // traversal.
        let document = StaticDocument::parse(
            "<html><body><p>a</p><p>b</p></body></html>",
        );
        let styles = build_style_plane(&document);
        let (fragments, _) = layout(
            &document,
            &styles,
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            DeviceIntSize::new(800, 600),
        );

        // The first command should be the html rect (root element),
        // not a text-run.
        match plist.commands().first() {
            Some(PaintCmd::DrawRect(_)) => {}
            other => panic!("expected leading DrawRect (html), got {other:?}"),
        }

        // Find the p indexes — there should be at least two.
        let p_count = document
            .dom_children(document.document())
            .flat_map(|html| document.dom_children(html))
            .flat_map(|body| document.dom_children(body))
            .filter(|id| {
                document
                    .element_name(*id)
                    .is_some_and(|q| q.local == local_name!("p"))
            })
            .count();
        assert_eq!(p_count, 2, "fixture has two <p> siblings");
    }
}
