// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sprigging supplies retained custom-paint leaves for GUI hosts.
//!
//! A small, sharp contract that lets an imperative custom-paint widget (a knob,
//! meter, waveform, graph canvas) live as one first-class serval element,
//! plugging into serval's four existing passes rather than standing up a second
//! UI engine. Design and rationale:
//! `docs/history/2026-07-07_chisel_widget_leaf_design.md`.
//!
//! This is the Path-A scaffold: a leaf paints by pushing common `PaintCmd`s
//! (portable, tile-cached) which serval splices into its paint list at the
//! leaf's box. Path B (a leaf renders its own `vello::Scene` which the host
//! rasterizes to a texture and places via `DrawExternalTexture`) lands with the
//! first imperative consumer (the meerkat orrery port).
//!
//! Sprigging is engine-neutral: it targets the paint seam (`paint_list_api`) and
//! accesskit, not the serval engine concretely. The retained per-leaf state
//! lives in a node-keyed [`LeafRegistry`], mirroring serval's external-texture /
//! font / image registries, so the DOM stays uniform.

use std::collections::HashMap;
use std::hash::Hash;

use paint_list_api::items::PathItem;
use paint_list_api::{CommonPlacement, LayoutPoint, LayoutRect, RectItem};

mod arrange;
mod glyphs;
mod grid;
mod path;
mod slot;

pub use arrange::{Placement, VirtualWindow};
pub use glyphs::{GraphGlyph, GraphGlyphNode, Knob, Meter};
pub use grid::{GridColumn, GridSpec};
pub use path::Path;
pub use slot::SceneSlot;
// The paint vocabulary leaves author against, re-exported (`pub use` also
// imports, so these serve the crate body too) so a leaf crate or a host wiring
// leaves needs no direct paint_list_api dep for the common cases.
pub use paint_list_api::items::{DashPattern, PathData, StrokeCap, StrokeJoin, StrokeStyle};
pub use paint_list_api::{ColorF, PaintCmd};

/// Known / available size passed to [`Leaf::measure`] (device px). `None` means
/// the dimension is unconstrained, matching CSS `auto` / indefinite space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SizeHint {
    pub width: Option<f32>,
    pub height: Option<f32>,
}

/// An intrinsic size a leaf reports back (device px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// Paint context handed to [`Leaf::paint`], covering both paint paths. Path A:
/// the leaf pushes common [`PaintCmd`]s, spliced into serval's paint list at
/// the leaf's box. Path B: the leaf encodes a [`vello::Scene`]
/// ([`scene`](Self::scene) / [`set_scene`](Self::set_scene)); the cache then
/// splices a single `DrawExternalTexture` carrying the leaf's key, and the
/// host rasterizes the scene into the texture it registers under that key
/// (Sprigging stays GPU-free). Local coordinates, `(0,0)` at the content-box
/// top-left, on both paths.
pub struct PaintCx<'a> {
    cmds: &'a mut Vec<PaintCmd>,
    scene: Option<vello::Scene>,
    size: Size,
}

impl<'a> PaintCx<'a> {
    pub fn new(cmds: &'a mut Vec<PaintCmd>, size: Size) -> Self {
        Self {
            cmds,
            scene: None,
            size,
        }
    }

    /// Path B: the leaf's own vello scene (created empty on first call).
    /// Painting into this makes the leaf a scene leaf for this repaint; any
    /// Path-A commands pushed alongside are ignored.
    pub fn scene(&mut self) -> &mut vello::Scene {
        self.scene.get_or_insert_with(vello::Scene::new)
    }

    /// Path B, whole-scene form: hand over an already-built scene (the
    /// [`SceneSlot`] shape, for hosts whose scene production is app-coupled).
    pub fn set_scene(&mut self, scene: vello::Scene) {
        self.scene = Some(scene);
    }

    pub(crate) fn take_scene(&mut self) -> Option<vello::Scene> {
        self.scene.take()
    }

    /// The leaf's box size in local coordinates.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Path A: emit one common paint command.
    pub fn emit(&mut self, cmd: PaintCmd) {
        self.cmds.push(cmd);
    }

    /// Convenience: fill a local rect with a solid color. Mirrors serval-layout
    /// `paint_emit`'s own `DrawRect` construction so the produced command is
    /// byte-identical to a native background fill.
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: ColorF) {
        self.cmds.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(LayoutRect::new(
                LayoutPoint::new(x, y),
                LayoutPoint::new(x + w, y + h),
            )),
            color,
        }));
    }

    /// Draw a path with an optional fill and optional stroke (CSS/SVG
    /// "filled then stroked"). Placement is the leaf's own box.
    pub fn path(&mut self, path: PathData, fill: Option<ColorF>, stroke: Option<StrokeStyle>) {
        let s = self.size;
        self.cmds.push(PaintCmd::DrawPath(PathItem {
            placement: CommonPlacement::new(LayoutRect::new(
                LayoutPoint::new(0.0, 0.0),
                LayoutPoint::new(s.width, s.height),
            )),
            path,
            fill,
            stroke,
        }));
    }

    /// Fill a path with a solid color.
    pub fn fill_path(&mut self, path: PathData, color: ColorF) {
        self.path(path, Some(color), None);
    }

    /// Stroke a path.
    pub fn stroke_path(&mut self, path: PathData, style: StrokeStyle) {
        self.path(path, None, Some(style));
    }
}

/// A solid stroke: butt caps, miter joins, no dash.
pub fn solid_stroke(color: ColorF, width: f32) -> StrokeStyle {
    StrokeStyle {
        color,
        width,
        cap: StrokeCap::Butt,
        join: StrokeJoin::Miter,
        dash: None,
    }
}

/// A round-capped, round-joined solid stroke (polylines, arcs, needles).
pub fn round_stroke(color: ColorF, width: f32) -> StrokeStyle {
    StrokeStyle {
        color,
        width,
        cap: StrokeCap::Round,
        join: StrokeJoin::Round,
        dash: None,
    }
}

/// An input event forwarded from serval's hit-test. Placeholder shape; the real
/// pointer / key vocabulary wires to Cambium's `dispatch_click` /
/// `dispatch_key` when the input seam is built out.
#[derive(Clone, Copy, Debug)]
pub struct LeafEvent;

/// An action a leaf bubbles up the Cambium message cycle (the retained-view
/// split: internal interaction stays local, semantic change routes
/// up). Placeholder until the action seam is built out.
#[derive(Clone, Copy, Debug)]
pub struct LeafAction;

/// A custom-paint widget that plugs into serval's four passes as one node.
///
/// The retention gates are two signals, because they gate different passes:
/// [`paint_dirty`](Leaf::paint_dirty) gates repaint,
/// [`layout_dirty`](Leaf::layout_dirty) gates serval's relayout.
///
/// `Any` supertrait: hosts push live values into registered leaves through
/// [`LeafRegistry::get_mut_as`] (a meter fed each frame, a glyph fed on model
/// change), so the registry can hand back the concrete type.
pub trait Leaf: std::any::Any {
    /// Intrinsic sizing. Feeds serval's replaced-element `replaced_intrinsic_size`
    /// (a custom leaf is a replaced element, like `<img>` / `<external-texture>`).
    fn measure(&mut self, known: SizeHint, available: SizeHint) -> Size;

    /// Paint. Path A: push [`PaintCmd`]s via `cx`. (Path B lands later.)
    fn paint(&mut self, cx: &mut PaintCx<'_>);

    /// serval hit-test forwards here. Internal interaction mutates `self` and
    /// marks paint-dirty; a semantic change returns an action routed up the
    /// message cycle. Default: inert.
    fn event(&mut self, _ev: &LeafEvent) -> Option<LeafAction> {
        None
    }

    /// Fill this node's accesskit node during serval-layout's `accesskit_tree`
    /// walk (a knob still announces as a slider). Default: no semantics.
    fn accessibility(&mut self, _node: &mut accesskit::Node) {}

    /// Retention gate: has paint output changed since last frame?
    fn paint_dirty(&self) -> bool;

    /// Retention gate: has intrinsic size or (for an arrangement leaf) child
    /// placement changed? Default: false (a pure paint leaf never relayouts).
    fn layout_dirty(&self) -> bool {
        false
    }
}

/// Host-owned retained state, keyed by node. Mirrors serval's external-texture /
/// font / image registries: the DOM stays uniform `NodeId`s, and each leaf's
/// retained struct lives here rather than in the ephemeral view tree. Generic
/// over the host's node key so Sprigging stays engine-neutral.
pub struct LeafRegistry<K: Eq + Hash> {
    leaves: HashMap<K, Box<dyn Leaf>>,
}

impl<K: Eq + Hash> Default for LeafRegistry<K> {
    fn default() -> Self {
        Self {
            leaves: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash> LeafRegistry<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: K, leaf: Box<dyn Leaf>) {
        self.leaves.insert(key, leaf);
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut (dyn Leaf + 'static)> {
        self.leaves.get_mut(key).map(|b| b.as_mut())
    }

    /// Typed access to a registered leaf (trait upcast to `Any`, then
    /// downcast): the host-side channel for pushing live values into a leaf
    /// (`meter.set_level(...)` each frame). `None` if absent or of another type.
    pub fn get_mut_as<T: Leaf>(&mut self, key: &K) -> Option<&mut T> {
        self.leaves
            .get_mut(key)
            .and_then(|b| (b.as_mut() as &mut dyn std::any::Any).downcast_mut::<T>())
    }

    pub fn remove(&mut self, key: &K) -> Option<Box<dyn Leaf>> {
        self.leaves.remove(key)
    }

    /// Retain only leaves whose keys satisfy `keep`.
    ///
    /// Hosts call this after reconciling model-owned leaf families so removed
    /// tracks, rows, or nodes do not leave retained widget state behind.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.leaves.retain(|key, _| keep(key));
    }

    pub fn contains(&self, key: &K) -> bool {
        self.leaves.contains_key(key)
    }
}

/// Rendered Path-A command buffers, keyed by leaf key. This is the **leaf-tier
/// paint cache**: the third of the four retention gates (view `memoize`,
/// `IncrementalLayout`, this, netrender tile cache). A leaf re-renders only when
/// it reports `paint_dirty` or has no buffer yet; an unchanged leaf keeps its
/// cached commands. Neutral: only `paint_list_api` types, so a serval-side
/// adapter can expose it through the layout engine's `LeafPaintSource`.
/// One leaf's rendered output: the splice commands (Path A: the leaf's own
/// stream; Path B: one `DrawExternalTexture` at the box), plus the scene and
/// its epoch for Path-B leaves.
struct RenderedLeaf {
    size: Size,
    splice: Vec<PaintCmd>,
    /// Path-B only: the leaf's encoded vello scene, awaiting host rasterize.
    scene: Option<vello::Scene>,
    /// Bumped on every repaint of this entry; the host re-rasterizes a scene
    /// only when its `(epoch, size)` moved — the retention chain extended
    /// through the GPU handoff.
    epoch: u64,
}

#[derive(Default)]
pub struct RenderedLeaves {
    map: HashMap<u64, RenderedLeaf>,
}

impl RenderedLeaves {
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached splice commands for `key`, or `None` if the leaf has not been
    /// rendered (no size available, or never present). This is exactly the shape
    /// a serval-side `LeafPaintSource` newtype forwards to.
    pub fn get(&self, key: u64) -> Option<&[PaintCmd]> {
        self.map.get(&key).map(|r| r.splice.as_slice())
    }

    /// The Path-B scenes awaiting host rasterize: `(key, scene, epoch, size)`.
    /// The host rasterizes into the texture it registers under `key` when
    /// `(epoch, size)` moved since its last pass, and skips the rest.
    pub fn scenes(&self) -> impl Iterator<Item = (u64, &vello::Scene, u64, Size)> {
        self.map
            .iter()
            .filter_map(|(k, r)| r.scene.as_ref().map(|s| (*k, s, r.epoch, r.size)))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Drop cached buffers for leaves the predicate rejects. The host calls this
    /// after a reactive diff removes leaf nodes, so the cache does not leak.
    pub fn retain_keys(&mut self, keep: impl Fn(u64) -> bool) {
        self.map.retain(|k, _| keep(*k));
    }
}

impl LeafRegistry<u64> {
    /// Re-render leaves into `out`, keyed by leaf key, running each leaf's
    /// `paint` when it is `paint_dirty`, has no cached buffer yet, **or** its box
    /// size changed since the cached buffer was painted — the leaf-tier retention
    /// gate. The size check catches container-driven relayouts (percentage / flex
    /// width, window resize) that resize the box without the leaf flipping
    /// `paint_dirty`. `size_of` supplies each leaf's content-box size from the
    /// host's completed layout; a leaf with no size (not laid out this frame) is
    /// skipped. Returns the count of leaves actually (re)painted, so the host can
    /// tell whether the paint list changed.
    pub fn render_into(
        &mut self,
        size_of: impl Fn(u64) -> Option<Size>,
        out: &mut RenderedLeaves,
    ) -> usize {
        use paint_list_api::items::ExternalTextureItem;
        let mut painted = 0;
        for (key, leaf) in self.leaves.iter_mut() {
            let Some(size) = size_of(*key) else {
                continue;
            };
            let prior = out.map.get(key);
            let cached_size = prior.map(|r| r.size);
            if !leaf.paint_dirty() && cached_size == Some(size) {
                continue;
            }
            let prior_epoch = prior.map(|r| r.epoch).unwrap_or(0);
            let prior_scene = out.map.remove(key).and_then(|r| r.scene);
            let mut cmds = Vec::new();
            let mut cx = PaintCx::new(&mut cmds, size);
            leaf.paint(&mut cx);
            // A scene repaint yields a fresh scene; a size-only repaint of a
            // scene leaf (relayout, no new content) carries the prior scene
            // forward so the host re-rasterizes it at the new size.
            let scene = cx.take_scene().or(prior_scene);
            let entry = match scene {
                Some(scene) => RenderedLeaf {
                    size,
                    // Path B splices one external-texture draw at the box; the
                    // texture key is the leaf key (the host registers its
                    // rasterized texture under the same key).
                    splice: vec![PaintCmd::DrawExternalTexture(ExternalTextureItem {
                        placement: CommonPlacement::new(LayoutRect::new(
                            LayoutPoint::new(0.0, 0.0),
                            LayoutPoint::new(size.width, size.height),
                        )),
                        texture_key: *key,
                        opacity: 1.0,
                        content_generation: None,
                    })],
                    scene: Some(scene),
                    epoch: prior_epoch + 1,
                },
                None => RenderedLeaf {
                    size,
                    splice: cmds,
                    scene: None,
                    epoch: prior_epoch + 1,
                },
            };
            out.map.insert(*key, entry);
            painted += 1;
        }
        painted
    }
}

/// The trivial first catalog leaf: a solid-color swatch. It proves the Path-A
/// pipeline end to end (measure -> emit one `DrawRect`) and exercises the
/// `paint_dirty` gate.
pub struct Swatch {
    pub color: ColorF,
    pub intrinsic: Size,
    dirty: bool,
}

impl Swatch {
    pub fn new(color: ColorF, intrinsic: Size) -> Self {
        Self {
            color,
            intrinsic,
            dirty: true,
        }
    }

    /// Recolor and mark paint-dirty so the next frame repaints.
    pub fn set_color(&mut self, color: ColorF) {
        self.color = color;
        self.dirty = true;
    }
}

/// `ColorF` (linear-ish f32 channels) to AccessKit's 8-bit `Color`.
fn access_color(color: ColorF) -> accesskit::Color {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    accesskit::Color {
        red: channel(color.r),
        green: channel(color.g),
        blue: channel(color.b),
        alpha: channel(color.a),
    }
}

impl Leaf for Swatch {
    /// A swatch announces as [`Role::ColorWell`](accesskit::Role::ColorWell)
    /// carrying the color it shows, which is the one fact assistive tech cannot
    /// recover from the pixels. Presentational on its own: the host decides
    /// whether a swatch is pickable and labels it, so no action is declared here.
    fn accessibility(&mut self, node: &mut accesskit::Node) {
        node.set_role(accesskit::Role::ColorWell);
        node.set_color_value(access_color(self.color));
    }

    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        let s = cx.size();
        cx.fill_rect(0.0, 0.0, s.width, s.height, self.color);
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A leaf whose dirtiness is externally observable, so the retention gate can
    /// be driven from the test.
    struct FlagLeaf {
        dirty: Rc<Cell<bool>>,
    }

    impl Leaf for FlagLeaf {
        fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
            Size {
                width: 8.0,
                height: 8.0,
            }
        }
        fn paint(&mut self, cx: &mut PaintCx<'_>) {
            cx.fill_rect(0.0, 0.0, 8.0, 8.0, ColorF::WHITE);
            self.dirty.set(false);
        }
        fn paint_dirty(&self) -> bool {
            self.dirty.get()
        }
    }

    #[test]
    fn render_into_respects_the_paint_dirty_gate() {
        let dirty = Rc::new(Cell::new(true));
        let mut reg: LeafRegistry<u64> = LeafRegistry::new();
        reg.insert(
            3,
            Box::new(FlagLeaf {
                dirty: dirty.clone(),
            }),
        );
        let mut out = RenderedLeaves::new();
        let sized = |_k: u64| {
            Some(Size {
                width: 8.0,
                height: 8.0,
            })
        };

        // Fresh + dirty -> painted.
        assert_eq!(
            reg.render_into(sized, &mut out),
            1,
            "fresh dirty leaf paints"
        );
        assert!(out.get(3).is_some());

        // Clean + cached -> skipped (the gate).
        assert_eq!(
            reg.render_into(sized, &mut out),
            0,
            "clean, cached leaf is not repainted"
        );

        // Size change without paint_dirty (container relayout) -> repainted.
        let bigger = |_k: u64| {
            Some(Size {
                width: 16.0,
                height: 16.0,
            })
        };
        assert_eq!(
            reg.render_into(bigger, &mut out),
            1,
            "a resized leaf repaints even when it is not paint_dirty"
        );
        assert_eq!(
            reg.render_into(bigger, &mut out),
            0,
            "and is stable once cached at the new size"
        );

        // Re-dirtied -> repainted.
        dirty.set(true);
        assert_eq!(
            reg.render_into(sized, &mut out),
            1,
            "a re-dirtied leaf repaints"
        );

        // No size (not laid out this frame) -> skipped even when dirty.
        dirty.set(true);
        let no_size = |_k: u64| None;
        assert_eq!(
            reg.render_into(no_size, &mut out),
            0,
            "a leaf with no layout size is skipped"
        );

        // retain_keys drops stale buffers.
        out.retain_keys(|k| k != 3);
        assert!(out.get(3).is_none());

        reg.insert(
            4,
            Box::new(FlagLeaf {
                dirty: dirty.clone(),
            }),
        );
        reg.retain(|key| *key == 4);
        assert!(!reg.contains(&3));
        assert!(reg.contains(&4));
    }

    #[test]
    fn swatch_measures_paints_and_clears_dirty() {
        let mut reg: LeafRegistry<u64> = LeafRegistry::new();
        reg.insert(
            1,
            Box::new(Swatch::new(
                ColorF::WHITE,
                Size {
                    width: 10.0,
                    height: 4.0,
                },
            )),
        );

        let leaf = reg.get_mut(&1).expect("leaf present");
        assert!(leaf.paint_dirty(), "a fresh leaf starts paint-dirty");

        let size = leaf.measure(SizeHint::default(), SizeHint::default());
        assert_eq!(
            size,
            Size {
                width: 10.0,
                height: 4.0
            }
        );

        let mut cmds = Vec::new();
        let mut cx = PaintCx::new(&mut cmds, size);
        leaf.paint(&mut cx);

        assert_eq!(cmds.len(), 1, "swatch emits exactly one rect");
        assert!(matches!(cmds[0], PaintCmd::DrawRect(_)));
        assert!(!leaf.paint_dirty(), "paint clears the paint-dirty gate");
    }
}
