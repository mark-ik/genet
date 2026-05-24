# PaintList layer extraction — neutral shared crate (plan)

**Status: DONE (2026-05-24).** Extraction committed (`15e9a0c`): `paint_list_api` + the
`PaintCmd→Scene` translator now live in the netrender workspace, and serval consumes them
(`paint_list_api`/`paint_list_render = { path = "../netrender/…" }`);
`components/shared/paint-list-api` + `components/paint/translator.rs` are deleted. (Was
"planned; no code moved yet", 2026-05-20.)
Successor to [2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md)
(the PM-3 design + as-built receipts). That doc designed a polyglot
renderer-input layer but, per the constraint below, the layer shipped
**inside serval** rather than as shared infrastructure. This plan lifts it
out so any engine — not just Serval — can feed paint output to the
netrender pipeline.

---

## Why it's serval-side today (the constraint)

The PM-3 dep graph placed PaintList consumption "in netrender"
([design doc §Crate dep graph](./2026-05-17_paintlist_polyglot_renderer.md)).
That couldn't ship as written:

1. **netrender is a fork of `servo/webrender`.** Its remotes are
   `origin = mark-ik/webrender-wgpu`, `upstream = servo/webrender`. It is
   kept rebaseable on upstream and engine-agnostic. Housing
   `paint-list-api` inside the `netrender` crate would inject Mere/Serval
   engine vocabulary (`EngineId::{SERVAL,NEMATIC,SCRYING}`, CSS display-list
   concepts) into a servo-lineage crate.
2. **Dependency inversion.** `paint-list-api` depends on `paint_types`
   (= `servo-paint-types`, a serval-workspace path crate). netrender is
   pulled *into* serval (`../netrender/netrender`); the arrow is
   serval → netrender. Putting `paint-list-api` in netrender would force
   netrender → servo-paint-types.

So the boundary landed one crate lower: `netrender` stays a pure
`Scene`/`SceneOp` rasterizer, and the `PaintList → Scene` adapter lives in
serval's `servo-paint` crate (`components/paint`). The design doc papered
over this by *calling* `components/paint` "netrender" — but they are
distinct crates.

**Consequence the extraction fixes:** the polyglot consumption layer is
trapped in serval. The second real renderer consumer, inker, proves it —
[`document-canvas/src/netrender_backend.rs`](../../mere/crates/inker/document-canvas/src/netrender_backend.rs)
re-rolls its own `DocumentRenderPacket → netrender::Scene` backend rather
than reaching for `paint-list-api`. Two consumers, two doors, no shared
layer.

---

## Target architecture

Both serval and mere already reach into the netrender repo by relative
path (serval: `../netrender/netrender`; inker:
`../../../../netrender/netrender`). The netrender **workspace** is
therefore already the neutral ground both consume — so the shared layer
becomes new workspace members there, **as siblings to the `netrender`
crate, not inside it**.

```
repos/netrender/                 (workspace; servo/webrender lineage)
  netrender/                     ← renderer primitives. UNTOUCHED. stays clean.
  netrender_device/
  netrender_text/
  paint_list_api/      ← NEW. engine-facing vocabulary + own primitives.
                          deps: euclid, serde (+ malloc_size_of? — see Open items).
                          NO netrender dep.
  paint_list_render/   ← NEW. the translator: paint_list_api → netrender::Scene.
                          deps: netrender, paint_list_api.

repos/serval/components/paint/   (servo-paint) ← shrinks to GPU glue + message loop.
                                                  deps paint_list_render.
repos/mere/crates/inker/document-canvas/  ← repoints through paint_list_api path.
```

**Dependency rules:**

- Engines (`serval-layout`, `nematic`, `inker`, future) depend only on
  `paint_list_api` — the CSS-ish engine-facing vocabulary. Never on
  netrender.
- Only the painter/host depends on `paint_list_render` (which depends on
  netrender).
- `netrender` depends on neither. Upstream-rebaseable surface preserved.

### Two resolved decisions

- **Crate home:** netrender-workspace member (sibling crates), not a
  standalone repo and not the mere workspace. Co-locates the
  renderer-input layer with the renderer; both consumers already path-dep
  into this repo; `netrender` crate itself stays untouched.
- **Primitives:** **fork the primitives subset** into `paint_list_api`,
  do **not** repoint onto netrender's types. The inventory (below) showed
  repointing is infeasible and backwards: netrender deliberately models
  renderer *primitives* (bare `f32`/`[f32;4]`, no euclid, no CSS enums),
  while `paint_list_api` is the engine-facing *display-list* vocabulary.
  The translator is the deliberate impedance bridge between the two;
  collapsing them would push CSS→primitive lowering up into the engines.

---

## Primitive inventory (what forks vs. what's already shared)

`paint_list_api` pulls these from `servo-paint-types`. Severing that dep
means lifting the subset into `paint_list_api`'s own `primitives` module.

**Already shared (netrender has an equivalent — trivial to define
locally, no semantic gap):**

| Type | netrender equivalent | Note |
| --- | --- | --- |
| `ImageKey` | `netrender::ImageKey` (u64) | opaque key; identical shape |
| `GradientStop` | `netrender::GradientStop` | offset + color; color premultiplies at lowering |

**Must fork (CSS / display-list vocabulary with NO netrender home — these
are the engine-facing contract, by design):**

- Geometry (euclid aliases): `LayoutPoint`, `LayoutRect`, `LayoutSize`,
  `LayoutSideOffsets`, `LayoutVector2D`, `LayoutTransform`,
  `DeviceIntSize`, `DeviceIntSideOffsets`. netrender uses bare floats; the
  translator already converts.
- Color: `ColorF` (unpremultiplied RGBA). Lowers to `[f32;4]`
  premultiplied — conversion already in `translator::color_to_array`.
- Border: `BorderStyle`, `BorderSide`, `NormalBorder`, `BorderRadius`.
- Lines/shadows: `LineStyle`, `BoxShadowClipMode`.
- Images/gradients: `ImageRendering`, `ExtendMode`, `RepeatMode`.
- Blend/transform: `MixBlendMode` (full 17-variant set; netrender's
  `SceneBlendMode` exposes only 6 — the truncation already happens at
  lowering in `mix_blend_mode_to_scene` and is unchanged by this move),
  `TransformStyle`.
- Font identity: `FontInstanceKey` (+ `IdNamespace` it embeds). netrender's
  `FontId` is a bare u32; mapping happens at lowering.

These are small: euclid type-aliases with unit markers + plain `enum`s.
euclid and serde are crates.io deps; the fork is low-cost and does **not**
re-create a "third vocabulary" problem — it relocates the one engine-facing
vocabulary out of serval.

---

## Translator cut — movable vs. stays-in-serval

`translator.rs` is ~930 lines; the pure `PaintCmd → Scene` walk + its
helpers is ~550 lines (tests excluded). Its only imports are
`netrender::{…}`, `paint_list_api::{…}`, `paint_types::ColorF`,
`rustc_hash` — **zero serval-internal imports**. That core moves cleanly.

| Concern | Verdict | Pinning dep |
| --- | --- | --- |
| Core `PaintCmd` match → `SceneOp` emission | **MOVE** → `paint_list_render` | none |
| Font side-table → `push_font` / `DrawText` lowering | **MOVE** | none (stateless; no FontRegistry type) |
| Image side-table → `set_image_source` / `DrawImage` | **MOVE** | none (stateless) |
| `DrawPath` / `DrawStroke` warn!-and-skip stubs | **MOVE** | none (deferred-feature placeholders; ride along) |
| `ExternalTextureDraw` struct (data) | **MOVE** | none |
| `BoxShadowMaskRequest` struct (data) | **MOVE** | none |
| External-texture **materialization** (compositor pass, wgpu texture registry) | **STAYS** | `netrender_painter::Paint`, `compositor::PaintCompositor`, `wgpu::Texture` |
| Box-shadow GPU **mask build** | **STAYS** | `netrender::Renderer::build_box_shadow_mask` (live GPU handle) |
| `PaintMessage` / `PaintProxy` / IPC transport | **STAYS** | `paint_api`, `servo_base`, `ipc_channel` |
| Painter message loop / `PipelineState` / renderer registry | **STAYS** | `Paint` struct, `RenderingContextCore`, per-pipeline state |

The translator already returns a `TranslatedDisplayList { scene,
external_texture_draws, box_shadow_mask_requests }`. That struct is the
clean handoff: `paint_list_render` produces it (pure); serval's `Paint`
consumes it and does the GPU glue. The cut is along an existing seam.

---

## Phases

Each phase is independently landable and leaves the tree green.

**P1 — scaffold `paint_list_api` in the netrender workspace.**
Copy `lib.rs`/`items.rs`/`specs.rs`, add a `primitives` module forked from
the servo-paint-types subset above. Resolve the `malloc_size_of` question
(Open items). New crate compiles + its own round-trip tests pass, in
isolation. Serval not yet touched.

**P2 — scaffold `paint_list_render`.** Move the pure translator walk +
helpers + `TranslatedDisplayList`/`ExternalTextureDraw`/`BoxShadowMaskRequest`.
Depends on `netrender` + `paint_list_api`. Port the translator's own unit
tests. Compiles + passes in isolation.

**P3 — repoint serval onto the workspace crates.** serval's
`paint_list_api` path-dep → `../netrender/paint_list_api`; delete the
in-tree `components/shared/paint-list-api/`. `components/paint` imports the
translator from `paint_list_render`; delete the moved code, keep the GPU
glue + message loop. `servo-paint-types` stays for serval-internal uses
(it's still used elsewhere). Run serval paint tests
(`paint_list_render_e2e`, `html_to_pixels_e2e`, `webgl_canvas_texture_e2e`).

**P4 — repoint inker (mere) through the PaintList path.** Make inker emit
a `PaintList` (an `InkerPaintList` or reuse `PaintEnvelope`) and lower via
`paint_list_render`, replacing the bespoke `netrender_backend.rs` walk.
This is the payoff: inker becomes the second genuine `paint_list_api`
producer, validating the polyglot story the way Nematic was meant to. Per
mere DOC_POLICY §8 this phase gets a mere-side plan pointer + DOC_README
entry when it starts.

---

## Open items / risks

1. **`malloc_size_of` sourcing.** `paint_list_api` currently derives
   `MallocSizeOf` and the `PaintList` trait bounds it. servo's
   `malloc_size_of` comes via the stylo git pin — not available in the
   netrender workspace. Options: (a) pull `malloc_size_of` from the same
   stylo git rev as a shared pin; (b) **drop the `MallocSizeOf` bound** from
   the neutral crate and have serval re-add memory reporting via a
   newtype/wrapper if it's load-bearing; (c) vendor a trivial impl. Lean
   (b) — keep the neutral crate dep-light unless cross-workspace mem
   reporting is actually needed. Decide at P1.
2. **`MixBlendMode` truncation is pre-existing.** netrender exposes 6 of
   17 modes; lowering already drops the rest. The move doesn't change this;
   note it so it isn't mistaken for a regression introduced here.
3. **`DrawPath` / `DrawStroke` gaps ride along** as warn!-and-skip stubs.
   Their eventual kurbo::BezPath wiring will be pure and lands in
   `paint_list_render`, not serval — which is the right home post-move.
4. **euclid version unification.** `paint_list_api`'s euclid must match
   whatever serval and inker already resolve, to avoid type-identity splits
   across the path-dep boundary. Pin via the netrender workspace's
   `[workspace.dependencies]`.
5. **`PaintEnvelope` flat-struct shape is retained** (it already avoids the
   dep inversion the PM-3 doc's enum would have caused — see as-built
   note in the design doc). No change needed; it moves as-is.

---

## Progress

- 2026-05-20: plan written. Decisions resolved — crate home (netrender
  workspace member), primitives (fork the subset; do not repoint onto
  netrender types, after inventory showed repointing infeasible +
  boundary-collapsing).

- 2026-05-20: **P1 + P2 + P3 landed and green.**
  - **P1** — `netrender/paint_list_api/` created: `lib.rs` + `items.rs` +
    `specs.rs` (moved) + new `primitives.rs` (forked subset of
    servo-paint-types: euclid `Layout*`/`DeviceInt*` aliases, `ColorF`,
    `BorderStyle`/`LineStyle`/`BoxShadowClipMode`/`BorderRadius`/
    `NormalBorder`/`BorderSide`, `ExtendMode`/`RepeatMode`/`GradientStop`,
    `ImageRendering`, `MixBlendMode`/`TransformStyle`,
    `IdNamespace`/`ImageKey`/`FontInstanceKey`). Deps reduced to
    **euclid + serde**. 7 round-trip tests pass in isolation.
  - **`malloc_size_of` decision: dropped** (Open item #1, option b).
    Evidence: netrender uses no `malloc_size_of` in source, and serval's
    is the `servo-malloc-size-of` *path* crate — pulling it into the
    netrender workspace would re-introduce the serval coupling being
    severed. The `MallocSizeOf` supertrait bound on `PaintList` and all
    derives are gone.
  - **P2** — `netrender/paint_list_render/` created: the translator moved
    verbatim (imports repointed paint_types→paint_list_api; `FxHashMap`→
    std `HashMap`, dropping the rustc-hash dep; structs + entry points
    made `pub`). Deps: **netrender + paint_list_api**. 7 translator tests
    pass against netrender.
  - **P3** — serval repointed: workspace `paint_list_api` →
    `../netrender/paint_list_api`, added `paint_list_render`; `servo-paint`
    re-exports the translator from `paint_list_render` and dropped
    `mod translator;`; `netrender_painter.rs` imports the structs from
    `paint_list_render`. Deleted `components/paint/translator.rs` and the
    in-tree `components/shared/paint-list-api/` (+ its workspace member).
    `serval-layout::ServalPaintList` dropped its `MallocSizeOf` derive.
    `cargo check -p servo-paint -p serval-layout` green; all five paint
    test targets compile (`--no-run`).
  - **P3 GPU validation:** full `cargo test -p servo-paint`
    (`--test-threads=1`) green — **28 passed, 0 failed, 1 ignored** across
    c4_smoke_probe (3), html_to_pixels_e2e (19), paint_list_render_e2e (2),
    paint_render_e2e (3), webgl_canvas_texture_e2e (1). The verbatim
    translator move is behavior-identical through real wgpu rasterization.

  **Implementation finding (DOC_POLICY §9):** forking the primitives made
  `paint_list_api::{ColorF,ImageKey,IdNamespace}` *distinct* from the
  `paint_types` originals the paint test files had been importing (they'd
  relied on the old re-export aliasing them to the same type). Fix was
  mechanical — repoint those imports in the 5 test files to
  `paint_list_api`, keeping `PipelineId` / `units::*` from `paint_types`
  where used for non-paint-list purposes. This type-identity split is the
  expected, correct consequence of severing the dependency; any *future*
  consumer must import paint vocabulary from `paint_list_api`, not
  `paint_types`.

- 2026-05-22: **P4 (inker repoint) — landed and green.** Mere's
  `document-canvas` now produces an `InkerPaintList: impl PaintList` and
  lowers it via `paint_list_render::translate_paint_list`, replacing the
  bespoke `DocumentRenderPacket → netrender::Scene` walk. **Inker is now
  the second genuine PaintList producer** — the extraction's payoff.
  - New portable `paint_list.rs` producer (depends only on
    `paint_list_api`; no wgpu — builds wasm-light). `netrender_backend.rs`
    reduced to a lowering shim behind the `netrender` feature.
  - **Font model: canonical bytes side-table** (chosen over an external
    pre-registered-font-map entry point on the translator, which would
    have split the shared crate's font model). `FontResolver::resolve_font_id
    → resolve_font_data` (face bytes + collection index); inker interns
    each face once into the `fonts()` side-table; the shared translator is
    unchanged. No production consumer existed (`FontResolver` was
    document-canvas-internal), so the trait reshaped freely.
  - `EngineId::INKER = Self(3)` added to `paint_list_api` (+ sentinel test).
  - `cargo test -p document-canvas --features netrender` → **23 passed,
    0 failed** (11 layout, 8 portable producer, 4 scene-level integration).
    `cargo build -p document-canvas` (no feature) green — producer stays
    wgpu-free. Mere-side plan + DOC_README entry filed per DOC_POLICY §8:
    `mere/design_docs/inker_docs/implementation_strategy/2026-05-22_inker_paintlist_adoption_plan.md`.

**Extraction complete.** All four phases landed. The PaintList layer
(`paint_list_api` + `paint_list_render`) lives neutrally in the netrender
workspace; serval and inker are two producers of the same vocabulary, both
lowering through one shared translator; `netrender` stays a pure
`Scene` rasterizer with its `servo/webrender` upstream remote intact.
