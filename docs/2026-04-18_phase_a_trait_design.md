# Phase A — `RenderingContext` Trait Design Spec

**Companion to**:

- [`2026-04-18_servo_wgpuification_plan.md`](2026-04-18_servo_wgpuification_plan.md) — plan
- [`2026-04-18_phase_a_rendering_context_audit.md`](2026-04-18_phase_a_rendering_context_audit.md) — consumer audit + addendum

**Purpose**: concrete trait shapes for Phase A. This is the design to argue with before touching code.

---

## Design premise

The existing `RenderingContext` trait is partly GL-shaped (`make_current`, `gleam_gl_api`, `glow_gl_api`, `create_texture`, `destroy_texture`, `connection`, `prepare_for_rendering`) and partly wgpu-shaped (`wgpu_device`, `wgpu_queue`, `wgpu_hal_device_factory`, `acquire_wgpu_frame_target`). Two of the GL methods (`gleam_gl_api`, `glow_gl_api`) are required with no default; the wgpu methods are all optional with `None` defaults. This makes every impl pretend GL exists, forces `WgpuRenderingContext` to carry `unreachable!()` panic surfaces, and scatters capability-check duct tape across consumers.

Phase A inverts this: **capabilities are optional objects accessible via `Option<&dyn _Capability>` getters on a minimal core trait**. The core trait holds only what every rendering context must do (geometry, presentation, readback, window handles). GL and wgpu specifics live on capability traits. Consumers explicitly match on which capability they need; the type system prevents forgetting.

---

## Core trait

```rust
/// Minimal contract a rendering context must satisfy. Host-neutral:
/// a wgpu-first, GL-first, or software-only backend all implement
/// this identically. Backend-specific surface lives behind the
/// capability getters below.
pub trait RenderingContextCore {
    // --- Geometry + presentation (all backends) ---

    fn size(&self) -> PhysicalSize<u32>;

    fn size2d(&self) -> Size2D<u32, DevicePixel> {
        let s = self.size();
        Size2D::new(s.width, s.height)
    }

    fn resize(&self, size: PhysicalSize<u32>);

    fn present(&self);

    /// Read the current back-buffer into an in-memory image. Backend-neutral:
    /// GL impls use `glReadPixels`; wgpu impls use a staging buffer + map-read.
    /// Returns `None` if readback isn't available (e.g. the wgpu impl
    /// currently has a `TODO` stub — tracked separately from Phase A).
    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage>;

    // --- Window integration (optional; offscreen contexts return None) ---

    /// Raw window + display handles, bundled. Needed for wgpu surface
    /// creation and Surfman native-widget wrapping. Offscreen / headless
    /// contexts return `None`.
    fn window_handles(&self) -> Option<WindowHandles> {
        None
    }

    /// Host-provided refresh driver; `None` means the default timer-based
    /// driver is used. Not backend-specific.
    fn refresh_driver(&self) -> Option<Rc<dyn RefreshDriver>> {
        None
    }

    // --- Capability objects ---

    /// wgpu capability — required for any context driving the wgpu
    /// compositor path. `None` for pure-GL / software legacy contexts.
    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        None
    }

    /// GL capability — required for WebGL, WebXR (current), and
    /// `egui_glow`-style embedder chrome. `None` for wgpu-first
    /// contexts that never expose GL.
    fn gl(&self) -> Option<&dyn GlCapability> {
        None
    }
}

/// Bundled raw window + display handles for creating a platform surface.
pub struct WindowHandles {
    pub window: raw_window_handle::RawWindowHandle,
    pub display: raw_window_handle::RawDisplayHandle,
}
```

### Design notes

- **Only four methods are required** (`size`, `resize`, `present`, `read_to_image`). The current trait requires 7 (adds `make_current`, `gleam_gl_api`, `glow_gl_api`). The reduction is the point.
- **`size2d` is a provided default** deriving from `size`. Same as today.
- **Window handles bundled** into one `Option<WindowHandles>` method rather than two separate `Option<RawWindowHandle>` / `Option<RawDisplayHandle>` getters. They're always used together for wgpu surface creation; separating them forces every caller to double-unwrap.
- **No `backend_binding()` enum** — the capability getters replace it. Callers that currently match `RenderingBackendBinding::Gl(...) | RenderingBackendBinding::Wgpu(...)` become `if let Some(wgpu) = ctx.wgpu() { ... } else if let Some(gl) = ctx.gl() { ... }`. Same information, no enum, no `unreachable!` panics when a caller forgets which variant it's holding.

---

## Wgpu capability

```rust
/// Capability surface for wgpu-backed rendering contexts. Accessed via
/// [`RenderingContextCore::wgpu`]. Holding an `&dyn WgpuCapability`
/// proves at the type level that the context can drive a wgpu compositor.
pub trait WgpuCapability {
    /// Clone of the context's wgpu device. The context's device handle
    /// is internally `Arc`-shared, so cloning is cheap and returned
    /// handles operate on the same GPU context.
    fn device(&self) -> wgpu::Device;

    /// Clone of the context's wgpu queue. Paired with `device()`.
    fn queue(&self) -> wgpu::Queue;

    /// Acquire the next swapchain texture for this frame. Wgpu equivalent
    /// of GL's `prepare_for_rendering` — returns the texture view the
    /// compositor should draw into. `None` means the swapchain couldn't
    /// acquire a target (lost / suboptimal surface).
    fn acquire_frame_target(&self) -> Option<wgpu::TextureView>;

    /// Optional factory hook for embedders that hold a raw `wgpu_hal`
    /// device and want WebRender to wrap it via `Adapter::create_device_from_hal`
    /// rather than creating its own device stack. Called at most once
    /// during `create_webrender_instance_with_backend`; takes `&self`
    /// but uses interior mutability to consume the factory.
    ///
    /// Takes precedence over `device()`/`queue()` when both are provided.
    fn hal_device_factory(
        &self,
    ) -> Option<Box<dyn FnOnce() -> (wgpu::Device, wgpu::Queue) + Send>> {
        None
    }
}
```

### Design notes

- **`device()` and `queue()` return owned clones**, not references. Matches the current trait's behavior (`wgpu::Device`/`wgpu::Queue` are Arc-shared internally). Consumers that need both always call both, so the structural cost is a two-method call instead of a one-method call returning a `WgpuBinding` struct. The `WgpuBinding` wrapper type goes away.
- **`acquire_frame_target` replaces `prepare_for_rendering` on the wgpu path.** The semantics are now explicit: "get me the texture I'm about to draw into." Painter code that currently calls `prepare_for_rendering()` on a wgpu context (no-op) will become `ctx.wgpu().and_then(|w| w.acquire_frame_target())` where it actually needs the target, or drop the call entirely where it doesn't.
- **`hal_device_factory` keeps its current shape** (`Option<Box<FnOnce>>` with `&self` + interior mutability). Pre-existing API wart; not worth widening Phase A to fix.

---

## GL capability

```rust
/// Capability surface for GL-backed rendering contexts. Accessed via
/// [`RenderingContextCore::gl`]. Holding an `&dyn GlCapability` proves
/// at the type level that the context has a current GL context available.
pub trait GlCapability {
    /// Make the GL context current on the calling thread.
    fn make_current(&self) -> Result<(), Error>;

    /// The `gleam`-flavored GL API handle.
    fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl>;

    /// The `glow`-flavored GL API handle.
    fn glow_gl_api(&self) -> Arc<glow::Context>;

    /// Prepare the GL framebuffer for rendering (binds the Surfman-owned
    /// framebuffer). No wgpu equivalent; wgpu's frame target is acquired
    /// per-frame via `WgpuCapability::acquire_frame_target` instead.
    fn prepare_for_rendering(&self);

    /// Wrap a Surfman surface as a GL surface texture. Returns the
    /// surface texture, the underlying GL texture name, and the size.
    /// Used by the GL external-image import path (WebGL → compositor).
    /// `None` if the backend doesn't support surface-texture wrapping.
    fn create_texture(
        &self,
        surface: Surface,
    ) -> Option<(SurfaceTexture, u32, UntypedSize2D<i32>)>;

    /// Release a previously created surface texture and return the
    /// underlying Surfman surface for recycling.
    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface>;

    /// The Surfman connection backing this GL context. Non-optional on
    /// `GlCapability`: if you have a GL capability, you have a connection.
    /// (On the old trait, `connection()` returned `Option<Connection>`
    /// because wgpu-only impls had to implement the method.)
    fn connection(&self) -> Connection;
}
```

### Design notes

- **`connection()` becomes non-optional.** Under the old trait, every impl had to implement it even if they had no Surfman connection; the wgpu-only impls returned `None`. Under the split, calling `connection()` is gated by first obtaining a `GlCapability`, so the `Option` indirection is moved to the capability getter where it belongs.
- **`create_texture` and `destroy_texture` stay on `GlCapability`** even though Phase C will eventually move them behind `ExternalImageImporter`. Phase A's job is the split; Phase C's job is the lease abstraction. Keeping these here means Phase A doesn't interfere with Phase C's future shape.
- **`prepare_for_rendering` lands here** per the audit addendum Q1 resolution. Two of three call sites are already GL-gated; the third becomes `if let Some(gl) = ctx.gl() { gl.prepare_for_rendering(); }`.

---

## Capability-impl ownership pattern

Concrete `RenderingContext` impls today either own GL state, wgpu state, or both (via `OffscreenRenderingContext`'s parent-context wrapper). Under the split, the impl types grow capability shims:

```rust
// Example: WindowRenderingContext (GL-backed, existing)
pub struct WindowRenderingContext {
    // existing fields: surfman_context, swap_chain, ...
}

impl RenderingContextCore for WindowRenderingContext {
    fn size(&self) -> PhysicalSize<u32> { /* ... */ }
    fn resize(&self, size: PhysicalSize<u32>) { /* ... */ }
    fn present(&self) { /* ... */ }
    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage> { /* ... */ }
    fn window_handles(&self) -> Option<WindowHandles> { /* ... */ }

    fn gl(&self) -> Option<&dyn GlCapability> {
        Some(self)
    }
}

impl GlCapability for WindowRenderingContext {
    fn make_current(&self) -> Result<(), Error> { /* existing */ }
    // ... etc
}
```

```rust
// Example: WgpuRenderingContext (wgpu-first, existing)
impl RenderingContextCore for WgpuRenderingContext {
    fn size(&self) -> PhysicalSize<u32> { /* ... */ }
    // ... etc

    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        Some(self)
    }
    // gl() uses the default `None`
}

impl WgpuCapability for WgpuRenderingContext {
    fn device(&self) -> wgpu::Device { self.device.clone() }
    fn queue(&self) -> wgpu::Queue { self.queue.clone() }
    fn acquire_frame_target(&self) -> Option<wgpu::TextureView> { /* existing body */ }
}
```

### Design notes

- **Returning `Some(self)` from the capability getter works** because the return type is `Option<&dyn _Capability>` and `Self: _Capability`. Standard Rust pattern; no lifetime gymnastics needed.
- **No double-dispatch overhead in hot paths.** The capability getter is a single virtual call; consumers pattern on `Some(cap)` once and hold `&dyn Capability` for the duration of the operation.
- **`OffscreenRenderingContext`'s parent-forwarding pattern** continues to work: it holds a parent `Rc<dyn RenderingContextCore>` and forwards `gl()`/`wgpu()` calls to the parent. One line of delegation per capability instead of ~6 lines of per-method forwarding.

---

## Consumer migration patterns

### Pattern A: pre-split

```rust
// Currently in painter.rs:170-180
let webrender_gl = if is_wgpu {
    None
} else {
    let gl = rendering_context.gleam_gl_api();
    if let Err(err) = rendering_context.make_current() {
        warn!("Failed to make the rendering context current: {:?}", err);
    }
    Some(gl)
};
```

### Pattern A: post-split

```rust
let webrender_gl = rendering_context.gl().and_then(|gl| {
    if let Err(err) = gl.make_current() {
        warn!("Failed to make the rendering context current: {:?}", err);
        None
    } else {
        Some(gl.gleam_gl_api())
    }
});
```

The `is_wgpu` flag disappears — `ctx.gl().is_some()` is the canonical check.

### Pattern B: pre-split

```rust
// Currently in paint.rs:239 — opportunistic Surfman init
if let Some(connection) = rendering_context.connection() {
    let adapter = connection.create_adapter().expect("Failed to create adapter");
    // ...
}
```

### Pattern B: post-split

```rust
if let Some(gl) = rendering_context.gl() {
    let connection = gl.connection();
    let adapter = connection.create_adapter().expect("Failed to create adapter");
    // ...
}
```

Same shape, clearer name. The capability getter reads as "do we have GL capability" instead of "do we happen to have a Surfman connection lying around."

### Pattern C: pre-split

```rust
// Currently in screenshot.rs:200
if let Err(error) = renderer.rendering_context.make_current() {
    error!("Failed to make the rendering context current: {error:?}");
}
let result = renderer.rendering_context.read_to_image(rect);
```

### Pattern C: post-split

```rust
if let Some(gl) = renderer.rendering_context.gl() {
    if let Err(error) = gl.make_current() {
        error!("Failed to make the rendering context current: {error:?}");
    }
}
let result = renderer.rendering_context.read_to_image(rect);
```

Wgpu path now skips the `make_current` no-op entirely rather than calling it and relying on the impl to return `Ok(())`.

---

## What gets deleted

Per the plan's reap-cycle discipline, Phase A's commit should delete:

1. **The old `RenderingContext` trait** (after all impls + consumers migrated).
2. **`RenderingBackendBinding` enum** and the `backend_binding()` trait method — replaced by the capability getters.
3. **`GlBinding` and `WgpuBinding` wrapper structs** — no longer needed; callers consume `&dyn GlCapability` / `&dyn WgpuCapability` directly.
4. **`WgpuRenderingContext::gleam_gl_api()` and `glow_gl_api()` `unreachable!()` stubs** — both methods disappear from the impl because they're no longer required by any trait it implements. The latent crash surfaces vanish.
5. **Any `is_wgpu` flags used to gate GL calls** — replaced by `ctx.gl().is_some()` checks at use sites.

---

## Open design decisions before implementation

1. **Module location.** New traits in `components/shared/paint/rendering_context.rs` alongside the old one during transition, or in a sibling `rendering_context_core.rs`? Recommend same file — the old trait deletes at the end of Phase A, and the capability traits read naturally as successors.

2. **`Option<&dyn _Capability>` vs `Option<Rc<dyn _Capability>>`.** Recommend `&dyn` for simplicity; it works for every impl currently in the tree. If a future impl needs the capability to outlive the context, the getter can be widened to return `Rc` without breaking callers.

3. **Error type on `GlCapability::make_current`.** Currently `Result<(), surfman::Error>` — should the GL capability trait expose `surfman::Error` directly, or wrap it in a neutral `GlError` type? Recommend passing through `surfman::Error`: every current consumer already handles it, and the abstraction tax isn't paid for by any current caller.

4. **`egui-wgpu` integration path.** The existing `egui_wgpu::Renderer::new` wants a `&wgpu::Device` and a `&wgpu::Queue`. Under the split: `let (device, queue) = ctx.wgpu().expect("egui path requires wgpu capability").map(|w| (w.device(), w.queue())).unzip()` — verbose. Consider a `WgpuCapability::device_and_queue(&self) -> (wgpu::Device, wgpu::Queue)` convenience returning both in one call.

5. **Toy embedder as validation artifact.** Next deliverable per the plan's updated Phase A DoD. A ~200-line embedder using only `RenderingContextCore` (no `GlCapability`, no `WgpuCapability`) should not compile — it needs at least one capability to be useful. A 200-line embedder using only `RenderingContextCore + WgpuCapability` *should* compile and drive a minimal webview. That asymmetry is the test of whether the trait design is actually wgpu-first.

---

## Acceptance bar (carries forward to implementation PR)

- [ ] New traits defined in `rendering_context.rs` alongside old trait
- [ ] All 4 existing `RenderingContext` impls grow `RenderingContextCore` + capability impls
- [ ] 20 call sites migrated (13 paint, 7 embedder)
- [ ] Old `RenderingContext` trait, `RenderingBackendBinding`, `GlBinding`, `WgpuBinding` deleted
- [ ] `WgpuRenderingContext::gleam_gl_api`/`glow_gl_api` `unreachable!` stubs deleted
- [ ] `graphshell`, `servoshell`, and a 200-line toy embedder all compile
- [ ] WPT compositor / hit-testing / canvas suites unchanged
- [ ] One commit per logical batch (trait definition; paint migration; embedder migration; reap)
