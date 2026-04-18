# Servo Wgpuification Plan

## Purpose

This branch is no longer evaluating whether Servo can use a wgpu compositor. That part is already real. The next phase is to finish the architectural transition away from GL-era render-path assumptions so that Servo is consistently wgpu-first in the compositor and host-integration path, while GL remains only where product features still require it.

This plan is written for the `webrender-wgpu-patch` / `wgpu-backend-0.68-experimental` stack and should be read as a direct-replacement migration plan, not as a backwards-compatibility preservation plan.

## Framing

The current branch is split-brain:

- The compositor and host integration already have a viable wgpu-first path.
- WebGL, WebXR, and several trait surfaces are still shaped around GL/Surfman assumptions.
- A substantial amount of complexity now comes from supporting both models at the same architectural layer.

The goal of this plan is to finish picking a side.

## Posture

This is a research fork. Under current Servo policy it is not mergeable upstream, so success here is not defined by immediate landability. Success is defined by demonstrable benefit: a branch that stays technically coherent, survives skeptical scrutiny, and produces evidence strong enough to justify a later policy conversation if the results warrant it.

## Working Thesis

wgpu's shared-device model enables a different architecture than Servo's inherited GL model.

- GL-shaped architecture: isolated contexts, `make_current`, Surfman swap chains, front-buffer extraction, texture wrapping, pixel or surface handoff between threads.
- wgpu-shaped architecture: shared device ownership, explicit frame/resource leases, direct texture handle movement, backend-native external resource import, fewer implicit thread-local rendering assumptions.

This plan prefers the second model whenever there is a meaningful architectural choice.

## Current State Summary

### Landed or partially landed on this branch

- WebRender can run with a shared wgpu device/queue.
- `WgpuRenderingContext` provides a pure-wgpu presentation path.
- Servo paint now owns WebGL external-image lifecycle policy on the Servo side.
- The wgpu WebGL external-image import transaction has been moved into `servo-wgpu-interop-adapter` as `SurfmanSurfaceImporter`.
- Canvas2D runtime policy is GPU-first when `vello` is compiled.
- `servoshell` now enables `vello` by default on this branch.

### Still structurally GL-shaped

- `RenderingContext` is GL-first and only optionally exposes wgpu.
- WebGL context production is still GL/Surfman-based.
- WebXR is still built around Surfman GL layer management.
- External image handling is cleaner, but still split across GL-native and wgpu-native mental models.
- Canvas2D still behaves like an image-update subsystem more than a shared GPU-resource producer.

## Non-Goals

- Do not preserve the current trait shape purely for compatibility if it obstructs the wgpu-first design.
- Do not treat `Cargo.lock` as design authority on this experimental branch.
- Do not push Servo-specific lifecycle policy into `webrender-wgpu`.
- Do not block compositor-side cleanup on a full WebGL replacement.

## Design Rules

### Context shape decision

Phase A should not be a mechanical trait split. It should explicitly choose between trait inheritance and capability objects before implementation.

- `RenderingContextCore` must always expose required wgpu-facing capabilities.
- GL should be represented as an explicit opt-in surface that consumers must prove they need.
- Shared-texture import/export should be a first-class capability, not a side effect of GL-only APIs.

### Layer ownership

The policy boundary is:

- Servo paint owns publication, lease, and release policy types.
- Bridge crates own platform import/export transactions.
- `webrender-wgpu` consumes generic imported resources and must not depend on Servo policy types.

This keeps Servo-specific lifecycle policy out of `webrender-wgpu` without forcing platform transaction code back into Servo paint.

### Synchronization model

The plan must make synchronization explicit.

- Preferred steady-state model: one shared wgpu device for producer and compositor.
- If producer and compositor cannot share a device, publication must carry explicit synchronization metadata.
- No phase is done if it relies on implicit GL-style serialization assumptions.

### Reap rule

Every phase must ship with a reap list: symbols, branches, helper types, and fallback paths that the phase intends to delete before its definition of done is satisfied.

The next phase starts by checking that the previous phase's reap list is empty.

## Migration Axes

This plan is organized around six improvement areas.

### 1. Make `RenderingContext` wgpu-first

This is the foundation. All other areas currently fight the GL-shaped default until this flips.

#### Problem

The trait still assumes GL primitives as core behavior:

- `make_current`
- `gleam_gl_api`
- `glow_gl_api`
- `create_texture`
- `destroy_texture`
- `connection`

wgpu hooks are optional add-ons rather than the primary backend contract.

#### Target shape

Decide between these two shapes before implementation:

```rust
// trait-inheritance option
trait RenderingContextCore { ... }
trait GlRenderingContextExt: RenderingContextCore { ... }

// capability-object option
trait RenderingContextCore {
    fn wgpu(&self) -> &dyn WgpuCapability;
    fn gl(&self) -> Option<&dyn GlCapability>;
    fn shared_texture(&self) -> Option<&dyn SharedTextureCapability>;
}
```

Trade-off:

- capability objects compose better when future backends such as Metal-native or browser-WebGPU land
- trait inheritance is more familiar and may be simpler for short-term implementation

Decide before implementation. Do not let the branch drift into an implicit hybrid.

#### Mandatory pre-work

Audit every consumer of `connection()` before the trait split lands, and record the result as a classification matrix rather than a prose list.

Minimum matrix columns:

- consumer
- category: compositor / WebGL / WebXR / paint / other
- action: delete / move to `GlCapability` / replace with wgpu primitive

Known high-risk area:

- `Paint::register_rendering_context()` opportunistically creates Surfman details when `connection()` exists.

That call site must be either:

- moved behind a GL-only capability boundary, or
- replaced with a wgpu-native equivalent for the compositor path.

Phase A must also run a WebXR design spike. It does not need a working XR implementation, but it does need to answer whether the core acquire/present API can express XR's predicted-display-time and per-eye submission lifecycle. If it cannot, the core trait is too narrow and must change before Phase A is considered done.

#### Acceptance bar

- A checked-in classification matrix exists for `connection()` consumers and is being burned down, not merely surveyed.
- Main compositor initialization does not require GL-facing `RenderingContext` methods.
- The normal window-rendering path compiles and runs with no dependency on GL-only trait methods.
- GL-specific functionality is isolated behind explicit optional interfaces chosen in Phase A.
- Graphshell's compositor adapter builds against the split interfaces without touching GL-only methods.
- Graphshell, servoshell, and a toy embedder all compile against the split traits without GL-only method calls.
- The Phase A reap list removes GL-only helper use from the compositor main path rather than merely deprecating it.

### 2. Replace manual busy counters with leases

This is the make-illegal-states-unrepresentable slice.

#### Problem

The current model relies on:

- busy counters keyed by `WebGLContextId`
- explicit increment/decrement discipline
- explicit abort paths
- explicit `finished_rendering_to_context` notifications
- deferred deletion flags on WebGL contexts

That is better than ad hoc duplication, but still easy to misuse.

#### Target shape

Introduce a publication/lease contract rather than a bare lease object.

Suggested shape:

- `FramePublisher`
- `PublishedFrame`
- `FrameToken`
- `FrameReceipt`

Where:

- producer publishes a `FrameToken` proving a frame is ready
- compositor converts that into a `PublishedFrame`
- compositor returns a `FrameReceipt` or equivalent release signal when sampling/present completion makes reuse legal

This gives the model a place to carry explicit synchronization data such as producer and consumer submission indices.

#### Important design constraint

The publication contract must survive from:

- producer lock
- through compositor/external-image use
- until frame completion / unlock / present boundary

That means the active lease/token state belongs in the compositor-side pending-frame structure, not in the producer thread.

Minimum state model:

- `Acquired`
- `Committed`
- `PendingPresent`
- `Presented`
- `Released`

The plan should treat that state diagram as a deliverable, not an implementation detail.

#### Two-sided lease contract

The producer should emit a `FrameToken` carrying its `SubmissionIndex`, the compositor should exchange that for a `FrameReceipt`, and the producer should wait on the receipt before reuse. This maps cleanly onto wgpu fence semantics and makes correctness under contention tractable instead of heuristic.

#### Acceptance bar

- No manual busy-counter choreography in normal lock/unlock flow.
- Context deletion waiting is expressed in terms of outstanding leases.
- Failure paths collapse to dropping an uncommitted lease rather than hand-maintained unwind code.
- Lease/publication transitions are observable through diagnostics or trace events.

### 3. Unify external-image import around the lease abstraction

This should be treated as a serious goal, not optional polish, but it depends on `1` and `2`.

#### Why `3` depends on `1` and `2`

- Without `1`, paint still sees a GL-shaped world first and a wgpu-shaped world second.
- Without `2`, import still returns backend-specific resource fragments that require manual lifecycle bookkeeping.

Done early, `3` risks becoming another enum-with-branches façade.
Done after `1` and `2`, it can become the real paint-level abstraction.

#### Target shape

Treat external images as one instance of a broader GPU frame publication model.

Paint should own backend-neutral policy concepts such as:

- `FramePublisher`
- `PublishedFrame`
- `ExternalImageLease`

Those objects should represent:

- a foreign GPU resource that can be sampled by the compositor
- the synchronization data required to reuse or release it correctly
- the release policy required when compositing is finished

Backend-specific implementations live below that layer:

- GL texture wrapping path
- Surfman-to-wgpu import path
- future native D3D11/D3D12/Metal resource import path

`webrender-wgpu` should consume imported resources and descriptors, not Servo-owned policy types. Bridge crates should implement import/export adapters below Servo paint's policy boundary.

#### Acceptance bar

- Paint policy code does not branch on GL vs wgpu for the common external-image lifecycle.
- Backend differences are contained to importer implementations.
- The same lease model underlies both the GL and wgpu paths.
- The policy trait boundary lives on the Servo side of the layer split, with no `webrender-wgpu` dependency on Servo types.

### 4. Attack the producer side, not just the compositor side

This is the biggest strategic payoff.

#### Problem

The compositor is already much less GL-bound. The producer side is not.

WebGL still fundamentally produces GL/Surfman-managed surfaces. That means the compositor must keep translating out of a GL-native world even if it no longer renders with GL.

#### Windows / DX12 objective

On Windows, the serious target is:

- WebGL producer output becomes a shareable D3D11/D3D12 resource
- compositor import becomes a backend-native resource import into wgpu
- GL framebuffer import becomes a compatibility fallback, not the mainline path

#### Hidden prerequisite

This phase starts with an explicit ANGLE capability audit.

Before Servo-side implementation work is treated as the blocker, confirm whether the target ANGLE line actually provides usable `EGL_ANGLE_device_d3d11` and shared-handle export for the path this branch wants. If it does not, Phase 4 is gated on ANGLE movement rather than Servo code cleanup.

#### ANGLE / mozangle implication

There is a path to making `mozangle` unnecessary for the compositor path on DX12.
There is not yet a path to making it unnecessary for all Servo features without replacing or radically reworking the WebGL producer implementation.

The staged version is:

1. compositor and host presentation become pure-wgpu/DX12
2. `mozangle` is isolated to WebGL and XR producer roles
3. WebGL producer evolves to export backend-native shareable resources
4. only then can a real branch decision be made about replacing `mozangle` entirely

#### Acceptance bar

- The preferred Windows WebGL producer-to-compositor path no longer requires GL framebuffer import.
- Backend-native shared resource import is the mainline path where supported.
- GL-based import is a fallback path with explicit downgrade semantics.

### 5. Rework WebXR away from Surfman GL

Implementation is a parallel track, but the design pressure is not. Phase A should validate its core trait against WebXR's expected lifecycle before the trait split is considered stable.

#### Problem

XR layer management is still explicitly Surfman GL-based.
That was historically reasonable when WebGL-in-XR was the dominant assumption.
It is much less aligned with a wgpu-first runtime.

#### Target shape

Introduce a wgpu-native XR layer manager path that matches what runtimes actually want:

- D3D11 or D3D12 shared resources on Windows/OpenXR
- Metal textures on Apple platforms
- backend-native layer submission where possible

#### Acceptance bar

- XR runtime integration does not require Surfman GL for the mainline path.
- WebXR compositor submission can consume backend-native resources.
- Surfman GL XR becomes fallback or compatibility mode.
- Phase A's core trait shape has already been validated against XR lifecycle needs before this implementation phase begins.

### 6. Make Canvas2D a first-class GPU resource producer

GPU rasterization alone is not enough. The delivery model also needs to change.

#### Problem

Canvas2D is now GPU-first at runtime when Vello is available, but it still behaves architecturally like an image-update subsystem with its own paint thread and image publication path.

#### Target shape

Canvas should expose compositor-consumable GPU residency directly:

- render on a device compatible with the compositor
- publish a texture/resource handle rather than an image-update abstraction wherever possible
- participate in the same external-image / lease model as other foreign GPU content

#### Acceptance bar

- Canvas composition can avoid round-tripping through image-style publication for the mainline GPU path.
- Canvas outputs are shareable on the compositor's device boundary.
- Canvas lifetime follows the same lease/import conventions as other external GPU resources.
- Canvas GPU handoff is diagnosable via producer-to-compositor tracing keyed by submission index or equivalent publication metadata.

## Dependency Order

The recommended order is:

1. `RenderingContext` wgpu-first split
2. lease/token model
3. unified external-image importer/lease abstraction
4. producer-side backend-native resource export
5. WebXR wgpu-native path (parallel track)
6. Canvas2D direct GPU residency

### Why this order

- `1` is upstream of almost everything.
- `2` and `3` should land together as one implementation slice even if they remain separate architecture headings.
- `4` is the biggest strategic win, but easiest to do cleanly once the compositor-side abstraction has been fixed.
- `5` is a large but parallel modernization track.
- `6` becomes much easier once device sharing is normal and external-image leases are already real.

## Proposed Phases

### Phase A. RenderingContext redesign, classification, and downstream compile gate

Deliverables:

- checked-in `connection()` classification matrix with consumer, category, and action columns
- checked-in design decision for trait inheritance versus capability objects, validated against compositor, WebGL, and a WebXR design spike
- move paint/painter setup to depend on wgpu-first core interfaces and optional backend surfaces
- Graphshell compositor adapter updated to compile against the split interfaces without calling GL-only methods
- Phase A reap list naming compositor-path uses of `connection`, `create_texture`, `destroy_texture`, and ad hoc Surfman detail creation for deletion or removal from the main path

Definition of done:

- compositor and ordinary window render path compile without depending on GL-only methods
- Graphshell, servoshell, and a toy embedder build against the split interfaces without touching GL-only methods
- the classification matrix is being burned down by concrete delete/move/replace actions
- the core trait shape is validated against XR lifecycle requirements before stabilization
- the Phase A reap list removes old compositor-path GL dependencies from the main path

### Phase B. External-image lease and publication model

Deliverables:

- checked-in lease state diagram covering `Acquired -> Committed -> PendingPresent -> Presented -> Released`
- `FramePublisher` / `PublishedFrame` / `FrameToken` / `FrameReceipt` model or an equivalent contract with the same ownership semantics
- replace manual busy-map choreography with the publication contract
- move pending frame ownership to compositor-side frame state
- define the Servo-side policy boundary and fit current bridge importers underneath it
- add diagnostics that correlate producer publication to compositor sampling/present completion
- Phase B reap list naming `WebGLContextBusyMap`, manual abort/unwind release paths, and superseded split lifecycle helpers for deletion

Definition of done:

- no manual lock/unlock counter dance in steady-state paths
- paint policy code uses one publication/lease contract for GL and wgpu-backed external images
- `webrender-wgpu` does not depend on Servo policy types
- producer-to-compositor publication is diagnosable in traces
- relevant WPT suites pass at baseline or better
- the Phase B reap list is empty, including deletion of the old busy-map mainline path

### Phase C. ANGLE capability audit and Windows backend-native producer path

Deliverables:

- Phase C.0 ANGLE capability audit covering the target line's D3D11 device/export support and shared-handle viability
- explicit go/no-go decision on whether ANGLE upgrade is a prerequisite for the desired Windows path
- prototype producer-side D3D11/D3D12 shareable resource export
- add backend-native Windows import path in the bridge where feasible
- make the synchronization model explicit: single shared device where possible, explicit producer/consumer submission handoff where not
- Phase C reap list naming GL framebuffer import as the default Windows path and any silent Windows fallback branches that preserve it as the mainline

Definition of done:

- the target ANGLE line is proven capable or formally identified as a blocker
- the preferred Windows producer-to-compositor path no longer depends on GL framebuffer import
- cross-device synchronization is either implemented explicitly or rejected by branch policy in favor of one-device-only publication
- relevant WPT suites pass at baseline or better
- the Phase C reap list demotes GL framebuffer import to fallback rather than leaving it as the silent default

### Phase D. WebXR backend modernization

Deliverables:

- isolate current Surfman GL XR assumptions
- include a design-sketch validation from Phase A showing that the core acquire/present API can express XR frame lifecycle requirements such as predicted-display-time and per-eye submission
- design a wgpu-native XR layer manager path against the stabilized core trait
- prototype Windows OpenXR resource submission aligned with the branch's publication and synchronization model
- Phase D reap list naming Surfman GL XR mainline-path assumptions and compatibility branches targeted for removal where replacement exists

Definition of done:

- XR has a credible wgpu-native path and no longer structurally blocks broader de-GL work
- the core trait has already proven capable of expressing XR lifecycle needs
- relevant WPT suites pass at baseline or better
- the Phase D reap list removes XR-only GL assumptions from the mainline path where replacement exists

### Phase E. Canvas residency unification and downstream smoke coverage

Deliverables:

- make Canvas GPU outputs publishable as compositor-native resources
- route Canvas through the same publication/lease abstraction used by foreign GPU resources
- add canvas handoff diagnostics keyed by producer/compositor submission indices or equivalent publication metadata
- add a headless Graphshell smoke test that exercises WebGL + Canvas2D + text and compares a screenshot against a golden
- Phase E reap list naming image-update-style Canvas mainline publication paths superseded by direct GPU publication

Definition of done:

- Canvas GPU composition no longer looks like an image-update subsystem in the mainline path
- Canvas publication is diagnosable in traces
- the Graphshell smoke test passes as a gate on the trait split and publication changes
- relevant WPT suites pass at baseline or better
- the Phase E reap list is empty

## DX12 / mozangle Decision Tree

### Can Servo obviate `mozangle` for DX12?

#### Yes, for the compositor and host presentation path

This branch already has the ingredients for a pure-wgpu compositor/presentation path.

#### Not yet, for full product feature coverage

As long as:

- WebGL remains GL-native in Servo
- WebXR remains Surfman GL-based

`mozangle` still has product-surface value even if the compositor itself no longer needs it.

ANGLE solves real portability and correctness problems today. This branch is not replacing a fake dependency with a clean one; it is proposing to replace one correctness mechanism with another, and both sides of that exchange are serious engineering. The standard here is not ideological purity. It is whether the new path preserves or improves correctness while simplifying the architecture enough to justify the change.

### Practical staged objective

The correct branch target is:

- make `mozangle` unnecessary for composition and presentation on DX12
- isolate it behind WebGL/XR producer feature boundaries
- then seriously evaluate replacing it for producer functionality

That is a meaningful architectural win even before full elimination.

## Embedder Compatibility

This branch should validate the design across multiple embedders, not just the one that motivated the fork.

Phase A should explicitly gate on Graphshell, servoshell, and a toy embedder compiling against the context split, and Phase E should gate on an end-to-end smoke test.

Minimum audit targets:

- `RenderingContext::connection()`
- GL-specific rendering-context assumptions
- compositor adapter code that still expects Surfman details to exist as a side effect of context registration

Graphshell-specific checks remain important:

- audit Graphshell consumers of `RenderingContext::connection()`
- audit Graphshell GL-specific rendering-context assumptions
- audit compositor adapter code that still expects Surfman details to exist as a side effect of context registration

This should be tracked as a branch acceptance criterion rather than discovered opportunistically during integration.

## Success Demonstrability

- WPT continuity: each phase should name the relevant WPT suites and attach before/after numbers so the branch shows continuity, not just anecdotal confidence.
- Multi-embedder validation: trait and publication changes should work for servoshell, a toy embedder, and graphshell rather than being tuned to a single consumer.
- Measurable wins: each phase should name the metric it expects to move, such as code size, allocation count, frame latency, or an eliminated bug class.
- Attribution: the branch README should carry one short disclosure paragraph describing the branch's AI-assisted development history honestly, neither hidden nor flaunted.

## Success Criteria for the Branch

The branch should be considered to have reached the next maturity tier when:

- the compositor and normal host-rendering path are fully wgpu-first
- GL-only producer functionality is isolated behind explicit capability boundaries
- external image ownership is lease-based rather than counter-based
- Windows WebGL composition prefers backend-native shared resources over GL framebuffer import
- WebXR has a credible non-GL path
- Canvas2D can participate as a first-class GPU resource producer
- previous-phase reap lists are empty instead of accumulating transitional compatibility layers

At that point, Servo is no longer a GL-native engine with a wgpu compositor attached. It is a wgpu-first engine with compatibility producers where needed.