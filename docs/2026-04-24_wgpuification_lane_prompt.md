# Servo-wgpu Wgpuification Lane Prompt

**Purpose.** Brief a fresh agent on the servo-wgpu wgpuification effort so they can pick up any one lane as a self-contained slice. Dispatcher assigns the lane; this document supplies the rest.

**How to use.** When dispatching, tell the agent: *"Read `docs/2026-04-24_wgpuification_lane_prompt.md`. You are assigned lane **&lt;S*#*&gt;**. Follow the lane's scope and acceptance bar; do not drift into adjacent lanes."*

---

## 1. Context

- Repo: `C:\Users\mark_\Code\repos\servo-wgpu`, branch `webrender-wgpu-patch`.
- This is a research fork. Not upstream-mergeable under current Servo policy. Success is measured by demonstrable architectural benefit, not immediate landability.
- Companion repo: `C:\Users\mark_\Code\repos\webrender-wgpu` on `spirv-shader-pipeline`, consumed here via Cargo path override. Do not push Servo-side lifecycle policy into `webrender-wgpu`.
- Canonical plan: [`2026-04-18_servo_wgpuification_plan.md`](2026-04-18_servo_wgpuification_plan.md). Read its **Working Thesis**, **Design Rules**, and the phase section matching your lane. Do not re-derive decisions the plan has already made.
- Phase A audit & trait design: [`2026-04-18_phase_a_rendering_context_audit.md`](2026-04-18_phase_a_rendering_context_audit.md), [`2026-04-18_phase_a_trait_design.md`](2026-04-18_phase_a_trait_design.md), [`2026-04-18_phase_a_toy_embedder.md`](2026-04-18_phase_a_toy_embedder.md).

## 2. Current State (2026-04-24)

**Phase A — RenderingContext wgpu-first split: DONE.** Verify with `git log --grep='Phase A' --oneline`. Landed:

| Slice | Description | Commit |
|---|---|---|
| A.1+A.2 | wgpu-first trait split, coexisting with legacy | `21998f16849` |
| A.3 | `RenderingContext: RenderingContextCore` subtrait | `e18bd412808` |
| A.4 | paint compositor consumers → capability accessors | `a0fffbc8173` |
| A.5 | servoshell embedder consumers → capability accessors | `4aa0f2c1adf` |
| A.6 | legacy `RenderingContext` trait reaped | `5cd3634a338` |
| A.7 | toy wgpu embedder as trait-split validation | `2f87c3d80f9` |

Plus earlier: `WgpuRenderingContext` with zero-copy `render_to_view` (`5c211127e1e`), composite_texture blit pass (`a2f4157d3d4`), embedder-provided shared device (`7e1d9c65ee3`), pipeline cache + `WgpuHal` wiring (`bf2a45754b6`), wgpu 26→29 bump (`e7812be984d`), Canvas vello/wgpu import stabilization (`3b8a3216f04`), `SurfmanSurfaceImporter` bridge (`65de84440cb`), ANGLE-default removal on Windows (`3b2bc79fdc3`).

**Phases B–E: open.** The lanes below decompose Phase B–E plus the plan's Near-Term Execution Map into independently-landable slices.

## 3. Working Rules (non-negotiable)

From the canonical plan:

- **Reap rule.** Every lane ships a reap list — symbols, branches, helper types, fallback paths scheduled for deletion before the lane is "done." The next lane starts by checking the previous reap list is empty. "Not tracked" is not an acceptable state.
- **Layer ownership.** Servo paint owns publication/lease/release *policy* types. Bridge crates own platform import/export *transactions*. `webrender-wgpu` consumes generic imported resources and must not depend on Servo policy types.
- **Synchronization is explicit.** Preferred steady state: one shared wgpu device for producer + compositor. If not shareable, the publication must carry explicit sync metadata.
- **Multi-embedder compile gate.** If a lane touches any public surface in `components/shared/paint/`, it must compile against `servoshell`, the toy embedder, and (at minimum) be verified not to break graphshell's adapter surface.
- **No compatibility cruft.** Research fork. Do not preserve trait shape for compatibility if it obstructs wgpu-first design. Coexistence artifacts that survive a lane must be named in that lane's reap list with a pointer to the lane that will delete them.

From the user:

- When asked to commit, commit the whole working tree unless told otherwise.
- Don't edit `Cargo.lock` as design authority on this branch.
- No backwards-compat hacks (rename-to-`_var`, re-exports for deleted code, stub methods with `// removed` comments). Delete completely.
- Prefer runtime verification over extended static code tracing when debugging. Surface blockers early rather than continuing static analysis.
- Don't skip hooks or signing. Don't `git push`. Don't `git merge servo/main`.

## 4. Lane Catalog

Each lane is self-contained. Scope creep into adjacent lanes is not helpful — it creates conflicts with parallel work.

### S1 · Render-contract unification

**Problem.** API drift: `paint()`, `render()`, explicit `present()`. Examples teach contradictory frame-ownership rules. The wgpu path already presents inside the compositor render flow.

**Scope.**
- Pick one canonical public verb for "produce the next frame."
- Make servoshell, the toy embedder, and `examples/*` conform.
- Delete the redundant wording and API surface. Update `servo_wgpu_integration.md`-equivalent embedder docs.

**Entry points.** `components/paint/painter.rs`, `components/shared/paint/rendering_context_core.rs`, `components/shared/paint/wgpu_rendering_context.rs`, `ports/servoshell/desktop/gui.rs`, `examples/*`.

**Acceptance.** One verb, one call pattern. `git grep` across `examples/` and `ports/servoshell/` for old verbs returns zero. Multi-embedder compile gate holds. Reap list empty.

**Dependencies.** None. Parallelizable with S2, S5a, S6, S7.

---

### S2 · Pure-wgpu readback

**Problem.** `read_to_image()` on `WgpuRenderingContext` is unfinished. Screenshot and golden-image tests are biased toward GL.

**Scope.**
- Implement staging-buffer readback on the pure-wgpu path.
- Make screenshot flows backend-neutral at the `RenderingContextCore` layer.
- Document unavoidable differences (sRGB view policy, pre-mult alpha).

**Entry points.** `components/shared/paint/wgpu_rendering_context.rs`, `components/shared/paint/rendering_context_core.rs`, `components/paint/screenshot.rs` (flagged at `:200` in the Phase A audit).

**Acceptance.** Headless toy-embedder produces a PNG that matches GL output within documented tolerance at DPR=1 and DPR=2. No GL-only code paths in `screenshot.rs` for the wgpu case. Reap list empty.

**Dependencies.** None. Parallelizable with S1, S5a, S6, S7.

---

### S3 · Lease / publication contract

This lane has two modes — the dispatcher specifies which.

#### S3-spike — design doc only

**Deliverable.** `docs/2026-MM-DD_lease_publication_contract_design.md` covering:

- Full type signatures for `FramePublisher`, `PublishedFrame`, `FrameToken`, `FrameReceipt`.
- Explicit state machine: `Acquired → Committed → PendingPresent → Presented → Released`. Every transition trigger. Every failure edge.
- How producer `SubmissionIndex` flows through token → receipt → producer wait. Maps onto wgpu fence semantics.
- Where the active lease lives (compositor-side pending-frame structure, not producer thread).
- Relationship to the code it replaces (`WebGLContextBusyMap`, `finished_rendering_to_context` notifications, deferred-deletion flags) and the bridges it composes with (`SurfmanSurfaceImporter`).
- Diagnostics surface: `tracing` event names, fields, spans.
- Servo-side policy boundary declaration — which types live in paint, which in bridge crates, which stay out of `webrender-wgpu`.

**Out of scope for the spike.** Do not migrate any call site. Do not delete `WebGLContextBusyMap`. Do not touch `webrender-wgpu`.

**Acceptance.** Doc with ASCII state diagram, full type signatures, worked-example end-to-end trace. User signs off before S3-execution is dispatched.

#### S3-execution — implementation

**Prerequisite.** S3-spike doc signed off (lane assignment cites the commit).

**Scope.**
- Introduce the types from the spike.
- Replace `WebGLContextBusyMap` mainline path.
- Move pending-frame ownership to compositor-side structure.
- Express context-deletion waiting as outstanding-lease accounting.
- Wire the diagnostics emitted in the spike.

**Acceptance.**
- No manual busy counter increment/decrement in normal lock/unlock flow.
- Failure paths drop uncommitted lease; no hand-maintained unwind code.
- `webrender-wgpu` has no dependency on Servo policy types.
- Relevant WPT suites at baseline or better (record before/after numbers in the commit body).
- Reap list empty: `WebGLContextBusyMap`, manual abort paths, split lifecycle helpers all deleted.

**Dependencies.** S3-execution blocked on S3-spike sign-off. S4 blocked on S3-execution.

---

### S4 · Canvas2D GPU-native publication

**Blocked by.** S3-execution landed.

**Scope.**
- Route Canvas GPU output through the publication/lease contract from S3.
- Stop treating Vello-backed canvas as byte-oriented publication in the mainline GPU path.
- Make the canvas texture/resource handle shareable on the compositor device boundary.

**Entry points.** `components/canvas/*`, `docs/components/canvas.md`, Canvas2D paint-thread publication path.

**Acceptance.**
- Canvas composition avoids round-tripping through image-style publication on the GPU path.
- Canvas lifetime follows the same lease model as external images.
- Canvas handoff traceable by submission index.
- Relevant WPT suites at baseline or better.
- Reap list empty: image-update-style canvas mainline publication paths deleted where superseded.

---

### S5a · ANGLE capability audit (research)

**Scope.** Produce `docs/2026-MM-DD_angle_capability_audit.md` answering:

- Does the target ANGLE line expose `EGL_ANGLE_device_d3d11`?
- Can we export shared handles usable by wgpu's D3D12 backend import?
- What's the current version the branch pulls? What's the upgrade path if needed?
- Concrete **go / no-go** recommendation for S5b.

**Method.** Inspect the local ANGLE build (via `mozangle`). Query extensions from a small test binary. Cross-reference Chromium's equivalent capabilities as a sanity check. The plan calls this the hidden prerequisite for Phase C — treat it as one.

**Out of scope.** The producer-side prototype (that's S5b).

**Acceptance.** Audit doc with extension list, shared-handle probe result, version notes, explicit go/no-go verdict the user can act on.

**Dependencies.** None. Independent research lane.

---

### S5b · Windows D3D producer prototype

**Blocked by.** S5a returning "go" with user sign-off.

**Scope.** Per plan §4.
- WebGL producer outputs a shareable D3D11/D3D12 resource.
- Compositor import becomes backend-native wgpu resource import.
- GL framebuffer import demoted to fallback path with explicit downgrade semantics.

**Acceptance.**
- Preferred Windows path no longer depends on GL framebuffer import.
- Cross-device synchronization implemented explicitly, or rejected by branch policy in favor of one-device-only publication (document the choice).
- Relevant WPT suites at baseline or better.
- Reap list demotes (not silently preserves) GL framebuffer import as the default Windows path.

---

### S6 · WebXR wgpu-native design spike

**Scope.** Produce `docs/2026-MM-DD_webxr_wgpu_design_spike.md` covering:

- How `RenderingContextCore` (post-Phase-A) expresses XR frame lifecycle: predicted-display-time, per-eye submission.
- Layer-manager shape for Windows OpenXR (D3D11/D3D12) and Apple platforms (Metal).
- Mapping XR submission onto the S3 lease contract (link to S3-spike if landed; otherwise, sketch the integration assuming the plan's contract shape).
- Gaps in the core trait that would need closing before implementation. The plan claims Phase A validated this — verify against the audit doc, confirm or refute.

**Out of scope.** Implementation. Surfman GL XR stays untouched.

**Acceptance.** Spike doc with explicit confirm-or-refute of "Phase A's core trait can express XR lifecycle." If refuted, name the minimum trait change needed.

**Dependencies.** None. Independent. Strengthens if S3-spike is landed first but does not require it.

---

### S7 · Publication tracing skeleton

**Scope.**
- Add `tracing` events for producer acquire, producer publish, compositor import/sample, present completion, release/recycle.
- Key events by submission index or equivalent publication metadata.
- Stub emission points where S3 types don't exist yet; fully light up once S3 execution lands.

**Entry points.** Wherever `WebGLContextBusyMap` is manipulated today; `SurfmanSurfaceImporter`; Canvas publication paths; compositor pending-frame structure.

**Acceptance.** Tracing scaffold producing structured output now. Documented stub points with TODO tags linked to S3. No behavior change. Reap list names the `println!` or ad hoc logging it replaces.

**Dependencies.** None to start. Gains full value after S3-execution.

---

## 5. Cross-cutting Requirements

Every lane must:

- **Run the multi-embedder compile gate** if touching `components/shared/paint/` public surface.
- **Record WPT suite numbers before/after** if changing compositor or paint behavior.
- **Prefer `tracing` events over `println!`**. State-machine transitions must be observable.
- **Ship a reap list** (in the plan doc or commit message) naming symbols/paths deleted or scheduled for deletion.
- **Leave `webrender-wgpu` untouched.** Cross-repo changes are out of scope for servo-wgpu lanes. If a webrender-wgpu change is needed, stop and ask.

## 6. Out of Scope for Every Lane

Do not, without explicit dispatcher approval:

- Modify `webrender-wgpu`.
- `git merge servo/main` or perform a Servo upstream merge.
- `git push`.
- Bump dependency versions beyond what the lane strictly requires.
- Rework the repo's path-override relationship to `webrender-wgpu`.
- Land commits that mix two lanes.

## 7. Before You Start

1. Confirm the Phase A state: `git log --grep='Phase A' --oneline` shows A.1–A.7. If something is absent, stop and ask.
2. Read the canonical plan's section for your lane's phase (plan §1 for Phase A context, §2 for S3/S7, §3 for S4, §4 for S5, §5 for S6, §6 for S4 also).
3. For lanes touching `RenderingContextCore`: read the Phase A audit and trait-design docs first.
4. **State your understanding back to the dispatcher** — a 3-sentence summary of the lane's scope, your planned approach, and the acceptance bar — before making significant code changes. Catch scope mismatches early.

## 8. Definition of Done (per lane)

A lane is done when all of the following are true:

- Lane's acceptance bar satisfied.
- Reap list empty (items deleted) or explicitly deferred with a named successor lane.
- Multi-embedder compile gate green (if applicable).
- WPT numbers recorded (if applicable).
- Commit body names the lane (e.g. `S3-execution:`), cites prerequisite commits, and links the reap list.
- Dispatcher notified with a 3-sentence report: what landed, what the reap list looked like, what the next blocker is.
