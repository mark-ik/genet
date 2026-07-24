// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`SceneSlot`]: the Path-B leaf for hosts whose scene production is deeply
//! app-coupled (meerkat's orrery: graph, camera, layout strategies, gnode
//! pool). The build stays host-side; the slot carries the built scene through
//! the leaf contract, so it gains the retention gates (`paint_dirty`, size),
//! the epoch-gated host rasterize loop (`RenderedLeaves::scenes`), and the
//! spliced `DrawExternalTexture` placement, without copying app state into a
//! leaf. Widgets that own their inputs outright (a waveform over a sample
//! buffer) implement [`Leaf`] directly and paint into `cx.scene()` instead.

use crate::{Leaf, PaintCx, Size, SizeHint};

/// A slot the host pushes a pre-built [`vello::Scene`] into. Dirty only when a
/// new scene arrives, so unchanged frames repaint nothing and the host's
/// rasterize pass skips the stable epoch.
pub struct SceneSlot {
    pending: Option<vello::Scene>,
    intrinsic: Size,
    dirty: bool,
}

impl SceneSlot {
    pub fn new(intrinsic: Size) -> Self {
        Self {
            pending: None,
            intrinsic,
            dirty: false,
        }
    }

    /// Hand the slot this frame's scene. Call only when the producer actually
    /// rebuilt (its own dirt says so); an unchanged frame skips the call and
    /// the cached scene + texture ride through untouched.
    pub fn set_scene(&mut self, scene: vello::Scene) {
        self.pending = Some(scene);
        self.dirty = true;
    }
}

impl Leaf for SceneSlot {
    // No `accessibility()` on purpose. A scene slot is a blank surface the host
    // hands an arbitrary vello `Scene`; the slot itself knows nothing about what
    // was drawn into it, so any role or name it invented would be a guess. It
    // stays a plain container that the host names with `aria-label`, which is
    // strictly better than announcing an unnamed `Role::Image`.

    fn measure(&mut self, _known: SizeHint, _available: SizeHint) -> Size {
        self.intrinsic
    }

    fn paint(&mut self, cx: &mut PaintCx<'_>) {
        // A size-only repaint has no pending scene; the cache carries the
        // prior scene forward (render_into's Path-B carry).
        if let Some(scene) = self.pending.take() {
            cx.set_scene(scene);
        }
        self.dirty = false;
    }

    fn paint_dirty(&self) -> bool {
        self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LeafRegistry, RenderedLeaves};
    use paint_list_api::PaintCmd;

    fn size48() -> Size {
        Size {
            width: 48.0,
            height: 32.0,
        }
    }

    fn a_scene() -> vello::Scene {
        let mut s = vello::Scene::new();
        s.fill(
            vello::peniko::Fill::NonZero,
            vello::kurbo::Affine::IDENTITY,
            vello::peniko::Color::from_rgb8(200, 40, 40),
            None,
            &vello::kurbo::Rect::new(0.0, 0.0, 10.0, 10.0),
        );
        s
    }

    #[test]
    fn scene_slot_splices_external_texture_and_gates_epochs() {
        let mut reg: LeafRegistry<u64> = LeafRegistry::new();
        reg.insert(77, Box::new(SceneSlot::new(size48())));
        let mut out = RenderedLeaves::new();
        let sized = |_k: u64| Some(size48());

        // A fresh (uncached) leaf renders once by contract; the empty slot
        // yields an empty splice and no scene awaiting rasterize.
        assert_eq!(reg.render_into(sized, &mut out), 1);
        assert!(out.get(77).unwrap().is_empty());
        assert!(
            out.scenes().next().is_none(),
            "no scene until one is pushed"
        );
        assert_eq!(reg.render_into(sized, &mut out), 0, "then stable");

        // A pushed scene renders: the splice is one DrawExternalTexture whose
        // texture key IS the leaf key, at the local content box.
        reg.get_mut_as::<SceneSlot>(&77)
            .unwrap()
            .set_scene(a_scene());
        assert_eq!(reg.render_into(sized, &mut out), 1);
        let splice = out.get(77).expect("spliced");
        assert_eq!(splice.len(), 1);
        assert!(matches!(
            &splice[0],
            PaintCmd::DrawExternalTexture(et)
                if et.texture_key == 77
                    && (et.placement.bounds.max.x - 48.0).abs() < 0.01
        ));
        let (key, _scene, epoch, size) = out.scenes().next().expect("scene awaiting rasterize");
        assert_eq!((key, epoch), (77, 2));
        assert_eq!(size, size48());

        // Unchanged frame: no repaint, epoch stable (host skips rasterize).
        assert_eq!(reg.render_into(sized, &mut out), 0);
        assert_eq!(out.scenes().next().unwrap().2, 2, "epoch stable");

        // New scene: repaint, epoch bumps (host re-rasterizes).
        reg.get_mut_as::<SceneSlot>(&77)
            .unwrap()
            .set_scene(a_scene());
        assert_eq!(reg.render_into(sized, &mut out), 1);
        assert_eq!(out.scenes().next().unwrap().2, 3, "epoch moved");

        // Size-only repaint (relayout, no new scene): the prior scene carries
        // forward, the splice re-sizes, the epoch bumps so the host
        // re-rasterizes at the new size.
        let bigger = |_k: u64| {
            Some(Size {
                width: 64.0,
                height: 40.0,
            })
        };
        assert_eq!(reg.render_into(bigger, &mut out), 1);
        let (_, scene, epoch, size) = out.scenes().next().unwrap();
        assert_eq!(epoch, 4);
        assert_eq!(size.width, 64.0);
        assert!(!scene.encoding().is_empty(), "prior scene carried forward");
        let splice = out.get(77).unwrap();
        assert!(matches!(
            &splice[0],
            PaintCmd::DrawExternalTexture(et)
                if (et.placement.bounds.max.x - 64.0).abs() < 0.01
        ));
    }
}
