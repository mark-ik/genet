# PaintList trait + polyglot NetRender (design, revised PM-2)

**Status (2026-05-17, revised PM-2):** proposed; revised after Mark's review. The earlier framing (extensions paint themselves into `vello::Scene` as the default ABI) is dropped — that pattern works in-process but breaks transport, capture/replay, tile caching, and resource ownership. This revision separates three layers that the first draft blurred, reframes extensions as **typed serializable payloads** rather than callbacks, and clarifies the text/font/glyph ownership boundary.

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
- `paint` (NetRender, the renderer) — input signature lifts from concrete `ServalDisplayList` to generic `<L: PaintList>`. Lowering machinery (translator.rs) stays renderer-private.

`paint_list_api` is the new crate.

---

## Decision 2 — hybrid pattern: common-minimum trait + typed payload extensions

The trait family mirrors `SemanticQuery`'s common-minimum + engine-extension pattern. **Critical difference from the first draft:** extensions are **typed serializable payloads**, not callbacks. Each engine declares its own extension enum (`ServalPaintExt`, `NematicPaintExt`); NetRender knows how to lower each variant into `netrender::Scene`.

### Trait + item sketch (serializable)

```rust
// paint_list_api/lib.rs

use serde::{Deserialize, Serialize};

/// What an engine emits. The unit of paint output for one rendered frame.
///
/// Fully serializable — `PaintMessage::SendDisplayList` can transport any
/// `PaintList` impl across IPC the same way it transports `ServalDisplayList`
/// today.
pub trait PaintList: Serialize + for<'de> Deserialize<'de> + Clone + Debug {
    /// Engine that produced this list. Names the extension variant family.
    fn engine_id(&self) -> EngineId;

    /// Final viewport this paint output is computed against.
    fn viewport(&self) -> DeviceIntSize;

    /// Generation/epoch matching FragmentQuery. Tile caches and frame
    /// schedulers key on this; rolls when the source regenerates.
    fn generation_id(&self) -> u64;

    /// Paint commands in paint order.
    fn commands(&self) -> &[PaintCmd<Self::Extension>];

    /// The engine-specific extension variant family.
    type Extension: PaintExtensionPayload;
}

/// What every extension variant must satisfy. It's **data**, not a callback.
pub trait PaintExtensionPayload:
    Serialize + for<'de> Deserialize<'de> + Clone + Debug
{
    /// Painted bounds, in local (post-transform/clip) coordinates. For
    /// culling. Renderers may skip extensions whose bounds fall outside the
    /// current clip.
    fn bounds(&self) -> Rect;
}

/// The common command stream. Compositor primitives + paint primitives + a
/// typed extension hole for engine-specific items.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PaintCmd<E: PaintExtensionPayload> {
    // Compositor primitives — composition state stack.
    PushClip(ClipSpec),
    PopClip,
    PushTransform(LayoutTransform),
    PopTransform,
    PushLayer(LayerSpec),       // opacity, blend mode, mask, scroll-container
    PopLayer,
    PushReferenceFrame(ReferenceFrameSpec),
    PopReferenceFrame,

    // Paint primitives — common-minimum item set (expanded; see below).
    DrawRect(RectItem),
    DrawStroke(StrokeItem),
    DrawLinearGradient(LinearGradientItem),
    DrawRadialGradient(RadialGradientItem),
    DrawConicGradient(ConicGradientItem),
    DrawText(TextRunItem),                  // shaped glyph runs from layout
    DrawImage(ImageItem),
    DrawRepeatingImage(RepeatingImageItem),
    DrawExternalTexture(ExternalTextureItem),
    DrawShadow(ShadowItem),

    // Engine-specific items, as data.
    Extension(E),
}
```

### What NetRender does with this

```rust
// In netrender (its own crate).

impl NetRenderer {
    pub fn render<L: PaintList>(&mut self, list: &L) {
        let mut composition = CompositionState::new(list.viewport());
        for cmd in list.commands() {
            match cmd {
                PaintCmd::PushClip(c)           => composition.push_clip(c),
                PaintCmd::PopClip               => composition.pop_clip(),
                // ... compositor primitives ...
                PaintCmd::DrawRect(r)           => self.lower_rect(&composition, r),
                PaintCmd::DrawText(t)           => self.lower_text(&composition, t),
                PaintCmd::DrawLinearGradient(g) => self.lower_linear_gradient(&composition, g),
                // ... common primitives ...
                PaintCmd::Extension(ext)        => self.lower_extension(&composition, ext),
            }
        }
    }
}

// NetRender's extension-lowering dispatch knows about specific engine
// payloads it has been compiled against. Initial version: built-in match
// arms keyed on engine_id + variant. Later: registration table.
impl NetRenderer {
    fn lower_extension<E: PaintExtensionPayload>(&mut self, c: &CompositionState, ext: &E) {
        // E is generic, but NetRender depends on the concrete extension
        // crates (paint_list_api_serval, paint_list_api_nematic, ...) and
        // downcasts via a registered type-id table OR via direct generic
        // monomorphization per engine. See "extension dispatch" below.
    }
}
```

**Extension dispatch — three options:**

1. **Generic monomorphization.** NetRender has `render::<ServalPaintList>(...)`, `render::<NematicPaintList>(...)` etc. Each instance knows the concrete extension type at compile time. Match arm dispatch on the extension enum. Fastest; requires NetRender to depend on each extension crate. Crate dep graph is "NetRender → engine extension crates," which inverts the desired "engines depend on api, NetRender consumes generically" graph.

2. **Trait-object dispatch with registered renderers.** Extensions are `dyn PaintExtensionPayload`; engines register `(EngineId, Box<dyn ExtensionRenderer>)` with NetRender at startup. NetRender looks up by engine_id, dispatches. Preserves the clean dep graph (engines don't appear in NetRender's deps); pays a per-extension lookup + dyn call. Reasonable since extensions are infrequent vs common items.

3. **Compile-time list of extensions baked into NetRender.** NetRender depends on each engine's extension crate (small types-only crates), pattern-matches the engine_id, dispatches to the right lowerer. Hybrid of 1 and 2 — extension types are statically known to NetRender, but lookup is runtime. Requires NetRender's Cargo.toml to list each engine's extension crate.

Lean **option 2** for the v1 design — preserves the dep graph cleanliness and the per-extension cost is bounded. Option 1 is a perf optimization if profiling later shows extension dispatch is hot. Option 3 is a compromise that buys little.

The detail of which option will be revisited at scaffold time; the doc commits to "typed payloads, NetRender owns lowering" without yet committing to the dispatch mechanism.

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

**Common items (committed):**

- Compositor: `PushClip`, `PopClip`, `PushTransform`, `PopTransform`, `PushLayer`, `PopLayer`, `PushReferenceFrame`, `PopReferenceFrame`.
- Fills: `DrawRect`, `DrawStroke`.
- Gradients: `DrawLinearGradient`, `DrawRadialGradient`, `DrawConicGradient`. **Promoted from extensions per Mark's correction** — gradients are a renderable primitive Vello supports natively; making them Serval extensions would hide reusable renderer capability behind per-engine dispatch.
- Text: `DrawText` (shaped glyph runs from layout; see [Text ownership](#text-ownership-boundary) below).
- Images: `DrawImage`, `DrawRepeatingImage`.
- External: `DrawExternalTexture` (Scrying's wgpu texture handoff).
- Shadows: `DrawShadow`. Common because both HTML (`box-shadow`) and likely Nematic-aesthetic uses (block quote drop shadow as a styling choice) want it.

**Extension candidates (engine-specific):**

- *Serval:* paint worklets (CSS Houdini Painter), mix-blend-mode applied to non-layer primitives, masks beyond simple alpha, specific HTML form-control native paint (checkboxes / radio buttons / scrollbars when delegated to system theme).
- *Nematic:* terminal-aesthetic blocks with cell-grid + cursor markers, ASCII-art preservation hints, gemtext-quote-with-side-rule as an atomic stylable command.
- *Scrying:* none beyond `DrawExternalTexture`.

**Borderline items:** complex multi-corner-radius borders, gradient-strokes, image-borders. Could be common or extension. Lean common since they're primitive-shaped and most renderers want them; revisit if the common item count balloons.

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
2. **Typed extension** — emit engine-specific data; NetRender's extension lowerer handles it.
3. **External texture handoff** — paint yourself (Vello direct, wgpu direct, whatever), register the texture, declare it in PaintList as `DrawExternalTexture(handle)`.

The first draft conflated options 2 and 3 by proposing extensions paint into Vello. The corrected design separates them: extensions are *typed data NetRender knows how to render*; direct-paint is *external textures composed into NetRender's output*.

---

## Per-engine impl notes

### Serval

Current `ServalDisplayList` renames to `ServalPaintList`, implements `PaintList`. Existing `ServalDisplayItem` enum gets split:

- Items mapping to common `PaintCmd` variants emit those (rect, stroke, gradient family, text, image, repeating image, external texture, shadow).
- Items that are HTML/CSS-specific become `ServalPaintExt` enum variants.

`ServalPaintExt` is the per-engine extension payload type:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ServalPaintExt {
    PaintWorkletInvocation(PaintWorkletItem),
    MixBlendModeRegion(MixBlendModeItem),
    Mask(MaskItem),
    NativeFormControl(FormControlItem),
    // others as Serval needs them
}

impl PaintExtensionPayload for ServalPaintExt { fn bounds(&self) -> Rect { ... } }
```

NetRender's lowerer for `ServalPaintExt` lives either in NetRender (option 2 dispatch) or in a small `paint_list_serval_lowering` crate (option 3). Renderable from cached or transported `ServalPaintList` because everything's `Serialize`.

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
    PaintList trait, PaintCmd<E> enum, PaintExtensionPayload trait,
    common item types
    depends on: paint-types, serde (for Serialize)

netrender (its own crate; formerly "components/paint")
    consumes: PaintList (generic, but with registered extension renderers)
    lowering: PaintCmd → netrender::Scene (Vello underneath)
    owns: font registry, glyph atlas, image cache, external texture registry,
          composition state
    depends on: paint_list_api, vello, parley (for glyph rasterization
                helpers, NOT shaping)

paint-api (existing; embedder-facing cross-process API)
    PaintMessage::SendDisplayList(L: PaintList) — generic
    depends on: paint_list_api

serval-layout (produces ServalPaintList)
    ServalPaintList: impl PaintList
    ServalPaintExt: enum of CSS-rich items
    depends on: paint_list_api, parley (for shaping)

nematic
    NematicPaintList: impl PaintList (common items only initially)
    NematicPaintExt: empty enum initially
    depends on: paint_list_api

scrying lane wrapper
    ScryingPaintList: impl PaintList (one ExternalTexture command)
    depends on: paint_list_api, scrying
```

**Dep rules:**

- Engines don't depend on each other.
- NetRender depends on `paint_list_api` (the trait) but ideally not on individual engine extension types; extensions register at runtime (option 2 dispatch). If option 3 (compile-time list) is picked later, NetRender adds dep on each engine's extension type crate (small, types-only).
- `paint_list_api` is the single shared trait surface every engine + NetRender depends on.
- `paint-api` is updated to carry `L: PaintList` generically (currently carries concrete `ServalDisplayList`).

---

## What changes in existing code

| Current | Becomes |
| --- | --- |
| `components/shared/paint/serval_display_list.rs::ServalDisplayList` | `serval-layout::ServalPaintList: impl PaintList`. Same data fields (viewport, items, spatial nodes, clips, transforms); items split between common `PaintCmd` and `ServalPaintExt`. |
| `components/shared/paint/lib.rs::PaintMessage::SendDisplayList(ServalDisplayList)` | `SendDisplayList(L: PaintList)` (generic). Wire payload is the serialized concrete type per engine. |
| `components/paint/translator.rs` | NetRender's internal lowering. Stays renderer-private. Updated to dispatch on common variants + extension lowerer registration. |
| `components/paint/` package name `servo-paint` | Eventually rename to `netrender` (already structurally that). |
| (none today) | New `components/shared/paint-list-api/` crate. |

The trait surface is straightforward; the work concentrates in:

1. Defining `paint_list_api` (the trait + common items + extension trait).
2. Refactoring `ServalDisplayList` items into common `PaintCmd` + `ServalPaintExt`.
3. Updating `PaintMessage::SendDisplayList` to be generic.
4. Updating `translator.rs` to dispatch on common variants + extension renderers.

Each can land as its own commit. The audit canary stays clean throughout — no SpiderMonkey impact.

---

## Open questions for review

1. **Crate name.** `paint_list_api` is the working name. Alternatives: `paint_command_api`, `paint_scene_api`, `render_input_api`, `paintlist`. Decide at scaffold time.

2. **Extension dispatch mechanism.** Options 1 (generic monomorphization), 2 (registered renderers), 3 (compile-time list) all valid. Lean 2 for v1 — preserves dep-graph cleanliness; per-extension cost is bounded. Revisit with profiling data once a real engine extension exists.

3. **Common-vocabulary coverage.** Have I missed renderable primitives that should be common from day one? Specific candidates worth a review pass: complex paths (Bezier outlines beyond stroke), filter primitives (SVG filters), CSS `backdrop-filter`. These exist in CSS but may also be wanted by other engines. Lean: stay primitive-shaped; gradients are in because Vello has them; filters are likely Serval extensions until proven cross-engine.

4. **Text run shape.** Sketched `TextRunItem` with `Vec<GlyphInstance>`. The actual parley→paint hand-off may want a richer shape (per-cluster source-text-range mapping for selection, line-box info for hit-test). Detailed design at first implementation; the high-level "shaped runs come from layout, not NetRender" is the load-bearing point.

5. **PaintList::commands() return type.** Sketched `&[PaintCmd<...>]` (slice). Could be an iterator. Slice is simpler; iterator allows lazy/streaming production. Lean slice — paint output is built-then-shipped, not streamed.

6. **Generation_id collision across PaintLists in a composite.** If a tile has multiple PaintLists (Serval page with embedded Scrying iframe), generation_ids don't share namespace. NetRender's cache key would be `(source_id, generation_id)`. Defer until multi-source composition is a real concern.

7. **PaintList trait object support.** `dyn PaintList` would need extension type erased. Lean: don't support `dyn PaintList` at the trait level; require generic monomorphization at the host's render dispatch. (Host has finite set of lane types; dispatching by lane type then calling `render::<L>(...)` is fine.)

---

## Review checklist

- [ ] Is the three-layer separation (producer / transport / renderer scene) clear enough? Any layer still bleeding into another in the trait shape?
- [ ] Is the typed-payload extension model the right pattern, or does some use case genuinely need callbacks? (I can't think of one that wouldn't be better served by `DrawExternalTexture` + direct paint outside the pipeline.)
- [ ] Common vocabulary — anything missing? Gradients are in. Shadows are in. Borders are common-shaped via stroke. Anything else that's renderable-primitive but I left out?
- [ ] Text-ownership boundary — is "shaping in layout, font/glyph/emission in NetRender" the right split? Or are there reasons NetRender needs to know about shaping (e.g., subpixel-position adjustment, hinting hints affecting line metrics)?
- [ ] Extension-dispatch option 2 (registered renderers) — is the per-extension dyn cost acceptable, or should we plan for option 3 (compile-time list) from the start?
- [ ] Crate name resonance: `paint_list_api` vs. `paint_command_api` vs. `paint_scene_api` vs. `paintlist` vs. ?

---

## Decision log

- **Decided 2026-05-17 PM-2:** Three layers are distinct: producer-facing `PaintList` trait (engine-friendly), transport-friendly serializable wire form (`PaintList` is fully serializable), renderer-private `netrender::Scene` (NetRender owns lowering). The first draft conflated these.
- **Decided 2026-05-17 PM-2:** Extensions are **typed serializable payloads** (`PaintExtensionPayload` trait, engine-specific enum variants), not callbacks. The first draft's `dyn PaintExtension::paint(&mut PaintContext)` model is rejected — it works in-process but breaks transport, capture/replay, tile caching, and resource ownership.
- **Decided 2026-05-17 PM-2:** Vello-direct access is an **escape hatch outside the PaintList pipeline**, not a feature of extensions. Lanes that want it use Vello directly and hand NetRender the resulting texture via `DrawExternalTexture`. This is what Scrying already does.
- **Decided 2026-05-17 PM-2:** Common vocabulary includes **gradients (linear/radial/conic)** and **shadows** from day one. Promotion criterion: if NetRender already supports the primitive, it's common, regardless of how many engines emit it today.
- **Decided 2026-05-17 PM-2:** **Text shaping happens in serval-layout** (parley). Paint carries shaped glyph runs (`TextRunItem` with `Vec<GlyphInstance>`). NetRender owns font registration, glyph cache/rasterization, scene emission. NetRender does not reshape.
- **Decided 2026-05-17 PM-2:** New shared crate (`paint_list_api` working name) in `serval/components/shared/`.
- **Decided 2026-05-17 PM-2:** `ServalDisplayList` renames to `ServalPaintList`, implements `PaintList`. Existing items split between common `PaintCmd` and `ServalPaintExt`. The wire format compatibility is preserved because both are `Serialize`.
- **Decided 2026-05-17 PM-2:** `paint-types` (primitives) and `paint-api` (embedder cross-process) stay as separate crates with their existing concerns. `paint_list_api` is new and complements them.
- **Decided 2026-05-17 PM-2:** `PaintMessage::SendDisplayList` becomes generic over `L: PaintList`. The same envelope carries any engine's paint output without renaming.
- **Open:** crate name spelling; extension dispatch mechanism (lean option 2); common-vocabulary edge cases (paths, filters); TextRunItem detailed shape; PaintList::commands() iterator vs slice; multi-source generation_id; trait-object support.
