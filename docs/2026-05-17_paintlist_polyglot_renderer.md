# PaintList trait + polyglot NetRender (design, revised PM-3)

**Status (2026-05-17, revised PM-3):** proposed; foundation decisions resolved after second review pass. PM-3 commits the dispatch mechanism (option 1, compile-time enum), the wire transport shape (`PaintEnvelope` enum, not generic), the `DrawExternalTexture` lowering contract (per-frame compositor pass, not vello `SceneOp::Image`), and the common-vocabulary deltas (`DrawPath` added; `PushReferenceFrame` renamed to `PushTransform`; filters common). Audit of current `ServalDisplayItem` variants finds `ServalPaintExt` v1 effectively empty; the extension machinery may be deferred. See PM-3 entries in [Decision log](#decision-log).

**Earlier status (PM-2):** proposed; revised after Mark's review. The earlier framing (extensions paint themselves into `vello::Scene` as the default ABI) is dropped — that pattern works in-process but breaks transport, capture/replay, tile caching, and resource ownership. PM-2 separated three layers that the first draft blurred, reframes extensions as **typed serializable payloads** rather than callbacks, and clarifies the text/font/glyph ownership boundary.

---

## Implementation status (2026-05-18)

PM-3 design landed in code over 2026-05-17 → 2026-05-18. The
codebase ships **one path** — `ServalPaintList` (or any `PaintList`
impl) → `PaintEnvelope` wire payload → `translate_envelope` →
`netrender::Scene`. There is no `ServalDisplayList` retirement
story because `ServalDisplayList` is gone; we cut it in the same
arc rather than carrying a migration corridor for a codebase with
no production consumers.

### Receipts

| PM-3 decision | Implementation |
| --- | --- |
| `paint_list_api` crate (closed-set vocabulary) | [`components/shared/paint-list-api/`](../components/shared/paint-list-api/) — `lib.rs` (`PaintList` trait, `PaintCmd` enum, `EngineId`, `PrimitiveFlags`, `CommonPlacement`, `PaintEnvelope`), `specs.rs` (compositor primitives + `FilterOp`), `items.rs` (Draw\* payloads incl. `ExternalTextureItem.content_generation`) |
| `PaintCmd` compile-time dispatch | `PaintCmd` is monomorphic; `Extension` variant deferred per audit (Serval `ServalPaintExt` empty in practice) |
| `PaintEnvelope` closed-set wire payload | [`paint-list-api/lib.rs::PaintEnvelope`](../components/shared/paint-list-api/lib.rs) — flat struct (`engine: EngineId`, `viewport`, `generation`, `commands`); chosen over the doc's literal enum to avoid `paint-api` → engine-crate dep inversion. Self-translatable: `impl PaintList for PaintEnvelope` |
| `DrawExternalTexture` compositor-pass lowering | [`components/paint/translator.rs`](../components/paint/translator.rs) routes `DrawExternalTexture` to `ExternalTextureDraw` (separate compositor vec), not `SceneOp::Image`. `content_generation: Option<u64>` forward-looking field on `ExternalTextureItem` for future texture-as-source lowerings |
| `DrawPath` in common vocabulary | `PaintCmd::DrawPath(PathItem)` with `PathData` / `PathCommand` (MoveTo / LineTo / QuadTo / CurveTo / Close). Translator emits a `warn!` placeholder — full lowering needs kurbo::BezPath plumbing (cleared in netrender; painter-side wiring pending) |
| `PushReferenceFrame` → `PushTransform` | `PaintCmd::PushTransform(TransformSpec)` with `TransformKind { Standard, Preserve3D, Perspective }` |
| Filters in `LayerSpec` | `LayerSpec::filters: Vec<FilterOp>` with full FilterOp surface (Blur, Brightness, Contrast, Grayscale, HueRotate, Invert, Opacity, Saturate, Sepia, DropShadow, ColorMatrix). `Opacity` collapses into layer alpha at lowering; others currently no-op until backdrop machinery wires through |
| `generation_id` as relowering-skip hint | Field on `PaintList`, `PaintEnvelope`; tile-cache invalidation remains via SceneOp content-hashing (decoupled) |
| Capture/replay at both layers | `PaintEnvelope` is `Serialize + Deserialize` (paint-list-api lib tests round-trip); `netrender::Scene` snapshots ship behind the `serde` feature on netrender |
| Producer (serval-layout) | [`components/serval-layout/paint_emit.rs`](../components/serval-layout/paint_emit.rs) — `emit_paint_list` / `emit_paint_list_with_layouts` produce `ServalPaintList` directly from the cascade + fragment + style planes |
| `PaintMessage::SendPaintList` wire variant | [`paint-api/lib.rs::PaintMessage::SendPaintList`](../components/shared/paint/lib.rs) carries `PaintEnvelope`; `PaintProxy::send_paint_list` wraps the send |
| Painter dispatch | [`components/paint/netrender_painter.rs::handle_one_message`](../components/paint/netrender_painter.rs) matches `SendPaintList`, routes through `translate_envelope_with_external_textures`, pipeline_id from `paint_info` |

### Pipeline as built

```text
  ┌─ producer (serval-layout) ─┐                ┌─ paint (renderer) ──────────────┐
  │   FragmentPlane            │                │  Paint::handle_messages         │
  │   StylePlane               │                │           │                     │
  │   TextMeasureCtx           │                │           ▼                     │
  │           │                │                │   translate_envelope_with_      │
  │           ▼                │                │   external_textures             │
  │   emit_paint_list(_with_   │                │           │                     │
  │   layouts)                 │                │           ▼                     │
  │           │                │                │   PipelineState { scene,        │
  │           ▼                │                │     external_textures, ... }    │
  │   ServalPaintList          │                │           │                     │
  │           │                │                │           ▼                     │
  │           ▼                │  SendPaintList │   Renderer::render_with_        │
  │   PaintEnvelope::from_list ───────────────► │   compositor[_and_external_     │
  │           │                │                │   textures]                     │
  │           ▼                │                │           │                     │
  │   send_paint_list          │                │           ▼                     │
  └────────────────────────────┘                │        master                   │
                                                └─────────────────────────────────┘
```

### Remaining work (painter-side wiring gaps)

Three `PaintCmd` variants currently `warn!` and skip in the translator. Each needs new `Paint`-level state, not just translator changes:

1. **`DrawText` → `FontRegistry`.** Painter needs `FontInstanceKey → netrender::FontId` map; translator emits `SceneOp::GlyphRun`.
2. **`DrawImage` / `DrawRepeatingImage` → `ImageRegistry`.** Painter needs `ImageKey → netrender::ImageKey` map; translator emits `SceneOp::Image` / `SceneOp::Pattern`.
3. **`DrawShadow` → `Renderer::build_box_shadow_mask`.** Painter constructs the blurred mask texture via its `Renderer` handle, registers under an `ImageKey`, emits as an `Image` primitive.

`DrawStroke` and `DrawPath` also `warn!`-and-skip pending kurbo::BezPath plumbing.

Sister reads:

- [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md) — cross-engine lane architecture. Paint Plane summary points here.
- [2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md) — serval-layout's planes design. Paint output is what's lifted here.
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md) — the analogous design for the DOM-side trait. This doc mirrors the common-minimum + extensions pattern on the renderer side.

---

## The three layers (separated)

Earlier versions of this doc blurred three distinct layers. They must stay distinct because they have different requirements (serializability, ownership, evolution).

| Layer | What it is | Today | Requirement |
| --- | --- | --- | --- |
| **(1) Producer-facing paint surface** | The trait engines implement to publish paint output | (not yet abstracted; concrete `ServalDisplayList`) | Engine-friendly; allows engine-specific items |
| **(2) Transport / wire payload** | Serializable form that crosses IPC / file caches | [`PaintMessage::SendDisplayList(ServalDisplayList)`](c:/Users/mark_/Code/repos/serval/components/shared/paint/lib.rs#L135) | **Must be `Serialize + Deserialize + Clone + MallocSizeOf`**. No `dyn` trait objects. |
| **(3) Renderer scene** | NetRender's internal optimized form | [`netrender::Scene` via `components/paint/translator.rs`](c:/Users/mark_/Code/repos/serval/components/paint/translator.rs) | Renderer-private. Engines never see this. |

The first version of this doc conflated layers 1 and 3 — proposing `PaintExtension::paint(&mut PaintContext { scene: &mut vello::Scene })`. That makes the producer surface depend on Vello and forces layer 3 details into engines' code. It also makes the layer-1 trait fundamentally non-transportable, since `dyn PaintExtension` is not `Serialize`.

The corrected design:

- Layer 1 (`PaintList` + typed payload variants) is the producer surface engines learn.
- Layer 1 is **fully serializable** so layer 2 can carry it directly. The producer surface and the wire payload are the same data shape.
- Layer 3 (`netrender::Scene` lowering) is NetRender's internal concern. Engines don't see it. NetRender owns the dispatch from `PaintList` → `netrender::Scene`.
- Direct Vello access is an **escape hatch outside the PaintList pipeline**, not a feature of extensions. Lanes that want to talk to Vello directly do so peer to NetRender, not through it.

---

## Decision 1 — separate crate (`paint_list_api`)

The `PaintList` trait + common-minimum item types + engine-extension hooks live in a new shared crate in `serval/components/shared/`. Working name: `paint_list_api`.

The crate is owned by neither Serval nor Nematic; consumed by NetRender, Serval-layout, Nematic, Scrying lane wrapper, and any future engine that wants the same renderer.

**Plausible consumers beyond Serval:**

1. **NetRender** — primary consumer; renders any `PaintList` impl.
2. **Nematic** — produces `NematicPaintList` for protocol-faithful rendering.
3. **Scrying lane wrapper** — produces a one-item `PaintList` containing the system-webview texture handle.
4. **Future engines** — PDF lane, markdown-direct, anything that wants paint without inventing its own renderer.
5. **Test harnesses** — synthetic `PaintList` impls for renderer testing without real engines.

That clears the "separate iff plausible additional consumer" bar by a wide margin.

### What stays in existing paint crates

- `paint-types` — image keys, color types, units. Common primitives. Kept; consumed by `paint_list_api`.
- `paint-api` — the cross-process embedder-facing API (`PaintMessage`, painter handles). Different concern; stays.
- `paint` (NetRender, the renderer) — input signature lifts from concrete `ServalDisplayList` to the closed-set `PaintEnvelope` enum (PM-3). Lowering machinery (translator.rs) stays renderer-private.

`paint_list_api` is the new crate.

---

## Decision 2 — hybrid pattern: common-minimum trait + typed payload extensions

The trait family mirrors `SemanticQuery`'s common-minimum + engine-extension pattern. **Critical difference from the first draft:** extensions are **typed serializable payloads**, not callbacks. Each engine declares its own extension enum (`ServalPaintExt`, `NematicPaintExt`); NetRender knows how to lower each variant into `netrender::Scene`.

### Trait + item sketch (serializable, post-PM-3)

```rust
// paint_list_api/lib.rs

use serde::{Deserialize, Serialize};

/// What an engine emits. The unit of paint output for one rendered frame.
///
/// Fully serializable — `PaintEnvelope` can transport any `PaintList` impl
/// across IPC the same way `PaintMessage::SendDisplayList(ServalDisplayList)`
/// does today. PM-3: trait drops the generic `Extension` associated type;
/// engine-specific payloads ride in the finite-set `PaintPayload` enum.
pub trait PaintList: Serialize + for<'de> Deserialize<'de> + Clone + Debug {
    /// Engine that produced this list. Names the `PaintPayload` variant
    /// family this list emits.
    fn engine_id(&self) -> EngineId;

    /// Final viewport this paint output is computed against.
    fn viewport(&self) -> DeviceIntSize;

    /// Producer-rolled semantic-equivalence epoch. Same `(source_id,
    /// generation_id)` asserts identical paint output and resource refs;
    /// NetRender may use this to skip *relowering* (PaintList → Scene).
    /// **Not a tile-cache invalidation key** — tile-cache correctness still
    /// derives from SceneOp content hashing post-lowering.
    fn generation_id(&self) -> u64;

    /// Paint commands in paint order.
    fn commands(&self) -> &[PaintCmd];
}

/// The common command stream. Compositor primitives + paint primitives + a
/// typed extension hole carrying the finite-set `PaintPayload` enum.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PaintCmd {
    // Compositor primitives — composition state stack.
    PushClip(ClipSpec),
    PopClip,
    PushTransform(TransformSpec),       // PM-3: renamed from PushReferenceFrame
    PopTransform,                       // PM-3: renamed from PopReferenceFrame
    PushLayer(LayerSpec),               // opacity, blend mode, filters, mask, scroll-container
    PopLayer,

    // Paint primitives — common-minimum item set (expanded; see below).
    DrawRect(RectItem),
    DrawStroke(StrokeItem),
    DrawLine(LineItem),                 // wavy / dashed / solid; text-decoration shape
    DrawPath(PathItem),                 // PM-3 add: Bezier outlines via vello kurbo
    DrawBorder(BorderItem),             // normal + nine-patch
    DrawLinearGradient(LinearGradientItem),
    DrawRadialGradient(RadialGradientItem),
    DrawConicGradient(ConicGradientItem),
    DrawText(TextRunItem),              // shaped glyph runs from layout
    DrawImage(ImageItem),
    DrawRepeatingImage(RepeatingImageItem),
    DrawExternalTexture(ExternalTextureItem),
    DrawShadow(ShadowItem),
    PushShadow(ShadowSpec),             // text-shadow style state
    PopAllShadows,

    HitTest(HitTestItem),               // hit-test region with tag

    // Engine-specific items, as data. Finite static set per PM-3.
    Extension(PaintPayload),
}

/// PM-3: finite static payload set. Each variant is a small types-only crate
/// owned by the originating engine; NetRender depends on those crates
/// (tolerable dep-graph cost for the v1 engine count) and pattern-matches
/// on the enum. No `dyn` dispatch, no TypeId downcasting.
///
/// Audit (PM-3) of current `ServalDisplayItem` finds every variant collapses
/// to common ops or `DrawExternalTexture`; v1 may ship with all three inner
/// payloads empty (or the entire `Extension` variant deferred). See
/// [Per-engine impl notes](#per-engine-impl-notes).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PaintPayload {
    Serval(ServalPaintExt),
    Nematic(NematicPaintExt),
    Scrying(ScryingPaintExt),
}
```

### What NetRender does with this

```rust
// In netrender (its own crate).

impl NetRenderer {
    pub fn render(&mut self, list: &dyn PaintList) {
        let mut composition = CompositionState::new(list.viewport());
        for cmd in list.commands() {
            match cmd {
                PaintCmd::PushClip(c)           => composition.push_clip(c),
                PaintCmd::PopClip               => composition.pop_clip(),
                PaintCmd::PushTransform(t)      => composition.push_transform(t),
                PaintCmd::PopTransform          => composition.pop_transform(),
                PaintCmd::PushLayer(l)          => composition.push_layer(l),
                PaintCmd::PopLayer              => composition.pop_layer(),
                // ... compositor primitives ...
                PaintCmd::DrawRect(r)           => self.lower_rect(&composition, r),
                PaintCmd::DrawText(t)           => self.lower_text(&composition, t),
                PaintCmd::DrawLinearGradient(g) => self.lower_linear_gradient(&composition, g),
                PaintCmd::DrawPath(p)           => self.lower_path(&composition, p),
                PaintCmd::DrawExternalTexture(e)=> self.lower_external_texture(&composition, e),
                // ... common primitives ...
                PaintCmd::Extension(payload)    => self.lower_extension(&composition, payload),
            }
        }
    }

    // PM-3: finite static dispatch. NetRender depends on the small
    // types-only payload crates and pattern-matches the enum.
    fn lower_extension(&mut self, c: &CompositionState, payload: &PaintPayload) {
        match payload {
            PaintPayload::Serval(ext)  => self.lower_serval_ext(c, ext),
            PaintPayload::Nematic(ext) => self.lower_nematic_ext(c, ext),
            PaintPayload::Scrying(ext) => self.lower_scrying_ext(c, ext),
        }
    }
}
```

### Extension dispatch — PM-3 resolution

**Selected: compile-time enum (option 1).** `PaintCmd` is monomorphic; engine-specific items ride in the finite `PaintPayload` enum. NetRender depends on the small types-only payload crates and pattern-matches at compile time. No `dyn PaintExtensionPayload`, no TypeId downcasting, no runtime registration table.

**Why option 2 (registered renderers with `dyn`) was dropped:** `PaintCmd<E>` was compile-time typed; preserving that while also doing runtime `dyn` registration fights the type shape. Either commit to mono (option 1) or restructure for true tag-based open dispatch (gltf-style). Option 2 was the worst of both — preserved the typed enum shape but bolted dyn dispatch on top, requiring TypeId machinery that bought nothing over plain pattern matching.

**Why option 3 (compile-time list, hybrid) was dropped:** identical dep graph to option 1, more machinery. Buys nothing.

**Dep graph cost is bounded.** Three known engines (Serval, Nematic, Scrying) means NetRender's `Cargo.toml` gains three types-only deps. Each engine extension crate is a small payload enum + `bounds()` impl. If engine count grows beyond ~6 or open-extension semantics become real, tag-based dispatch (gltf-pattern: `(stable_tag, postcard_bytes)` with registered decoders) is the cutover path. Today's count doesn't justify the open-dispatch complexity.

### Real-world prior art

- **WebRender custom display items** (Servo's prior renderer). Display lists carried a defined common vocabulary plus embedder-supplied typed items. Closest direct analog to this design.
- **PostScript / PDF content streams** with extension dictionaries. Common operators + typed XObjects for embedded content. 1980s prior art for "common ops + typed extensions."
- **SVG `<foreignObject>`** + namespaced extension elements. SVG's escape hatch for embedded non-SVG content with typed metadata.
- **gltf** (3D scene transfer format) — common nodes + extension table with named extensions; renderers opt in to extensions they support.
- **OpenType feature tables** — common shaping + script/feature extensions. Conceptually similar pattern.

Note what's **not** here: egui's `PaintCallback`, which I cited in the first draft. PaintCallback is a callback pattern — works in-process only, can't cross IPC, can't be cached or replayed. It was the wrong analog for a renderer with a transport layer.

---

## The common vocabulary (expanded from first draft)

The common-minimum `PaintCmd` variants are **renderable primitives**, not "HTML/CSS-shaped." If NetRender already supports it and any engine could reasonably want to emit it, it's common.

**Common items (committed, post-PM-3):**

- Compositor: `PushClip`, `PopClip`, `PushTransform` (PM-3: was `PushReferenceFrame`), `PopTransform`, `PushLayer`, `PopLayer`. `PushReferenceFrame`/`PopReferenceFrame` dropped — WebRender legacy that doesn't map to a NetRender primitive; `PushTransform` with a `TransformSpec` is the honest common shape.
- Fills: `DrawRect`, `DrawStroke`, `DrawLine` (wavy/dashed/solid; text-decoration shape).
- **Paths: `DrawPath`** (PM-3 add). Bezier outlines via vello `kurbo`. NetRender has the machinery (`SceneOp::Shape`, R2/R3 cleared path-precise containment), so by the "renderer capability belongs in common" criterion this is common from day one.
- Borders: `DrawBorder` (normal + nine-patch). Normal lowers to 4 strokes; nine-patch is image-sliced — both are renderable primitives.
- Gradients: `DrawLinearGradient`, `DrawRadialGradient`, `DrawConicGradient`. Renderable primitives Vello supports natively; making them per-engine would hide reusable renderer capability behind dispatch.
- Text: `DrawText` (shaped glyph runs from layout; see [Text ownership](#text-ownership-boundary) below).
- Images: `DrawImage`, `DrawRepeatingImage`.
- External: `DrawExternalTexture` (Scrying / WebGL canvas / iframe / paint worklet output / native form controls / anything-rasterized-by-the-producer).
- Shadows: `DrawShadow` (box-shadow), `PushShadow` / `PopAllShadows` (text-shadow state-stack pair).
- Hit-testing: `HitTest` (region + tag). Every engine needs hit-testing for selection/input.
- **Filter primitives in `LayerSpec`** (PM-3 clarification). `PushLayer` carries an optional filter chain (`Vec<FilterOp>`: `Blur`, `Brightness`, `Contrast`, `DropShadow`, `Opacity`, `ColorMatrix`, etc.). NetRender's Roadmap D1 shipped filter-via-backdrop machinery; filters are common renderer capability, not per-engine. Includes `Opacity` (subsumes the older "alpha multiplier on every item" pattern) and `DropShadow` (filter-chain shadow, distinct from `DrawShadow`'s box-shadow).

**Extension candidates (engine-specific, post-PM-3 audit):**

Per the [PM-3 audit of `ServalDisplayItem`](#per-engine-impl-notes), the cited extension candidates from PM-2 all collapse:

- *Serval paint worklets* → `DrawExternalTexture` (Serval rasterizes the worklet to a wgpu texture and hands the handle to NetRender).
- *Serval mix-blend-mode on non-layer primitives* → common (`PushLayer` carries `mix_blend_mode` in `LayerSpec`).
- *Serval masks beyond simple alpha* → common (`PushLayer` with `mask`; Roadmap C3 already shipped `SceneCompose::DestIn`).
- *Serval native form controls* → `DrawExternalTexture` (system-themed widget rasterized by the platform integration layer).
- *Nematic terminal-aesthetic blocks* → common (`PushTransform` + `DrawRect` + `DrawText` sequence; aesthetic comes from the composition, not from a custom primitive).
- *Nematic ASCII-art preservation hints* → not paint at all — a hint field on `TextOptions` (font selection / antialiasing preference), no command of its own.
- *Nematic gemtext quote-with-side-rule* → common decomposition (rect + stroke + text).
- *Scrying* → already `DrawExternalTexture` exclusively.

**Genuinely engine-specific items left:** CSS-3D `transform_style: Preserve3D` and `raster_space: Local` on stacking contexts (Serval-only). Both could be flags on common `LayerSpec` rather than a separate variant. **Practical upshot: `ServalPaintExt` v1 has zero non-trivial variants** — see [Per-engine impl notes / Serval](#serval).

**Borderline items:** complex multi-corner-radius borders, gradient-strokes, image-borders. Lean common (primitive-shaped, most renderers want them). Revisit if the common item count balloons.

---

## Text ownership boundary

**Critical correction from first draft.** I had said "NetRender owns text shaping via parley." That's wrong — `serval-layout` needs shaped metrics for line breaking. If NetRender reshapes independently, layout and paint can drift (wrap positions, hit-test, selection rects all stop matching).

**The correct boundary:**

| Concern | Owner |
| --- | --- |
| Font face selection, fallback chain | layout (with platform integration via the host) |
| Text **shaping** (codepoint → glyph mapping with positioning) | layout (parley) — produces shaped runs |
| Line **breaking** + paragraph layout | layout (parley) — consumes shaped runs |
| Font **registration** (loading face data, sharing across paint passes) | NetRender |
| Glyph **cache** + rasterization (glyph atlas, hinting, subpixel positioning) | NetRender |
| **Scene emission** (glyph runs → Vello text primitives) | NetRender |

Paint carries shaped glyph runs in `TextRunItem`:

```rust
pub struct TextRunItem {
    pub bounds: Rect,
    pub font_key: FontKey,        // resolves in NetRender's font registry
    pub glyphs: Vec<GlyphInstance>, // shaped + positioned, from parley
    pub color: ColorF,
    pub options: TextOptions,     // subpixel positioning, hinting hints, etc.
}

pub struct GlyphInstance {
    pub index: GlyphIndex,        // index into the shaped font
    pub point: LayoutPoint,       // baseline-aligned position
}
```

NetRender's job: take `FontKey` + `GlyphIndex` + `point`, look up the glyph in its atlas (rasterizing if cold), emit the right Vello text primitive. Doesn't reshape. Doesn't reflow. The shaping is fixed at the layout layer.

This matches WebRender's prior model (Servo's prior renderer) and parley's intended integration shape.

---

## Vello-direct is an escape hatch outside the PaintList pipeline

If a lane wants to talk to Vello directly — bypassing the common vocabulary and the PaintList pipeline entirely — it can. **That happens peer to NetRender, not through it.** The lane emits its own Vello scene (or its own rendered output) and the host composes it with NetRender output at the compositor layer.

This is what Scrying already does: it doesn't go through PaintList for its actual paint content; it hands the host a wgpu texture. Scrying's `PaintList` exists only to declare "place this external texture here" — the actual paint happens outside.

A future Nematic could do the same for a specific subset of its content (e.g., "this gemtext code block paints as a self-rendered wgpu texture for terminal-perfect rendering, registered as an ExternalTexture") without inventing a Nematic extension type. The `ExternalTexture` common variant handles it.

So a lane has three ways to put pixels on screen:

1. **Common vocabulary** — emit standard `PaintCmd` variants; NetRender renders.
2. **Typed extension** — emit engine-specific data; NetRender's extension lowerer handles it. (Per [PM-3 audit](#per-engine-impl-notes), v1 has no real users of this path; the machinery may be deferred.)
3. **External texture handoff** — paint yourself (Vello direct, wgpu direct, whatever), register the texture, declare it in PaintList as `DrawExternalTexture(handle)`.

The first draft conflated options 2 and 3 by proposing extensions paint into Vello. The corrected design separates them: extensions are *typed data NetRender knows how to render*; direct-paint is *external textures composed into NetRender's output*.

### PM-3: `DrawExternalTexture` lowering contract

**The lowering for `DrawExternalTexture` is the per-frame compositor pass** ([`ExternalTextureComposite`](https://github.com/mark-ik/netrender/blob/main/netrender/src/external_texture.rs) + `scene_op_boundary` + [`VelloTileRasterizer::render_overlay_fragment`](https://github.com/mark-ik/netrender/blob/main/netrender/src/vello_tile_rasterizer.rs), landed in netrender 2026-05-16), **not vello `SceneOp::Image`**. This is a load-bearing architectural commitment, not an impl detail.

**Why this matters:** the compositor pass reads the texture view at frame composite time. If the WebGL canvas (or any other external texture producer) redraws between frames, the next frame's composite picks up new pixels with no Scene mutation and no tile-cache invalidation needed. Tile cache hashes vello rasterization, which is unchanged; external textures composite *over* the rasterized master via a per-frame pipeline pass. **No `content_generation` is needed for this lowering** — the invalidation problem is sidestepped by construction.

**Forward-looking: `ExternalTextureItem.content_generation`.** Carried as `Option<u64>` for the adjacent case where an external texture is used as a *sampling source for other PaintCmds* (e.g., a future repeating-pattern op that samples the texture). Those lowerings would emit `SceneOp` variants that tile-cache on `ImageKey`; without `content_generation` in the tile-cache hash, the cache would miss content mutations. Default `None` for compositor-pass use; producers set it when texture-as-source. Producers that set `content_generation` accept the invalidation contract: rolling the generation marks the texture content as semantically new.

`scene_op_boundary` (the ordered-interleaving foundation in `ExternalTextureComposite`) is what makes this lowering credible for mid-page external textures — not just topmost overlays. Before that feature landed, the "Nematic code-block as self-rendered wgpu texture" example in option 3 would have been unwieldy (always-topmost). With ordered composite, textures can sit at any z-position in painter order.

---

## Per-engine impl notes

### Serval

Current `ServalDisplayList` renames to `ServalPaintList`, implements `PaintList`. Existing `ServalDisplayItem` enum is mapped to common `PaintCmd` variants.

**PM-3 audit of current `ServalDisplayItem` variants:**

| ServalDisplayItem variant | Lands as |
| --- | --- |
| `Rect` / `RectWithAnimation` | common `DrawRect` (animation field is currently a `None`-only stub — vestigial, renderer ignores) |
| `Line` | common `DrawLine` (wavy/dashed/solid; carries `LineStyle`) |
| `Image` / `RepeatingImage` | common `DrawImage` / `DrawRepeatingImage` |
| `ExternalTexture` | common `DrawExternalTexture` |
| `Text` | common `DrawText` (shaped glyph runs; see [Text ownership](#text-ownership-boundary)) |
| `Border` (normal + nine-patch) | common `DrawBorder` |
| `BoxShadow` | common `DrawShadow` |
| `PushShadow` / `PopAllShadows` | common compositor pair |
| `Gradient` / `RadialGradient` / `ConicGradient` | common `DrawLinearGradient` / `DrawRadialGradient` / `DrawConicGradient` |
| `Iframe` | lowers to `DrawExternalTexture` (iframe's rendered output is a texture from the parent pipeline's POV) |
| `PushStackingContext` / `PopStackingContext` | common `PushLayer` / `PopLayer` with `LayerSpec` carrying `mix_blend_mode`, optional `filters: Vec<FilterOp>`, opacity |
| `PushReferenceFrame` / `PopReferenceFrame` | common `PushTransform` / `PopTransform` (PM-3 rename) |
| `HitTest` | common `HitTest` (every engine needs hit-testing) |

**Genuinely Serval-only after this audit:** CSS-3D `transform_style: Preserve3D` and `raster_space: Local` on stacking contexts. Both could be flags on common `LayerSpec` rather than variants in a Serval extension enum.

**Practical upshot: `ServalPaintExt` v1 is empty (or near-empty).**

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ServalPaintExt {
    // No variants identified at scaffold time. CSS-3D fields (if not
    // absorbed into LayerSpec) would land here; otherwise empty.
}
```

The PM-2 list of cited extension candidates (paint worklets, mix-blend-mode regions, masks, native form controls) all collapse to common ops or `DrawExternalTexture` per the audit table above. The `PaintPayload::Extension` mechanism scaffolds for hypothetical future needs; **consider deferring the extension machinery entirely** until a real case surfaces (a future Houdini layout API, an SVG filter primitive Vello doesn't yet support, etc.).

**Decision deferred to scaffold time:** ship with empty `ServalPaintExt` and the extension machinery in place (smaller diff for the first real extension), or ship without the extension variant at all (smaller v1 surface). Both are viable; resolve when scaffolding the api crate.

`ServalDisplayList`-as-it-exists-today can either be renamed in-place or kept as a transitional wire format until the trait surface is in place. Lean rename-in-place; the trait shape is straightforward enough that the cutover doesn't need a transitional period.

### Nematic

Emits `NematicPaintList: impl PaintList`. Initially: common items only. `DrawText` for gemtext lines, `DrawRect` for quote-block backgrounds, `DrawStroke` for separators, `DrawImage` for inline images. NetRender renders natively. **No extension needed for most smolweb content.**

`NematicPaintExt` exists as an empty enum (or one with a single test variant) initially; populated later if Nematic discovers it wants protocol-shaped items. Until then, Nematic is "common-vocabulary-only" — proving NetRender's polyglot story on the cheap.

### Scrying

`ScryingPaintList` emits one `PaintCmd::DrawExternalTexture(scrying_texture_item)` at the viewport rect. The wgpu texture is registered out-of-band with NetRender's `ExternalTextureRegistry` (per the existing `ExternalTextureItem.texture_key` pattern from the 2026-05-15 paint refactor). NetRender composites natively. `ScryingPaintExt` empty.

---

## Graduation path (refined)

Extension items that prove cross-engine-useful **graduate** to common `PaintCmd` variants. Plus a second criterion: items that already have native renderer support should be common, not extensions, even if only one engine emits them today.

1. Engine A adds an extension variant (e.g., Serval's `LinearGradient` would have been an extension under the first draft).
2. NetRender already has native support for the underlying primitive (Vello renders linear gradients).
3. PR adds the common variant. Both criteria met → graduation is automatic, not a function of multi-engine demand.

(This is what makes gradients common from day one even though Nematic isn't emitting them today.)

Engine-specific items that **don't** have native renderer support (paint worklets, terminal-aesthetic blocks with custom cell-grid logic) stay as extensions until the renderer's primitive set absorbs them. That's the engine's burden to push for.

The common vocabulary stays small + primitive-shaped. CSS-shaped sugar stays Serval-specific. Renderer primitives stay common.

---

## Crate dep graph

```text
paint-types (existing)
    primitives: color, image keys, units

paint_list_api (new shared crate)
    PaintList trait, PaintCmd enum (monomorphic per PM-3),
    PaintEnvelope enum, common item types
    depends on: paint-types, serde, paint_list_serval / _nematic / _scrying
                (only if Extension variant is retained per scaffold decision)

paint_list_serval / paint_list_nematic / paint_list_scrying (small types-only crates)
    ServalPaintExt / NematicPaintExt / ScryingPaintExt payload enums
    depends on: paint-types, serde
    NOTE: deferred entirely if PM-3 audit's "defer Extension" path is taken

netrender (its own crate; formerly "components/paint")
    consumes: PaintEnvelope (concrete enum dispatch — no generics on the
              entry point)
    lowering: PaintCmd → netrender::Scene (Vello underneath)
    owns: font registry, glyph atlas, image cache, external texture registry,
          composition state
    depends on: paint_list_api, vello, parley (for glyph rasterization
                helpers, NOT shaping)

paint-api (existing; embedder-facing cross-process API)
    PaintMessage::SendDisplayList(PaintEnvelope) — closed-set enum per PM-3
    depends on: paint_list_api

serval-layout (produces ServalPaintList)
    ServalPaintList: impl PaintList
    (PM-3 audit: maps to common ops only; ServalPaintExt empty for v1)
    depends on: paint_list_api, parley (for shaping)

nematic
    NematicPaintList: impl PaintList (common items only)
    NematicPaintExt: empty
    depends on: paint_list_api

scrying lane wrapper
    ScryingPaintList: impl PaintList (one DrawExternalTexture per frame)
    ScryingPaintExt: empty
    depends on: paint_list_api, scrying
```

**Dep rules (post-PM-3):**

- Engines don't depend on each other.
- **PM-3:** if the `Extension` variant is retained, `paint_list_api` depends on the small types-only payload crates (`paint_list_serval` / `_nematic` / `_scrying`) — that's where the `PaintPayload` enum lives. NetRender inherits these as transitive deps. Tolerable for the v1 engine count; revisit if engine count grows past ~6 or open dispatch becomes a real requirement.
- **PM-3:** if the `Extension` variant is deferred (per [audit](#serval)), the payload crates aren't created at all; `paint_list_api` stands alone with the common vocabulary.
- `paint_list_api` is the single shared trait surface every engine + NetRender depends on.
- `paint-api` is updated to carry `PaintEnvelope` (closed-set enum), not generic `L: PaintList`.

---

## What changes in existing code

| Current | Becomes |
| --- | --- |
| `components/shared/paint/serval_display_list.rs::ServalDisplayList` | `serval-layout::ServalPaintList: impl PaintList`. Same data fields (viewport, items, spatial nodes, clips, transforms); items map to common `PaintCmd` variants per the [Serval audit table](#serval). `ServalPaintExt` empty or near-empty for v1. |
| `components/shared/paint/lib.rs::PaintMessage::SendDisplayList(ServalDisplayList)` | `PaintMessage::SendDisplayList(PaintEnvelope)` where `enum PaintEnvelope { Serval(ServalPaintList), Nematic(NematicPaintList), Scrying(ScryingPaintList) }`. **PM-3:** envelope is a closed-set enum, not generic over a trait — receivers need a concrete channel payload, and generic-over-trait isn't a workable wire shape. Tag-based open dispatch (gltf-style) is the cutover path if open extensions become real. |
| `components/paint/translator.rs` | NetRender's internal lowering. Stays renderer-private. Updated to dispatch on common variants + (if `Extension` variant retained) on `PaintPayload` enum match arms. **PM-3:** compile-time pattern match, no runtime registration. |
| `components/paint/` package name `servo-paint` | Eventually rename to `netrender` (already structurally that). |
| (none today) | New `components/shared/paint-list-api/` crate. |

The trait surface is straightforward; the work concentrates in:

1. Defining `paint_list_api` (the trait + common items + `PaintPayload` enum). Decide at scaffold time whether to include the `Extension` variant or defer entirely (see [Serval audit](#serval)).
2. Refactoring `ServalDisplayList` items into common `PaintCmd` per the audit table.
3. Updating `PaintMessage::SendDisplayList` to carry the `PaintEnvelope` enum.
4. Updating `translator.rs` to dispatch on common variants (+ `PaintPayload` if retained).

Each can land as its own commit. The audit canary stays clean throughout — no SpiderMonkey impact.

---

## Open questions for review

**Resolved at PM-3:** dispatch mechanism (option 1, compile-time enum); transport envelope shape (`PaintEnvelope` enum, not generic); `DrawExternalTexture` lowering (compositor pass); `generation_id` semantics (producer-rolled semantic-equivalence epoch; tile cache invariant separate); common-vocabulary additions (`DrawPath`, filters in `LayerSpec`, line/border/hit-test common); `PushReferenceFrame` rename to `PushTransform`; capture/replay both layers. See [Decision log](#decision-log).

**Remaining at PM-3:**

1. **Crate name.** `paint_list_api` is the working name. Alternatives: `paint_command_api`, `paint_scene_api`, `render_input_api`, `paintlist`. Decide at scaffold time.

2. **Whether to scaffold the `Extension` variant at all.** Per [Serval audit](#serval), `ServalPaintExt` v1 has no identified non-trivial variants. Two options:
   - **Keep:** ship `PaintCmd::Extension(PaintPayload)` with empty/near-empty payload enums. Smaller diff for the first real extension, scaffold lives.
   - **Defer:** ship without `Extension` entirely. Smaller v1 surface; when a real case surfaces, retrofitting the variant is a single-PR change.

   Lean **defer** — the audit suggests the machinery is scaffolding for hypothetical needs. Revisit when a concrete case emerges (Houdini layout API, SVG filter Vello doesn't render, etc.).

3. **Text run shape.** Sketched `TextRunItem` with `Vec<GlyphInstance>`. The actual parley→paint hand-off may want a richer shape (per-cluster source-text-range mapping for selection, line-box info for hit-test). Detailed design at first implementation; the high-level "shaped runs come from layout, not NetRender" is the load-bearing point.

4. **PaintList::commands() return type.** Sketched `&[PaintCmd]` (slice). Could be an iterator. Slice is simpler; iterator allows lazy/streaming production. Lean slice — paint output is built-then-shipped, not streamed.

5. **Generation_id collision across PaintLists in a composite.** If a tile has multiple PaintLists (Serval page with embedded Scrying iframe), generation_ids don't share namespace. NetRender's relowering-skip key would be `(source_id, generation_id)`. Defer until multi-source composition is a real concern.

6. **Border / stroke decomposition shape.** `DrawBorder` is common (per audit); nine-patch is a specialized pattern, normal borders are 4-stroke compositions. Decide at scaffold time whether `DrawBorder` is its own variant or whether the lowerer decomposes it into common strokes upstream of NetRender's match.

---

## Review checklist

- [x] Three-layer separation (producer / transport / renderer scene) — PM-2 separated; PM-3 confirms.
- [x] Typed-payload extension model — PM-2 chose payloads; PM-3 audited and found `ServalPaintExt` v1 empty, recommending defer.
- [x] Common vocabulary additions — PM-3 added `DrawPath`, filters-in-`LayerSpec`, common shadows/borders/hit-test; resolved.
- [ ] Text-ownership boundary — is "shaping in layout, font/glyph/emission in NetRender" the right split? Or are there reasons NetRender needs to know about shaping (e.g., subpixel-position adjustment, hinting hints affecting line metrics)?
- [x] Extension dispatch — PM-3: option 1 (compile-time enum). Option 2 rejected; option 3 collapsed into option 1.
- [x] Transport envelope shape — PM-3: closed-set `PaintEnvelope` enum, not generic.
- [x] `DrawExternalTexture` lowering contract — PM-3: compositor pass, not `SceneOp::Image`; cites `scene_op_boundary` foundation.
- [ ] Crate name resonance: `paint_list_api` vs. `paint_command_api` vs. `paint_scene_api` vs. `paintlist` vs. ?
- [ ] Whether to retain the `Extension` variant at all per PM-3 audit (lean defer).

---

## Decision log

### PM-3 (2026-05-17, foundation resolutions)

- **Decided PM-3:** Extension dispatch is **option 1 (compile-time enum)**. `PaintCmd` is monomorphic (no `<E>` generic); engine-specific items ride in `enum PaintPayload { Serval(ServalPaintExt), Nematic(NematicPaintExt), Scrying(ScryingPaintExt) }`. NetRender depends on the small types-only payload crates and pattern-matches at compile time. **Rejects PM-2 option 2** (registered renderers with `dyn PaintExtensionPayload`) — it fights the typed-payload shape and requires TypeId machinery that buys nothing over plain pattern matching.

- **Decided PM-3:** Transport is a **`PaintEnvelope` enum**, not a generic `PaintMessage::SendDisplayList<L: PaintList>`. Receivers need a concrete channel payload; generic-over-trait is not a workable wire shape. `enum PaintEnvelope { Serval(ServalPaintList), Nematic(NematicPaintList), Scrying(ScryingPaintList) }`. Tag-based open-extension dispatch (gltf-style: `(stable_tag, postcard_bytes)` with registered decoders) is the cutover path if open extensions ever become a real requirement.

- **Decided PM-3:** `DrawExternalTexture` **lowering contract** is the per-frame compositor pass (`ExternalTextureComposite` + `scene_op_boundary` + `VelloTileRasterizer::render_overlay_fragment`, landed in netrender 2026-05-16), **not vello `SceneOp::Image`**. This sidesteps tile-cache invalidation by construction: external textures aren't part of vello Scene; the compositor pass reads the texture view at frame time. The lowering choice is a load-bearing commitment, pinned in the doc — not an impl detail.

- **Decided PM-3:** `ExternalTextureItem` gains `content_generation: Option<u64>` as a forward-looking field. **Default `None`** for the compositor-pass lowering (no invalidation needed). **Producers set it** when an external texture is used as a sampling source for *other* PaintCmds (future repeating-pattern op, etc.) where the lowering would emit `SceneOp` variants tile-cached on `ImageKey` and need invalidation in the cache hash.

- **Decided PM-3:** `DrawPath` added to common vocabulary. Bezier outlines via vello kurbo; NetRender has the machinery already (`SceneOp::Shape`, R2/R3 path-precise containment). Passes the "renderer capability belongs in common" graduation criterion.

- **Decided PM-3:** `PushReferenceFrame` / `PopReferenceFrame` **renamed to `PushTransform` / `PopTransform`** in the common vocabulary. ReferenceFrame is WebRender legacy that doesn't map to a NetRender primitive; PushTransform is the honest name. (Serval-side internal spatial-tree state is unaffected — that's engine-local, not common-vocabulary.)

- **Decided PM-3:** Filter primitives are **common**, carried in `LayerSpec` on `PushLayer` (optional `filters: Vec<FilterOp>`). FilterOp variants (Blur, Brightness, Contrast, DropShadow, Opacity, ColorMatrix, etc.) are renderer-capability — Roadmap D1 shipped filter-via-backdrop machinery in netrender. Not per-engine.

- **Decided PM-3:** `generation_id` is a **producer-rolled semantic-equivalence epoch**, not a tile-cache invalidation key. Same `(source_id, generation_id)` asserts identical paint output and resource references; NetRender may use this to skip *relowering* (PaintList → Scene). Tile-cache correctness still derives from SceneOp content hashing post-lowering. Epoch is an optimization hint, not a correctness contract.

- **Decided PM-3:** Capture/replay supports **both layers** as first-class: `PaintList` snapshots (engine-output regression — does the engine emit stable paint output across renderer changes?) and `netrender::Scene` snapshots (renderer regression — Roadmap A2 already shipped). Each validates a different layer's stability.

- **Decided PM-3 (audit finding):** Per [Serval audit](#serval), every current `ServalDisplayItem` variant maps to common ops or `DrawExternalTexture`. **`ServalPaintExt` v1 is empty** (or carries only CSS-3D flags that could be absorbed into `LayerSpec`). The `Extension` variant on `PaintCmd` may be deferred entirely until a real case surfaces. Open at scaffold time: include empty-payload `Extension` (smaller diff for first real extension) vs. defer entirely (smaller v1 surface).

### PM-2 (2026-05-17, prior pass)

- **Decided 2026-05-17 PM-2:** Three layers are distinct: producer-facing `PaintList` trait (engine-friendly), transport-friendly serializable wire form (`PaintList` is fully serializable), renderer-private `netrender::Scene` (NetRender owns lowering). The first draft conflated these.
- **Decided 2026-05-17 PM-2:** Extensions are **typed serializable payloads** (`PaintExtensionPayload` trait, engine-specific enum variants), not callbacks. The first draft's `dyn PaintExtension::paint(&mut PaintContext)` model is rejected — it works in-process but breaks transport, capture/replay, tile caching, and resource ownership. *(PM-3: dispatch mechanism resolved to compile-time enum; the trait collapses to a finite `PaintPayload` enum.)*
- **Decided 2026-05-17 PM-2:** Vello-direct access is an **escape hatch outside the PaintList pipeline**, not a feature of extensions. Lanes that want it use Vello directly and hand NetRender the resulting texture via `DrawExternalTexture`. This is what Scrying already does.
- **Decided 2026-05-17 PM-2:** Common vocabulary includes **gradients (linear/radial/conic)** and **shadows** from day one. Promotion criterion: if NetRender already supports the primitive, it's common, regardless of how many engines emit it today.
- **Decided 2026-05-17 PM-2:** **Text shaping happens in serval-layout** (parley). Paint carries shaped glyph runs (`TextRunItem` with `Vec<GlyphInstance>`). NetRender owns font registration, glyph cache/rasterization, scene emission. NetRender does not reshape.
- **Decided 2026-05-17 PM-2:** New shared crate (`paint_list_api` working name) in `serval/components/shared/`.
- **Decided 2026-05-17 PM-2:** `ServalDisplayList` renames to `ServalPaintList`, implements `PaintList`. Items map to common `PaintCmd` variants (PM-3 audit: `ServalPaintExt` empty for v1).
- **Decided 2026-05-17 PM-2:** `paint-types` (primitives) and `paint-api` (embedder cross-process) stay as separate crates with their existing concerns. `paint_list_api` is new and complements them.
- **Decided 2026-05-17 PM-2:** ~~`PaintMessage::SendDisplayList` becomes generic over `L: PaintList`.~~ *Superseded by PM-3:* envelope is `PaintEnvelope` enum, not generic.

### Still open

Crate name spelling; whether to scaffold `Extension` variant at all (PM-3 leans defer); `TextRunItem` detailed shape; `PaintList::commands()` iterator vs slice; multi-source generation_id namespace; `DrawBorder` as variant vs upstream-decomposition.
