# Phase A — RenderingContext Consumer Audit

**Status (archived 2026-05-17):** Phase A landed (RenderingContext trait split shipped 2026-04). Audit complete; kept here as the record of which consumers required which capability — useful when revisiting the trait shape.

---

**Companion to** [`2026-04-18_servo_wgpuification_plan.md`](2026-04-18_servo_wgpuification_plan.md)
**Deliverable**: classification matrix per Phase A's "full audit of `connection()` consumers" directive, expanded to cover every GL-shaped method on the `RenderingContext` trait.

## Methodology

Grepped `make_current`, `gleam_gl_api`, `glow_gl_api`, `create_texture`, `destroy_texture`, `connection`, `prepare_for_rendering` across the `servo-wgpu` tree. Raw: 62 matches in 20 files.

Filtered out:

- **Trait internals** — self-references inside `rendering_context.rs` (the trait definition's default `backend_binding` + the Surfman-backed impls forwarding internally). 24 sites. Not real consumers; they vanish with the split.
- **False positives on method-name collisions**:
  - `wgpu::Device::create_texture` (vello backend, wgpu_engine internals): 6 sites
  - `glow::Context::create_texture` (WebGL thread, XR layer managers): 3 sites
  - `surfman::Device::connection` inside `accelerated_gl_media`: 2 sites

Remaining: **21 real consumer sites across 9 files.** Classified below.

## Classification Matrix

| # | Site | Method | Category | Phase A action | Risk |
|---|---|---|---|---|---|
| 1 | `paint.rs:239` | `connection()` | Paint compositor | Capability-check via `gl()` Option; keep as `if let Some(gl) = ctx.gl()` branch | **High** — plan flagged this specifically |
| 2 | `painter.rs:140` | `make_current` | Paint compositor | Gated by `is_wgpu` check already; move to `GlExt` path | Medium |
| 3 | `painter.rs:173` | `gleam_gl_api` | Paint compositor | Already in `if !is_wgpu` branch; move to `GlExt` | Low |
| 4 | `painter.rs:175` | `make_current` | Paint compositor | Same branch as #3; moves together | Low |
| 5 | `painter.rs:213` | `prepare_for_rendering` | Paint compositor | Neutral name but GL-shaped body; needs wgpu equivalent or no-op | Medium |
| 6 | `painter.rs:437` | `make_current` | Paint compositor | Verify capability-gated | Medium |
| 7 | `painter.rs:565` | `make_current` | Paint compositor | Verify capability-gated | Medium |
| 8 | `painter.rs:569` | `prepare_for_rendering` | Paint compositor | See #5 | Medium |
| 9 | `painter.rs:777` | `make_current` | Paint compositor | Verify capability-gated | Medium |
| 10 | `painter.rs:1467` | `make_current` | Paint compositor | Verify capability-gated | Medium |
| 11 | `screenshot.rs:200` | `make_current` | Paint compositor | GL-based readback; needs `read_to_image` on wgpu path — likely already works via the trait's existing `read_to_image` | Medium |
| 12 | `webrender_external_images.rs:45` | `create_texture` | Paint compositor | Moves to Phase C `ExternalImageImporter` abstraction | Low (planned) |
| 13 | `webrender_external_images.rs:60` | `destroy_texture` | Paint compositor | Same as #12 | Low (planned) |
| 14 | `servoshell/window.rs:125` | `make_current` | Embedder | servoshell's GL context init — stays but needs explicit `GlExt` capability handle | Low |
| 15 | `servoshell/desktop/headless_window.rs:76` | `make_current` | Embedder | Same | Low |
| 16 | `servoshell/desktop/headed_window.rs:183` | `make_current` | Embedder | Same | Low |
| 17 | `servoshell/desktop/headed_window.rs:1222` | `connection` | Embedder | Audit what it's checking; likely a GL-branch switch | Low |
| 18 | `servoshell/desktop/gui.rs:156, 171, 351, 626` | `make_current` (×4) | Embedder | egui-wgpu init path; needs capability-checked variant | Low |
| 19 | `servoshell/desktop/gui.rs:175` | `glow_gl_api` | Embedder | egui-glow / egui-wgpu bridge — stays on `GlExt` | Low |
| 20 | `servoshell/desktop/gui.rs:630` | `prepare_for_rendering` | Embedder | See #5 | Low |
| 21 | `components/servo/tests/common/mod.rs:43`, `examples/winit_minimal.rs:76` | `make_current` | Test/example | Update after trait split | Low |

## Observations

**The trait is already half-migrated.** Key call sites (`paint.rs:239`, `painter.rs:170`) already branch on capability (`if let Some(connection)`, `if is_wgpu { None } else { ... }`). The plumbing is capability-aware; what lags is the *trait shape* — `make_current`, `gleam_gl_api`, `glow_gl_api` are still required methods with no default, forcing every impl to pretend GL exists.

Phase A's split is therefore narrower than it first appears: it's primarily about **formalizing the capability boundary the callers already treat as real**.

**Paint-compositor real work clusters in 2 files.** `painter.rs` (8 sites) and `paint.rs` (1 high-risk site). `screenshot.rs` and `webrender_external_images.rs` are either already wgpu-friendly or planned for Phase C.

**`prepare_for_rendering` is an undertreated method.** It's in the trait as a no-op default, but the Surfman impls do real work (bind framebuffer). Three call sites (painter.rs ×2, servoshell/gui.rs ×1) depend on it being meaningful. For a wgpu-first context, what does "prepare for rendering" mean? Likely: acquire the next swapchain texture and set it as the current render target. Worth a spec decision in Phase A, not just a trait move.

**Embedder surface is concentrated in servoshell.** Eight sites across four servoshell files. If graphshell has an equivalent consumer set (it does; see `graphshell/shell/desktop/host/window.rs` and similar), the multi-embedder validation point from the plan's new "Success Demonstrability" section is load-bearing. Running the audit against graphshell in parallel would preempt surprise during Phase A landing.

## Recommended Phase A sequencing

Before any trait split:

1. **Spec decision on `prepare_for_rendering` semantics** for wgpu backends. Current Surfman binding is GL-specific; wgpu equivalent isn't obvious (swap-chain acquire? viewport bind? no-op?). Land this before moving the method.
2. **Audit `headed_window.rs:1222`** — what is `connection()` being checked for? If it's a GL/wgpu branch, the branch shape informs the trait design.
3. **Confirm `read_to_image` covers the `screenshot.rs:200` wgpu path.** If yes, #11 above is already solved. If no, Phase A needs to specify the wgpu readback contract.

Then the split itself:

4. **Capability-object trait design** (per the plan's new Phase A design alternative). Sketch:
   ```rust
   trait RenderingContextCore {
       fn size(&self) -> PhysicalSize<u32>;
       fn resize(&self, size: PhysicalSize<u32>);
       fn present(&self);
       fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage>;
       fn prepare_for_rendering(&self) {} // spec-dependent; see #1
       fn raw_window_handle(&self) -> Option<RawWindowHandle> { None }
       fn raw_display_handle(&self) -> Option<RawDisplayHandle> { None }
       fn wgpu(&self) -> &dyn WgpuCapability;              // required
       fn gl(&self) -> Option<&dyn GlCapability> { None }  // optional
   }

   trait GlCapability {
       fn make_current(&self) -> Result<(), Error>;
       fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl>;
       fn glow_gl_api(&self) -> Arc<glow::Context>;
       fn create_texture(&self, surface: Surface) -> Option<(SurfaceTexture, u32, UntypedSize2D<i32>)>;
       fn destroy_texture(&self, st: SurfaceTexture) -> Option<Surface>;
       fn connection(&self) -> Connection;
   }
   ```
   `connection()` becomes non-optional on `GlCapability` (if you have GL, you have a connection), which removes the `Option<Connection>` indirection.

5. **Migrate the 21 call sites** in two batches:
   - Batch 1: paint compositor (13 sites) — biggest single payoff.
   - Batch 2: embedder (8 sites) — can trail Batch 1 by a commit.

6. **Delete the deprecated `RenderingContext` trait** once all consumers migrated. (Per Phase A's "reap list" commitment.)

## Phase A definition of done — validated against this audit

- [ ] `prepare_for_rendering` wgpu semantics specified
- [ ] `headed_window.rs:1222` `connection()` usage understood and planned
- [ ] `read_to_image` verified as wgpu-capable for screenshot path
- [ ] `RenderingContextCore` + `GlCapability` (or equivalent split) lands
- [ ] All 21 call sites updated
- [ ] graphshell embedder compiles against split traits (Phase A DoD line from plan update)
- [ ] servoshell embedder compiles against split traits
- [ ] A 200-line toy embedder compiles against `RenderingContextCore` alone (no `GlCapability`), proving the wgpu-first path is viable
- [ ] Old `RenderingContext` trait definition deleted
- [ ] WPT compositor / hit-testing / canvas suites unchanged

## Notes for implementer

- The 6 `make_current` calls in `painter.rs` are the bulk of per-frame compositor GL overhead. If any of them survives on the wgpu path as a live call (not a no-op), something is wrong with the split.
- `webrender_external_images.rs` should wait for Phase C rather than be ported in Phase A — moving it now creates an interim abstraction that Phase C throws away.
- `accelerated_gl_media.rs` uses `surfman::Device::connection()` (not the trait). Unrelated to Phase A but worth noting — it's part of the macOS hardware video decode path and survives the trait split unchanged.

## Addendum — Pre-split spec questions resolved

### Q1. `prepare_for_rendering` wgpu semantics

**Decision**: `prepare_for_rendering` is GL-only. Belongs on `GlCapability`, not `RenderingContextCore`.

**Evidence**:

- Surfman impls bind the GL framebuffer (`rendering_context.rs:342-348`). This is fundamentally a GL operation — "make sure the framebuffer GL draws into is the one I own."
- `WgpuRenderingContext::prepare_for_rendering` is already `// No-op: wgpu has no implicit context to bind.` (`wgpu_rendering_context.rs:173`). The wgpu impl has been faking the method for contract reasons, not because it needs semantic meaning.
- Three real callers: `painter.rs:213` (unconditional), `painter.rs:569` (gated on `webrender_gl.is_some()`), `servoshell/gui.rs:630` (gated on `!SERVO_WGPU_BACKEND`). Two of three are already GL-gated. The third (`painter.rs:213`) becomes `if let Some(gl) = ctx.gl() { gl.prepare_for_rendering(); }` under the split.

wgpu backends explicitly select their render target at the point of encoding via `TextureView`. There's no "prepare" phase to emulate.

### Q2. `headed_window.rs:1222` `connection()` usage

**Decision**: False positive. Not a `RenderingContext` consumer.

**Evidence**: The call site is `device.connection()` where `device: &mut surfman::Device` — it's calling `surfman::Device::connection()`, not the trait method. Lives inside `impl servo::webxr::GlWindow for XRWindow`; XR subsystem code using Surfman directly.

**Consequence**: Real consumer count drops from 21 to 20. Zero embedder `connection()` calls against the trait. Phase A's split does not need to consider embedder consumption of `connection()` at all.

### Q3. `screenshot.rs:200` wgpu readback coverage

**Decision**: Split-and-accept. `read_to_image` stays on `RenderingContextCore` (it is not GL-shaped by contract); `make_current` moves to `GlCapability`; wgpu-side readback implementation is a separate follow-on and does **not** block Phase A.

**Evidence**:

- `screenshot.rs:200` calls `make_current()` immediately before `read_to_image(rect)`. The `make_current` is GL prep; the `read_to_image` is the actual readback. Under the split, the call becomes:

  ```rust
  if let Some(gl) = renderer.rendering_context.gl() {
      gl.make_current();
  }
  let result = renderer.rendering_context.read_to_image(rect);
  ```

- `WgpuRenderingContext::read_to_image` is currently `// TODO: Implement GPU→CPU readback via staging buffer` returning `None` (`wgpu_rendering_context.rs:190-193`). Screenshots on wgpu-backed contexts already silently fail. This is a pre-existing gap, not something Phase A introduces.
- `read_to_image` is conceptually backend-neutral: "read a rect of the presented frame into an RgbaImage." The wgpu path needs a real implementation eventually (wgpu staging buffer + map-read), but the trait contract doesn't change.

**Follow-on ticket suggestion**: `Implement wgpu::Texture → RgbaImage readback in WgpuRenderingContext::read_to_image`. Not Phase A scope; track separately.

## Additional finding — `unreachable!()` panic surfaces

The wgpu RenderingContext impl currently has:

```rust
fn gleam_gl_api(&self) -> Rc<dyn gleam::gl::Gl> {
    unreachable!("gleam_gl_api() called on WgpuRenderingContext")
}

fn glow_gl_api(&self) -> Arc<glow::Context> {
    unreachable!("glow_gl_api() called on WgpuRenderingContext")
}
```

These are implemented purely to satisfy the trait's required-method contract. Any caller that forgets to capability-check before calling them crashes the process with `unreachable!`. Four call sites rely on upstream capability checks to avoid this (painter.rs:173, plus a servoshell egui-bridge site, plus two inside `OffscreenRenderingContext::glow_gl_api/gleam_gl_api` forwarders).

The capability-object split eliminates this panic surface entirely: `gl()` returns `Option<&dyn GlCapability>`, and the only way to reach `gleam_gl_api()` is via a `Some(...)` match, which is type-checked. Small correctness improvement but a real one — it's currently a *latent* crash that only doesn't fire because every caller remembers to check `is_wgpu` first. The trait makes forgetting possible; the capability-object version makes forgetting impossible.

Worth mentioning in Phase A commit messages as a concrete demonstration of "make illegal states unrepresentable," which generalizes the theme Phase B is named after.

## Revised Phase A sequencing (post-addendum)

1. ~~Spec decision on `prepare_for_rendering`~~ → done above: move to `GlCapability`.
2. ~~Audit `headed_window.rs:1222`~~ → done above: false positive, ignore.
3. ~~Confirm `read_to_image` covers screenshot.rs wgpu path~~ → done above: stays on core, wgpu impl is a known separate gap.
4. **Capability-object trait design** — carries forward unchanged.
5. **Migrate 20 call sites** (not 21) in two batches:
   - Batch 1: paint compositor (13 sites)
   - Batch 2: embedder (7 sites after Q2's subtraction)
6. **Delete deprecated `RenderingContext` trait** once migrated.
7. **Bonus**: the `unreachable!()` panic surface disappears as a byproduct. Worth calling out in the commit.
