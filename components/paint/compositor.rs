/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! C4 — `ServoCompositor` and `StubCompositor`.
//!
//! Implements [`netrender_device::compositor::Compositor`], the trait
//! `netrender::Renderer::render_with_compositor` invokes once per frame
//! to hand back the master texture + per-surface `LayerPresent` slice.
//!
//! ## What's here
//!
//! - [`StubCompositor`] — single-fullscreen-surface fallback. Doesn't
//!   talk to the OS compositor; just stashes the master texture from
//!   the most recent `present_frame` call. The `composite_texture`
//!   path on `Paint` reads from this for the wgpu-shared-device
//!   embedder hand-off (`WebView::composite_texture()`).
//! - [`ServoCompositor`] — production wrapper that delegates to a
//!   per-platform [`OsCompositorBackend`]. Holds a
//!   [`HostWgpuContext`] (device + queue + detected backend) so
//!   backends can issue their own GPU work; per-platform
//!   synchronizers (e.g. [`crate::interop::Dx12FenceSynchronizer`])
//!   own the producer-fence / consumer-fence bookkeeping that
//!   survives the netrender → OS-compositor handoff.
//!
//! ## Direction note
//!
//! Pre-cut, the OS-handoff was webrender's job and lived inside its
//! renderer. C4 lifts the responsibility into a netrender-shaped
//! [`Compositor`] impl on the consumer side. The direction-neutral
//! interop primitives ([`InteropBackend`], [`HostWgpuContext`],
//! [`SyncMechanism`], the platform fence wrappers) live in
//! [`crate::interop`], extracted from `wgpu-native-texture-interop`'s
//! patterns but with no dep on it — WNTI's synchronizer trait shape
//! is import-direction-coupled (`&NativeFrame` / `&ImportedTexture`)
//! and doesn't fit the export path; rebuilding the small
//! direction-neutral foundation in serval is cleaner than adapting
//! around WNTI's import-shaped trait.
//!
//! See [`docs/2026-05-05_serval_netrender_cut_plan.md`](../../docs/2026-05-05_serval_netrender_cut_plan.md)
//! § C4 for the design.

use std::collections::HashMap;

use netrender_device::compositor::{Compositor, PresentedFrame, SurfaceKey};
use rustc_hash::FxHashMap;
use wgpu::Texture;

use crate::interop::{HostWgpuContext, InteropBackend, SyncMechanism};

/// A `Compositor` impl that captures the master texture from each
/// `present_frame` call so embedder code can read it back via
/// [`WgpuMasterCaptureBackend::last_master`].
///
/// This is the **wgpu-shared-device embedder route** — the embedder
/// holds the same wgpu device as netrender, so the master texture it
/// reads here is directly samplable in its own render pass (zero
/// copy). It's the right backend when the embedder wants to integrate
/// the serval composite into its own render pipeline (e.g. for a
/// custom UI shell that draws on top), and the wrong one when the
/// embedder wants serval to present pixels directly to the OS — for
/// that, install a per-platform backend ([`crate::compositor_dxgi::WindowsDxgiBackend`]
/// on Windows; `MacosCALayerBackend` on macOS;
/// `WaylandSubsurfaceBackend` on Linux).
///
/// Renamed from `StubCompositor`; the old name is retained as a
/// deprecated alias.
pub struct WgpuMasterCaptureBackend {
    /// Per-surface world bounds; updated on `declare_surface`.
    surfaces: FxHashMap<SurfaceKey, [f32; 4]>,
    /// Cloned handle to the most recently presented master texture.
    /// `wgpu::Texture` is `Arc`-shared internally so cloning is cheap.
    last_master: Option<Texture>,
}

/// Deprecated alias for [`WgpuMasterCaptureBackend`]. Retained for
/// source-compat with code written before the rename. Existing call
/// sites (tests, embedder smokes) continue to work; new code should
/// prefer the descriptive name.
#[deprecated(
    since = "0.2.0",
    note = "renamed to WgpuMasterCaptureBackend; this alias will be removed in a follow-up"
)]
pub type StubCompositor = WgpuMasterCaptureBackend;

impl WgpuMasterCaptureBackend {
    pub fn new() -> Self {
        Self {
            surfaces: FxHashMap::default(),
            last_master: None,
        }
    }

    /// Texture handed to the most recent `present_frame` call, if any.
    /// Used by `Paint::composite_texture` for the wgpu-shared-device
    /// embedder route.
    pub fn last_master(&self) -> Option<&Texture> {
        self.last_master.as_ref()
    }
}

impl Default for WgpuMasterCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Compositor for WgpuMasterCaptureBackend {
    fn declare_surface(&mut self, key: SurfaceKey, world_bounds: [f32; 4]) {
        self.surfaces.insert(key, world_bounds);
    }

    fn destroy_surface(&mut self, key: SurfaceKey) {
        self.surfaces.remove(&key);
    }

    fn present_frame(&mut self, frame: PresentedFrame<'_>) {
        // Capture the master so `Paint::composite_texture` can hand
        // it back to the wgpu-shared-device embedder.
        self.last_master = Some(frame.master.clone());
    }
}

/// Extension trait Paint stores compositors through. Adds the
/// `last_master()` accessor that [`Paint::composite_texture`] reads
/// — a method only [`WgpuMasterCaptureBackend`] meaningfully
/// implements. Default returns `None` so platform OS-handoff
/// backends ([`ServoCompositor`] / `WindowsDxgiBackend` etc.)
/// satisfy the trait without exposing a master-texture path that
/// wouldn't make sense for them (their pixels go to the OS
/// compositor, not back to the embedder's wgpu device).
pub trait PaintCompositor: Compositor + Send {
    /// Most recently captured master texture, if this backend
    /// captures masters. `None` for OS-handoff backends.
    fn last_master(&self) -> Option<&Texture> {
        None
    }
}

impl PaintCompositor for WgpuMasterCaptureBackend {
    fn last_master(&self) -> Option<&Texture> {
        WgpuMasterCaptureBackend::last_master(self)
    }
}

impl<B: OsCompositorBackend> PaintCompositor for ServoCompositor<B> {}

/// Per-platform OS-compositor backend. Implementations bridge
/// netrender's wgpu master texture into a native texture that the OS
/// compositor (DXGI Composition / CALayer / Wayland subsurface) can
/// consume.
///
/// The trait surface is parameterized over [`HostWgpuContext`] (so
/// backends can encode their own GPU copies onto the host's device+
/// queue) and [`SyncMechanism`] (so the producer→consumer fence
/// machinery is uniform across platforms; the per-platform
/// synchronizer in [`crate::interop`] drives the actual fence
/// signalling).
///
/// Per-platform impls live alongside their `OsCompositorBackend`
/// blanket: the Windows path is in
/// [`crate::compositor_dxgi::WindowsDxgiBackend`].
pub trait OsCompositorBackend: Send {
    /// Present the netrender master texture as the root visual /
    /// fullscreen surface of this backend's OS compositor handoff.
    /// Called from [`ServoCompositor::present_frame`] once per
    /// frame, before any per-`SurfaceKey` `present()` calls.
    ///
    /// Default impl is a no-op for backends that don't yet wire the
    /// master path (e.g. the trait shape compiles without forcing
    /// every platform to implement).
    fn present_master(&mut self, _master: &Texture) {}


    /// Which wgpu/native graphics backend this implementation targets.
    /// `ServoCompositor` cross-checks against the
    /// [`HostWgpuContext::backend`] at construction.
    fn interop_backend(&self) -> InteropBackend;

    /// Synchronization mechanism this backend speaks. Determines what
    /// fence/semaphore the per-frame `present` path coordinates.
    fn sync_mechanism(&self) -> SyncMechanism {
        SyncMechanism::None
    }

    /// Allocate a per-surface destination texture and register it
    /// with the OS compositor. `host` provides the wgpu device the
    /// destination texture should be allocated on.
    fn declare(&mut self, key: SurfaceKey, host: &HostWgpuContext, native: &Texture);

    /// Drop a previously-declared surface. After this, the OS
    /// compositor no longer references the surface.
    fn destroy(&mut self, key: SurfaceKey);

    /// Hand the surface's native texture to the OS compositor with
    /// the given world transform / clip / opacity. This corresponds
    /// to one entry in netrender's `present_frame` `layers` slice.
    fn present(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    );
}

/// Production compositor wrapper. Holds an [`OsCompositorBackend`], a
/// [`HostWgpuContext`] for GPU encode access, and a per-`SurfaceKey`
/// destination-texture pool. `present_frame` blits from the master
/// into the destination textures, then hands native handles to the
/// backend.
///
/// **C4 milestone:** trait shape only; the blit machinery lands with
/// the per-backend impls.
pub struct ServoCompositor<B: OsCompositorBackend> {
    host: HostWgpuContext,
    destinations: HashMap<SurfaceKey, Texture>,
    backend: B,
}

impl<B: OsCompositorBackend> ServoCompositor<B> {
    /// Construct a compositor over the given host wgpu context and
    /// backend. Panics if the backend's
    /// [`OsCompositorBackend::interop_backend`] doesn't match
    /// `host.backend` — these must agree at construction time so the
    /// per-platform GPU encode paths work.
    pub fn new(host: HostWgpuContext, backend: B) -> Self {
        assert_eq!(
            host.backend,
            backend.interop_backend(),
            "ServoCompositor: HostWgpuContext backend ({:?}) does not match OsCompositorBackend ({:?})",
            host.backend,
            backend.interop_backend(),
        );
        Self {
            host,
            destinations: HashMap::new(),
            backend,
        }
    }

    /// Reference to the underlying backend (for tests and debug
    /// inspection).
    pub fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: OsCompositorBackend> Compositor for ServoCompositor<B> {
    fn declare_surface(&mut self, _key: SurfaceKey, _world_bounds: [f32; 4]) {
        // Backend texture allocation lands with the per-platform impl.
        // Deferred: per-backend `declare` is invoked from
        // `present_frame` once a destination texture exists for the
        // surface.
    }

    fn destroy_surface(&mut self, key: SurfaceKey) {
        self.destinations.remove(&key);
        self.backend.destroy(key);
    }

    fn present_frame(&mut self, frame: PresentedFrame<'_>) {
        // Step 1 — present the master through the OS compositor.
        // Backends that don't override `present_master` get a no-op
        // and the StubCompositor-shaped behavior persists for them.
        self.backend.present_master(frame.master);

        // Step 2 — per-`SurfaceKey` blit + present. C4 milestone: not
        // wired yet. The flow when it lands:
        //  1. for each layer in frame.layers: lookup or allocate a
        //     destination texture in `self.destinations`, sized to the
        //     layer's source_rect.
        //  2. encode `copy_texture_to_texture(frame.master[rect] →
        //     destination)` via `frame.handles.queue`.
        //  3. for each backend-recognised platform handle, hand the
        //     native texture to the OS via
        //     `self.backend.present(key, transform, clip, opacity)`.
        let _ = (&self.host, &frame.layers);
    }
}
