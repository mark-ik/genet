/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Full-list walker: `ServalDisplayList` → `Vec<PaintCmd>`. Companion
//! to [`paint_cmd_bridge`] — that module does per-item lowering
//! (state-blind); this one threads clip + spatial palette state
//! through compositor primitives (`PushClip`/`PushTransform` and
//! their `Pop` matches) so the emitted command stream is structurally
//! complete.
//!
//! ## What this validates
//!
//! The PM-3 audit asserted the closed-set `PaintCmd` vocabulary is
//! sufficient for every `ServalDisplayItem` and that compositor
//! state (clip stacks, transform stacks) flows through Push/Pop
//! pairs rather than per-item references. The walker is the proof:
//! it converts the WebRender-shaped placement model (every item
//! carries `clip_chain_id` + `spatial_id` references into palettes)
//! into the PaintCmd-shaped stack model.
//!
//! ## What this does **not** do
//!
//! - **Production-ready optimization.** The walker emits the
//!   structurally-correct stream; it doesn't optimize for minimal
//!   push/pop sequences across runs of identically-placed items.
//!   A real producer either walks the layout tree directly (see
//!   `serval-layout::paint_emit`) or runs an optimizer pass.
//! - **Coordination with explicit state items.** `PushReferenceFrame`
//!   and `PushStackingContext` items emit their own compositor
//!   pushes via [`paint_cmd_bridge::lower_item`]; the walker
//!   threads the *passive* placement state alongside. If the
//!   producer is consistent (every item inside a stacking context
//!   does declare its spatial_id as inside that context's spatial
//!   subtree) the two state streams coexist; if not, double-push
//!   can occur. Coordination logic is a follow-up.
//! - **ScrollFrame / StickyFrame as compositor primitives.** Both
//!   lower to identity transforms here — the scroll-offset / sticky
//!   transform application is producer-side concern (paint-list-api
//!   doesn't model scroll containers as compositor primitives in
//!   v1).

use paint_list_api::{self as ple};
use paint_types::SpatialId;
use paint_types::units::LayoutTransform;

use crate::paint_cmd_bridge::{LowerContext, lower_item};
use crate::serval_display_list::{
    self as sdl, ClipChainId, ClipDef, CommonItemPlacement, ScrollFrameDef, ServalDisplayItem,
    ServalDisplayList, SpatialNodeDef,
};

/// Top-level entry: lower a complete `ServalDisplayList` to the
/// equivalent `PaintCmd` stream. Threads clip + spatial state through
/// `PushClip`/`PopClip` and `PushTransform`/`PopTransform` pairs.
pub fn lower_display_list(list: &ServalDisplayList) -> Vec<ple::PaintCmd> {
    let ctx = LowerContext {
        clip_defs: &list.clip_defs,
        spatial_nodes: &list.spatial_nodes,
        transforms: &list.transforms,
    };
    let mut walker = Walker::new(list, &ctx);
    walker.walk();
    walker.finish()
}

// =============================================================================
// Walker — paths + emitted commands
// =============================================================================

struct Walker<'a> {
    list: &'a ServalDisplayList,
    ctx: &'a LowerContext<'a>,
    commands: Vec<ple::PaintCmd>,
    /// Spatial nodes from root → current, by SpatialId. Always begins
    /// with the root node.
    spatial_path: Vec<SpatialId>,
    /// Clip-chain ids from root → current. May be empty (no clip
    /// applied) or carry a chain of `ClipChainId`s; sentinel
    /// `ClipChainId::INVALID` is never pushed.
    clip_path: Vec<ClipChainId>,
}

impl<'a> Walker<'a> {
    fn new(list: &'a ServalDisplayList, ctx: &'a LowerContext<'a>) -> Self {
        Self {
            list,
            ctx,
            commands: Vec::new(),
            spatial_path: vec![list.root_spatial_id()],
            clip_path: Vec::new(),
        }
    }

    fn walk(&mut self) {
        for item in &self.list.items {
            // Items that don't carry a placement (PopStackingContext,
            // PopReferenceFrame, PopAllShadows) lower directly with no
            // transition; their effect on the *active* stacking-context /
            // ref-frame state is observed via lower_item.
            if let Some(placement) = item_placement(item) {
                self.transition_to_spatial(placement.spatial_id);
                self.transition_to_clip(placement.clip_chain_id);
            }
            let cmd = lower_item(item, self.ctx);
            self.commands.push(cmd);
        }
    }

    fn finish(mut self) -> Vec<ple::PaintCmd> {
        self.unwind_clip_to(0);
        self.unwind_spatial_to(1); // leave root on the stack (never pushed)
        self.commands
    }

    // -------------------------------------------------------------------
    // Spatial transitions
    // -------------------------------------------------------------------

    fn transition_to_spatial(&mut self, target: SpatialId) {
        let target_path = self.compute_spatial_path(target);
        let lca_len = common_prefix_len(&self.spatial_path, &target_path);
        self.unwind_spatial_to(lca_len);
        for node_id in &target_path[lca_len..] {
            self.push_spatial(*node_id);
        }
    }

    fn unwind_spatial_to(&mut self, lca_len: usize) {
        while self.spatial_path.len() > lca_len {
            self.spatial_path.pop();
            self.commands.push(ple::PaintCmd::PopTransform);
        }
    }

    fn push_spatial(&mut self, id: SpatialId) {
        let node = match self.spatial_node(id) {
            Some(n) => n,
            None => {
                // Index out-of-bounds — emit identity to keep the
                // stack balanced; surfaces in tests as a structural
                // mismatch.
                self.spatial_path.push(id);
                self.commands.push(ple::PaintCmd::PushTransform(ple::TransformSpec {
                    origin: paint_list_api::LayoutPoint::new(0.0, 0.0),
                    transform: LayoutTransform::identity(),
                    kind: ple::TransformKind::Standard,
                }));
                return;
            },
        };
        let spec = self.spatial_node_to_transform(node);
        self.spatial_path.push(id);
        self.commands.push(ple::PaintCmd::PushTransform(spec));
    }

    fn spatial_node(&self, id: SpatialId) -> Option<&'a SpatialNodeDef> {
        self.list.spatial_nodes.get(id.0 as usize)
    }

    /// Build the path from root → `target` by following parent links.
    fn compute_spatial_path(&self, target: SpatialId) -> Vec<SpatialId> {
        let mut chain = Vec::new();
        let mut current = target;
        // Defensive cycle break — should never trigger on a
        // well-formed list, but protects the walker from infinite
        // recursion if a producer ever wires parents into a cycle.
        let mut guard = self.list.spatial_nodes.len() + 2;
        loop {
            chain.push(current);
            match self.spatial_node(current) {
                Some(SpatialNodeDef::Root) => break,
                Some(SpatialNodeDef::ScrollFrame(f)) => current = f.parent,
                Some(SpatialNodeDef::StickyFrame(f)) => current = f.parent,
                Some(SpatialNodeDef::ReferenceFrame(f)) => current = f.parent,
                None => break,
            }
            guard = guard.saturating_sub(1);
            if guard == 0 {
                break;
            }
        }
        chain.reverse();
        chain
    }

    fn spatial_node_to_transform(&self, node: &SpatialNodeDef) -> ple::TransformSpec {
        match node {
            SpatialNodeDef::Root => ple::TransformSpec {
                origin: paint_list_api::LayoutPoint::new(0.0, 0.0),
                transform: LayoutTransform::identity(),
                kind: ple::TransformKind::Standard,
            },
            SpatialNodeDef::ScrollFrame(f) => self.scroll_frame_transform(f),
            SpatialNodeDef::StickyFrame(_) => ple::TransformSpec {
                // Sticky positioning is layout-side; the frame contributes
                // identity at the compositor seam. The producer applies
                // the sticky offset by mutating the affected items'
                // placements rather than transforming the spatial node.
                origin: paint_list_api::LayoutPoint::new(0.0, 0.0),
                transform: LayoutTransform::identity(),
                kind: ple::TransformKind::Standard,
            },
            SpatialNodeDef::ReferenceFrame(f) => {
                let transform = self
                    .ctx
                    .transforms
                    .get(f.transform.0 as usize)
                    .copied()
                    .unwrap_or_else(LayoutTransform::identity);
                ple::TransformSpec {
                    origin: f.origin,
                    transform,
                    kind: match f.kind {
                        paint_types::ReferenceFrameKind::Transform { .. } => {
                            ple::TransformKind::Standard
                        },
                        paint_types::ReferenceFrameKind::Perspective { .. } => {
                            ple::TransformKind::Perspective
                        },
                    },
                }
            },
        }
    }

    fn scroll_frame_transform(&self, f: &ScrollFrameDef) -> ple::TransformSpec {
        // The scroll-offset moves content; the compositor seam sees the
        // offset as a translation. Real producer wires the live offset
        // here; this walker uses the static `external_scroll_offset`
        // recorded at list-build time.
        let tx = LayoutTransform::translation(
            -f.external_scroll_offset.x,
            -f.external_scroll_offset.y,
            0.0,
        );
        ple::TransformSpec {
            origin: f.content_rect.min,
            transform: tx,
            kind: ple::TransformKind::Standard,
        }
    }

    // -------------------------------------------------------------------
    // Clip transitions
    // -------------------------------------------------------------------

    fn transition_to_clip(&mut self, target: ClipChainId) {
        if target.is_invalid() {
            // No clip: unwind any active clips.
            self.unwind_clip_to(0);
            return;
        }
        let target_path = self.compute_clip_path(target);
        let lca_len = common_prefix_len(&self.clip_path, &target_path);
        self.unwind_clip_to(lca_len);
        for clip_id in &target_path[lca_len..] {
            self.push_clip(*clip_id);
        }
    }

    fn unwind_clip_to(&mut self, lca_len: usize) {
        while self.clip_path.len() > lca_len {
            self.clip_path.pop();
            self.commands.push(ple::PaintCmd::PopClip);
        }
    }

    fn push_clip(&mut self, id: ClipChainId) {
        let spec = match self.clip_def(id) {
            Some(ClipDef::Rect(rd)) => ple::ClipSpec {
                kind: ple::ClipKind::Rect(rd.rect),
            },
            Some(ClipDef::RoundedRect(rrd)) => ple::ClipSpec {
                kind: ple::ClipKind::RoundedRect {
                    rect: rrd.rect,
                    radius: rrd.radius,
                    clip_out: matches!(rrd.mode, sdl::ClipMode::ClipOut),
                },
            },
            // Chain entries are walker-internal — they're traversed
            // by `compute_clip_path`, not pushed individually. If we
            // get here it's a producer/walker mismatch; emit an empty
            // rect clip and keep going so tests can surface the issue.
            Some(ClipDef::Chain(_)) | None => ple::ClipSpec {
                kind: ple::ClipKind::Rect(paint_list_api::LayoutRect::zero()),
            },
        };
        self.clip_path.push(id);
        self.commands.push(ple::PaintCmd::PushClip(spec));
    }

    fn clip_def(&self, id: ClipChainId) -> Option<&'a ClipDef> {
        self.list.clip_defs.get(id.0 as usize)
    }

    /// Build the path from outermost → `target` by following
    /// `ClipDef::Chain` parent links. Non-Chain entries are leaves;
    /// the path is the sequence of leaf ids walking up via Chain
    /// parents.
    fn compute_clip_path(&self, target: ClipChainId) -> Vec<ClipChainId> {
        let mut chain = Vec::new();
        let mut current = target;
        let mut guard = self.list.clip_defs.len() + 2;
        loop {
            if current.is_invalid() {
                break;
            }
            match self.clip_def(current) {
                Some(ClipDef::Chain(c)) => {
                    // The chain entry references a leaf clip; push the
                    // leaf, then ascend to its parent chain.
                    chain.push(c.clip);
                    current = c.parent;
                },
                Some(ClipDef::Rect(_)) | Some(ClipDef::RoundedRect(_)) => {
                    // Direct leaf — record and stop.
                    chain.push(current);
                    break;
                },
                None => break,
            }
            guard = guard.saturating_sub(1);
            if guard == 0 {
                break;
            }
        }
        chain.reverse();
        chain
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn common_prefix_len<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Extract the `CommonItemPlacement` from any item that has one.
/// `Pop*` items, `PushShadow`, and `PushReferenceFrame` don't carry a
/// placement — they're handled separately.
fn item_placement(item: &ServalDisplayItem) -> Option<&CommonItemPlacement> {
    use ServalDisplayItem as S;
    match item {
        S::Rect(it) => Some(&it.placement),
        S::RectWithAnimation(it) => Some(&it.placement),
        S::Line(it) => Some(&it.placement),
        S::Image(it) => Some(&it.placement),
        S::ExternalTexture(it) => Some(&it.placement),
        S::RepeatingImage(it) => Some(&it.placement),
        S::Text(it) => Some(&it.placement),
        S::Border(it) => Some(&it.placement),
        S::BoxShadow(it) => Some(&it.placement),
        S::Gradient(it) => Some(&it.placement),
        S::RadialGradient(it) => Some(&it.placement),
        S::ConicGradient(it) => Some(&it.placement),
        S::Iframe(it) => Some(&it.placement),
        S::PushStackingContext(it) => Some(&it.placement),
        S::HitTest(it) => Some(&it.placement),
        // Stateful items without placement.
        S::PushShadow(_)
        | S::PopAllShadows
        | S::PopStackingContext
        | S::PushReferenceFrame(_)
        | S::PopReferenceFrame => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use paint_types::ColorF;
    use paint_types::PipelineId;
    use paint_types::SpatialId;
    use paint_types::units::{DeviceIntSize, LayoutPoint, LayoutRect, LayoutTransform};

    use super::*;
    use crate::serval_display_list::{
        ClipDef, ClipRectDef, CommonItemPlacement, PrimitiveFlags, ReferenceFrameDef,
        ServalDisplayList, SpatialNodeDef,
    };

    fn box2d(x: f32, y: f32, w: f32, h: f32) -> LayoutRect {
        LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
    }

    fn fresh() -> ServalDisplayList {
        ServalDisplayList::new(DeviceIntSize::new(800, 600), PipelineId::default())
    }

    fn placement_at(
        list: &ServalDisplayList,
        bounds: LayoutRect,
        clip: ClipChainId,
        spatial: SpatialId,
    ) -> CommonItemPlacement {
        let _ = list;
        CommonItemPlacement {
            clip_rect: bounds,
            clip_chain_id: clip,
            spatial_id: spatial,
            flags: PrimitiveFlags::HIT_TESTABLE,
        }
    }

    fn count<F: Fn(&ple::PaintCmd) -> bool>(cmds: &[ple::PaintCmd], pred: F) -> usize {
        cmds.iter().filter(|c| pred(c)).count()
    }

    fn is_push_transform(c: &ple::PaintCmd) -> bool {
        matches!(c, ple::PaintCmd::PushTransform(_))
    }
    fn is_pop_transform(c: &ple::PaintCmd) -> bool {
        matches!(c, ple::PaintCmd::PopTransform)
    }
    fn is_push_clip(c: &ple::PaintCmd) -> bool {
        matches!(c, ple::PaintCmd::PushClip(_))
    }
    fn is_pop_clip(c: &ple::PaintCmd) -> bool {
        matches!(c, ple::PaintCmd::PopClip)
    }

    #[test]
    fn empty_list_emits_no_commands() {
        let list = fresh();
        let out = lower_display_list(&list);
        assert!(out.is_empty(), "expected empty stream, got {:?}", out);
    }

    #[test]
    fn flat_root_only_list_emits_no_transitions() {
        let mut list = fresh();
        let root = list.root_spatial_id();
        let common = placement_at(&list, box2d(0.0, 0.0, 100.0, 50.0), ClipChainId::INVALID, root);
        list.push_rect(&common, box2d(0.0, 0.0, 100.0, 50.0), ColorF::default());
        list.push_rect(&common, box2d(10.0, 20.0, 80.0, 40.0), ColorF::default());

        let out = lower_display_list(&list);
        assert_eq!(count(&out, is_push_transform), 0);
        assert_eq!(count(&out, is_pop_transform), 0);
        assert_eq!(count(&out, is_push_clip), 0);
        assert_eq!(count(&out, is_pop_clip), 0);
    }

    #[test]
    fn reference_frame_spatial_emits_push_transform_pair() {
        let mut list = fresh();
        let root = list.root_spatial_id();
        // Define a child reference frame off root.
        let translation = list.define_transform(LayoutTransform::translation(50.0, 80.0, 0.0));
        let child_spatial = list.define_spatial_node(SpatialNodeDef::ReferenceFrame(
            ReferenceFrameDef {
                parent: root,
                origin: LayoutPoint::new(0.0, 0.0),
                transform: translation,
                kind: paint_types::ReferenceFrameKind::Transform,
            },
        ));

        // One rect in root, one in the child frame, one back in root.
        let root_placement = placement_at(
            &list,
            box2d(0.0, 0.0, 100.0, 100.0),
            ClipChainId::INVALID,
            root,
        );
        let child_placement = placement_at(
            &list,
            box2d(0.0, 0.0, 50.0, 50.0),
            ClipChainId::INVALID,
            child_spatial,
        );
        list.push_rect(&root_placement, box2d(0.0, 0.0, 100.0, 100.0), ColorF::default());
        list.push_rect(&child_placement, box2d(0.0, 0.0, 50.0, 50.0), ColorF::default());
        list.push_rect(&root_placement, box2d(0.0, 0.0, 100.0, 100.0), ColorF::default());

        let out = lower_display_list(&list);
        // Expect one Push+Pop pair around the middle item, and a final
        // Pop is NOT emitted (root stays on the stack).
        assert_eq!(count(&out, is_push_transform), 1, "stream: {out:?}");
        assert_eq!(count(&out, is_pop_transform), 1, "stream: {out:?}");

        // Order: rect, PushTransform, rect, PopTransform, rect.
        let positions: Vec<_> = out
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match c {
                ple::PaintCmd::DrawRect(_) => Some(("rect", i)),
                ple::PaintCmd::PushTransform(_) => Some(("push", i)),
                ple::PaintCmd::PopTransform => Some(("pop", i)),
                _ => None,
            })
            .collect();
        let tags: Vec<_> = positions.iter().map(|(t, _)| *t).collect();
        assert_eq!(tags, vec!["rect", "push", "rect", "pop", "rect"]);
    }

    #[test]
    fn rect_clip_emits_push_clip_pair() {
        let mut list = fresh();
        let root = list.root_spatial_id();
        let clip_id = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: root,
            rect: box2d(10.0, 10.0, 100.0, 50.0),
        }));

        let unclipped =
            placement_at(&list, box2d(0.0, 0.0, 200.0, 200.0), ClipChainId::INVALID, root);
        let clipped = placement_at(&list, box2d(20.0, 20.0, 50.0, 30.0), clip_id, root);

        list.push_rect(&unclipped, box2d(0.0, 0.0, 200.0, 200.0), ColorF::default());
        list.push_rect(&clipped, box2d(20.0, 20.0, 50.0, 30.0), ColorF::default());
        list.push_rect(&unclipped, box2d(0.0, 0.0, 200.0, 200.0), ColorF::default());

        let out = lower_display_list(&list);
        assert_eq!(count(&out, is_push_clip), 1, "stream: {out:?}");
        assert_eq!(count(&out, is_pop_clip), 1, "stream: {out:?}");
    }

    #[test]
    fn finish_unwinds_lingering_state() {
        let mut list = fresh();
        let root = list.root_spatial_id();
        let translation = list.define_transform(LayoutTransform::translation(10.0, 20.0, 0.0));
        let child = list.define_spatial_node(SpatialNodeDef::ReferenceFrame(
            ReferenceFrameDef {
                parent: root,
                origin: LayoutPoint::new(0.0, 0.0),
                transform: translation,
                kind: paint_types::ReferenceFrameKind::Transform,
            },
        ));
        let clip_id = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: root,
            rect: box2d(0.0, 0.0, 100.0, 100.0),
        }));

        let inside =
            placement_at(&list, box2d(0.0, 0.0, 50.0, 50.0), clip_id, child);
        list.push_rect(&inside, box2d(0.0, 0.0, 50.0, 50.0), ColorF::default());
        // No item returns to root: the walker should still close out
        // the clip + transform pushes in finish().

        let out = lower_display_list(&list);
        // 1 push_clip + 1 pop_clip (closed in finish), 1 push_transform
        // + 1 pop_transform (closed in finish).
        assert_eq!(count(&out, is_push_transform), 1, "stream: {out:?}");
        assert_eq!(count(&out, is_pop_transform), 1, "stream: {out:?}");
        assert_eq!(count(&out, is_push_clip), 1, "stream: {out:?}");
        assert_eq!(count(&out, is_pop_clip), 1, "stream: {out:?}");

        // Tail must be the pops, in clip-then-transform order
        // (clip first because we unwind it ahead of transform in
        // finish()).
        assert!(matches!(out.last(), Some(ple::PaintCmd::PopTransform)));
    }
}
