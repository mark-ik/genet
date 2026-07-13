# Proposed-upstream notes — genet's two load-bearing fork patches

Status: **drafts, ready to offer — not pushed.** genet depends on two
patched forks (see the [2026-05-24 audit addendum](./2026-05-24_workspace_audit_snapshot.md#addendum--2026-05-25-review-post-snapshot-developments--the-rendering-pipeline)).
Policy is **offer-don't-push**: keep a clean pitch on hand so that *if* a
maintainer ever shows interest there's something ready to hand over — but
don't open unsolicited PRs. Each patch below is additive and (we argue)
generally useful beyond genet, so neither is a "please carry my weird
hack" ask.

---

## 1. Nova — an embedder native-data slot (`EmbedderObject`)

- **Fork:** `github.com/mark-ik/nova`, branch `genet-embedder`, commit `fbca54b`
  (local clone at `crates/nova`, pinned via root `[patch.crates-io]`).
- **Upstream:** `github.com/trynova/nova`.

**What the patch adds.** A way to associate **opaque host (embedder) data**
with a JS object — genet attaches a DOM `NodeId` reflector, so script that
holds a JS node object mutates the *real* genet DOM through it. The
round-trip is GC-safe (verified: a reflector survives garbage collection and
still resolves to its `NodeId`).

**Why genet needs it.** The scripting tier (`genet-scripted`) bridges JS ↔
the `genet-scripted-dom` arena through `NodeId` reflectors. Without a native
slot on JS objects there's nowhere to hang the `NodeId`, so JS can't reach the
DOM at all.

**Why it's generally useful (the pitch).** *Every* embedder of a JS engine
needs to associate host objects with JS objects — DOM nodes, file handles, GPU
resources, FFI handles. It's a standard embedding primitive: V8 has internal
fields / `External`, SpiderMonkey has reserved slots / private data, JSC has
private data. Nova currently has no equivalent, which forces any embedder to
fork exactly as genet did. A first-class embedder-data slot unblocks the whole
class of "use Nova to script my app" use cases — which is squarely Nova's
stated "small, embeddable, data-oriented JS engine" goal.

**Clean-PR shape.** Add an `EmbedderObject` (exotic object, or a typed
native-data slot on ordinary objects) with safe get/set and correct GC tracing
of the embedder payload. Additive; no behavior change for non-embedding users.

---

## 2. Xilem / Masonry — realize `VisualLayerKind::External` (embedder composite hook)

- **Fork:** `crates/xilem`, branch `mere-wgpu-29-vello-0-9`, commit `694cc7f`
  (`masonry_winit`).
- **Upstream:** `github.com/linebender/xilem`.

**What the patch adds.** It *finishes an existing upstream placeholder*.
`masonry_core` already defines `VisualLayerKind::External { bounds }` —
documented as "a placeholder for externally realized content" — and the paint
pass already emits it (`push_external_layer`). But `masonry_winit`'s render
path **only handled `Scene` layers and silently skipped `External` ones**, so
the concept was inert. The patch:

- collects `External` layers in `redraw()` (logical → physical-px bounds), and
- after `render_to_texture` / before present, calls a new
  `AppDriver::composite_external_layers(&mut ExternalCompositeCtx)`
  (**default no-op**) handing the embedder the shared `wgpu::Device`/`Queue`,
  the surface `target_texture` (already `Rgba8Unorm` + `COPY_DST`), and each
  layer's bounds — so the embedder draws its own GPU content into the holes.

Paired with the existing `AppDriver::on_wgpu_ready` (shared device), this is a
**zero-copy** embedding path: no GPU→CPU readback.

**Why genet needs it.** `pelt-viewer` reserves the web-content area as an
`External` layer and composites genet's netrender output into it via
`copy_texture_to_texture` on Masonry's shared device. Without the realization
hook the layer is a no-op hole and nothing renders.

**Why it's generally useful (the pitch).** It completes Masonry's *own* design
— `VisualLayerKind::External` exists precisely for this — and unlocks embedding
**any** externally-rendered GPU content into a Masonry/Xilem window: a web
engine, a video decoder's frames, a `<canvas>`/WebGL surface, a game viewport,
another renderer. The change is additive and default-no-op, so it's invisible
to every existing Masonry app. Low-controversy: it's not a new abstraction,
it's the missing back half of one they already shipped.

**Clean-PR shape.** The `composite_external_layers` hook + `ExternalLayer` /
`ExternalCompositeCtx` types + the `redraw()`/`render()` wiring. (The branch's
wgpu-29 / vello-0.9 version bump is *separate* — upstream will do that on their
own cadence — and is **not** part of this proposal.)
