# PaintList trait + polyglot NetRender (design, for review)

**Status (2026-05-17):** proposed; for review. Captures Mark's vision for NetRender as a polyglot structured-document renderer that consumes a `PaintList` trait family — common-minimum vocabulary + engine-specific extensions. Symmetric to how `SemanticQuery` works for cross-engine document inspection.

This doc resolves the Paint Plane vocabulary question raised in the Hekate doc. Sister reads:

- [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md) — cross-engine lane architecture. Paint Plane section points here for the vocabulary detail.
- [2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md) — serval-layout's planes design. Paint output (today `ServalDisplayList`) is renamed and lifted under this proposal.
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md) — the analogous design for the DOM-side trait. Sets the pattern this doc mirrors on the renderer side.

---

## The problem this fixes

Earlier docs proposed that all three lanes (Nematic, Serval, Scrying) produce `ServalDisplayList`. That's functional but architecturally muddy:

1. **Name leaks Serval origin** into a vocabulary that ought to be cross-engine. If Nematic produces it, Serval shouldn't own it.
2. **Items are HTML/CSS-derived.** `Rect`, `Border`, `Gradient`, `StackingContext`, `Image`. Nematic gets the items it needs but is constrained to HTML-shaped primitives. Protocol-shaped items (terminal-aesthetic blocks, gemtext quote-with-side-rule as an atom) have no home.
3. **NetRender becomes implicitly Serval-coupled.** If its input is "ServalDisplayList," its identity is tied to one engine.

The fix: NetRender becomes a **polyglot structured-document renderer** that lives on its own terms. Its input is the `PaintList` trait family, not a concrete type. Engines implement `PaintList` with whatever concrete representation they prefer; NetRender renders any impl.

---

## Decision 1 — separate crate (`render_scene_api` or similar)

The `PaintList` trait + common-minimum item types + `PaintExtension` trait live in a **new shared crate** in `serval/components/shared/`. Working name: `render_scene_api`. (Alternatives: `paint_list_api`, `paint_api` (already exists, would need to repurpose), `render_api`.)

The crate is owned by neither Serval nor Nematic; consumed by NetRender, Serval-layout, Nematic, Scrying lane wrapper, and any future engine that wants the same renderer.

**Plausible consumers beyond Serval:**

1. **NetRender** — primary consumer; renders any PaintList impl. Lives in its own crate (already structurally true post-audit).
2. **Nematic** — produces NematicPaintList for protocol-faithful rendering.
3. **Scrying lane wrapper** — produces a one-item PaintList containing the system-webview texture.
4. **Future engines** — PDF lane later, Markdown-direct, anything that wants to paint without inventing its own renderer.
5. **Test harnesses** — synthetic PaintList impls for renderer testing without a real engine in the loop.

That clears the "separate iff plausible additional consumer" bar by a wide margin.

### What stays in existing paint crates

- `paint-types` already exists — image keys, color types, units. Keep as is; not duplicated.
- `paint-api` (the cross-process paint API, embedder-facing) stays for embedder integration. Different concern from `render_scene_api`.

`render_scene_api` is the new crate; it complements the existing two without absorbing them.

---

## Decision 2 — hybrid pattern (common minimum + extension trait)

The trait family mirrors the SemanticQuery pattern from the Hekate doc: **common minimum trait** + **engine-specific extensions**, with NetRender as the polyglot consumer that handles common items natively and dispatches extensions back to the producing engine for rendering.

This is structurally identical to:

- `SemanticQuery` + `HtmlSemanticExt` / `NematicSemanticExt` / `FeedSemanticExt` — common-minimum traits + engine extensions, Hekate consuming the common minimum.
- `LayoutDom` + capability traits (`ReplacedElementProvider`, etc.) — common-minimum DOM + opt-in capabilities, serval-layout consuming.

Same shape, applied to the paint output.

### Trait sketch

```rust
// In render_scene_api/lib.rs

/// What an engine emits for a renderer to consume. The unit of paint output
/// for a single rendered frame of a single source.
pub trait PaintList {
    /// Iterator type — opaque per-engine so concrete representation
    /// stays flexible (Vec, generator, etc.).
    type CmdIter<'a>: Iterator<Item = PaintCmd<'a>>
    where
        Self: 'a;

    /// Paint commands in paint order.
    fn paint_commands(&self) -> Self::CmdIter<'_>;

    /// Final viewport this paint output is computed against.
    fn viewport(&self) -> Size;

    /// Generation/epoch matching FragmentQuery. Renderers can cache
    /// rendered output keyed by this; rolls when the source regenerates.
    fn generation_id(&self) -> u64;
}

/// The common minimum vocabulary. Every renderer consuming PaintList
/// handles these natively; every engine can use them without coordinating
/// extensions.
pub enum PaintCmd<'a> {
    // Compositor primitives — composition state stack.
    PushClip(Rect),
    PopClip,
    PushTransform(Affine),
    PopTransform,
    PushLayer(LayerSpec),  // opacity, blend mode, mask, scroll-container
    PopLayer,

    // Paint primitives — the common-minimum item set.
    DrawRect(RectItem),               // filled rectangles (background, etc.)
    DrawStroke(StrokeItem),           // outlined paths (borders-as-line is StrokeItem)
    DrawText(TextItem<'a>),           // parley glyph runs
    DrawImage(ImageItem),             // raster + raster-resampling info
    DrawExternalTexture(ExternalTextureItem),  // wgpu texture handoff (Scrying)

    // Extension escape hatch — engine-specific items.
    Extension(&'a dyn PaintExtension),
}

/// Engine-specific paint items. The engine owns the rendering; the renderer
/// provides the canvas + composition state.
pub trait PaintExtension: std::fmt::Debug + Send + Sync {
    /// Which engine produced this; for renderer profiling and debug.
    fn engine_id(&self) -> EngineId;

    /// Painted bounds, in local (post-transform/clip) coordinates.
    /// Used for culling. Renderers may skip extensions whose bounds
    /// fall outside the current clip.
    fn bounds(&self) -> Rect;

    /// Paint into the renderer's scene at the current composition state.
    /// The extension reads vello directly, writes into the supplied scene.
    fn paint(&self, ctx: &mut PaintContext);
}

/// The composition surface the extension paints into.
pub struct PaintContext<'a> {
    pub scene: &'a mut vello::Scene,
    pub transform: Affine,
    pub clip: Option<Rect>,
    pub viewport: Size,
    pub glyph_atlas: &'a GlyphAtlas,        // shared text resources
    pub image_cache: &'a mut ImageCache,    // shared raster cache
    pub external_textures: &'a ExternalTextureRegistry, // for ExternalTexture refs
}
```

### How NetRender consumes it

```rust
// In netrender (its own crate).

pub struct NetRenderer { /* glyph atlas, image cache, scene state, vello renderer */ }

impl NetRenderer {
    pub fn render<L: PaintList>(&mut self, list: &L) {
        let mut composition_state = CompositionState::new(list.viewport());
        for cmd in list.paint_commands() {
            match cmd {
                PaintCmd::PushClip(r)            => composition_state.push_clip(r),
                PaintCmd::PopClip                => composition_state.pop_clip(),
                PaintCmd::PushTransform(t)       => composition_state.push_transform(t),
                PaintCmd::PopTransform           => composition_state.pop_transform(),
                PaintCmd::PushLayer(spec)        => composition_state.push_layer(spec),
                PaintCmd::PopLayer               => composition_state.pop_layer(),

                PaintCmd::DrawRect(r)            => self.paint_rect_native(&composition_state, r),
                PaintCmd::DrawStroke(s)          => self.paint_stroke_native(&composition_state, s),
                PaintCmd::DrawText(t)            => self.paint_text_native(&composition_state, t),
                PaintCmd::DrawImage(i)           => self.paint_image_native(&composition_state, i),
                PaintCmd::DrawExternalTexture(e) => self.paint_external_native(&composition_state, e),

                PaintCmd::Extension(ext)         => {
                    let mut ctx = composition_state.paint_context(&self.glyph_atlas, &mut self.image_cache, &self.external_textures);
                    ext.paint(&mut ctx);
                }
            }
        }
    }
}
```

NetRender's responsibility surface:

1. **Native rendering for common items.** Text-shaping (via parley), glyph atlas, raster image cache, fill/stroke into vello scenes.
2. **Composition state tracking.** Clip stack, transform stack, layer stack. Vello-level primitives.
3. **Extension dispatch.** Hand the extension a paint context; the extension paints itself.
4. **Shared resource ownership.** Glyph atlas, image cache, external texture registry. Extensions read these; can register their own resources via the registry where appropriate.
5. **Generation tracking** (future). Cache rendered output keyed by `PaintList::generation_id()` for repaint-without-relayout.

NetRender knows nothing about Serval, Nematic, Scrying, or any specific engine. It knows PaintList + Vello.

### Real-world prior art

This pattern is established across rendering systems:

- **egui's `PaintCallback`** ([docs.rs/egui](https://docs.rs/egui/latest/egui/epaint/struct.PaintCallback.html)). egui processes its own paint primitives natively (rect, text, mesh, circle) and exposes `PaintCallback` for user code to paint into the same render target with whatever low-level API the backend uses. Closest direct analog to `PaintExtension`.
- **WebRender's custom display items** (Servo's prior renderer). The display list had a defined common vocabulary plus embedder-supplied item types (e.g., `NotificationRequest`) the embedder rendered. Same shape, less abstracted.
- **gpui's element painting** (Zed's framework). Element trait has `paint` method that runs against gpui's scene; native elements use built-in primitives, custom elements paint themselves.
- **PostScript / PDF content streams.** Built-in operators plus extension dictionaries for embedded objects (XObjects, forms). 1980s prior art.
- **SVG `<foreignObject>`.** SVG's escape hatch for embedded non-SVG content. Same principle: common minimum + extension escape.
- **Vello's scene API itself.** Vello *is* a scene-builder API; engines build scenes directly. Our trait wraps Vello with composition primitives + extension dispatch on top — adding common-minimum item types so multiple engines don't reinvent.

If the pattern works for egui and WebRender, it works for us. The novelty isn't risk.

### Trade-off summary

| Concern | Common-vocabulary-only (current ServalDisplayList) | Polyglot PaintList + extensions |
| --- | --- | --- |
| Renderer simplicity | Higher (one input shape) | Slightly lower (trait dispatch on iterator + extension dyn call) |
| Engine autonomy | Low (engines constrained to common vocab) | High (extensions for engine-specific items) |
| Cross-engine optimization sharing | Yes | Yes (common items + shared resources) |
| Naming honesty | Leaks Serval origin | Honest cross-engine vocabulary |
| Extension perf cost | n/a | One dyn call + one vtable dispatch per extension item (negligible if extensions aren't hot) |
| Onboarding cost for new engines | High (add items to common vocab + get Serval to agree) | Low (impl PaintList + add Extensions for engine-specific items) |
| Risk | Low (concrete known) | Medium (trait-shape choices to make right) |

The polyglot pattern's runtime cost is bounded — extension dispatch is one dyn call per extension item, and extensions are typically the minority of paint commands (most paint is text + rect + image). The architectural wins (engine autonomy, honest naming, onboarding) outweigh the small perf cost.

---

## Per-engine impl notes

### Serval

Current `ServalDisplayList` renames to `ServalPaintList`, implements `PaintList`. Items that map to common `PaintCmd` variants emit those (text, image, rect, line/stroke, ExternalTexture for canvas elements). Items that are HTML/CSS-specific become `ServalPaintExtension` variants:

- Gradients (linear, radial, conic) — each a separate extension variant.
- Complex borders (multi-corner-radius, image-borders, dashed/dotted with custom patterns).
- Stacking contexts (with their compositor effects).
- Mix-blend-mode applied to non-layer primitives.
- Masks.
- Paint worklets (CSS Houdini) — extension variant whose paint() invokes the worklet script.
- Box shadows.

Many of these may graduate to common over time (see [Graduation path](#graduation-path) below) as other engines discover they want them.

### Nematic

Initially emits common items only — `DrawText` for gemtext lines, `DrawRect` for quote-block backgrounds, `DrawStroke` for separators, `DrawImage` for inline images. NetRender renders natively. **No extension needed for most smolweb content.**

Later, if Nematic wants protocol-shaped items, those become `NematicPaintExtension` variants:

- Terminal-aesthetic blocks (monospace glyphs with cell-aligned grids, cursor markers, ANSI-color preservation).
- Gemtext-quote-with-side-rule as an atomic command (instead of composed rect+stroke+text).
- Preformatted-block with overflow indicators specific to gemtext semantics.
- ASCII-art preservation hints (specific font handling, no font fallback).

These stay Nematic-internal until/unless they prove cross-engine useful.

### Scrying

`ScryingPaintList` emits essentially one command: `DrawExternalTexture(scrying_texture)` at the viewport rect. The wgpu texture is registered out-of-band with NetRender's `ExternalTextureRegistry` (per the existing `ExternalTextureItem.texture_key` pattern from the 2026-05-15 paint refactor). NetRender composites the texture natively. No extension needed.

---

## Graduation path

Extension items that prove cross-engine-useful **graduate** to common `PaintCmd` variants:

1. Engine A adds an extension item (e.g., Serval's `LinearGradient`).
2. Engine B (e.g., a future markdown-direct lane) wants the same item.
3. PR adds `PaintCmd::DrawLinearGradient(LinearGradientItem)` as a common variant.
4. NetRender adds native handling.
5. Engine A's extension variant is deprecated; engine A updates to emit the common variant.
6. (Optional) Engine A's extension variant is removed in a subsequent release.

This keeps the common vocabulary from bloating with single-engine items while keeping the evolution path open. Items earn their common spot by proving multi-engine usefulness.

The common vocabulary should stay **small and primitive** — text, rect, stroke, image, external-texture, plus compositor primitives. CSS-specific decorations (gradients, complex shadows, etc.) should default to extensions until they prove cross-engine.

---

## Crate dep graph

```text
render_scene_api (new shared crate)
  ├── defines: PaintList, PaintCmd, PaintExtension, common item types
  └── depends on: paint-types (for image keys etc.), vello (for the Scene type referenced by PaintContext)

netrender (its own crate)
  ├── consumes: PaintList (generic)
  ├── implements: native rendering for common items, extension dispatch
  ├── owns: glyph atlas, image cache, external texture registry
  └── depends on: render_scene_api, vello, parley (for text shaping)

serval-layout (produces ServalPaintList)
  ├── ServalPaintList: impl PaintList
  ├── ServalPaintExtension variants for CSS-rich items
  └── depends on: render_scene_api

nematic (produces NematicPaintList)
  ├── NematicPaintList: impl PaintList (common items only initially)
  ├── NematicPaintExtension variants (when needed)
  └── depends on: render_scene_api

scrying lane wrapper (produces ScryingPaintList)
  ├── ScryingPaintList: impl PaintList (one ExternalTexture command)
  └── depends on: render_scene_api, scrying
```

**Dep rules:**

- Engines don't depend on each other.
- NetRender doesn't depend on any engine.
- Everyone depends on `render_scene_api`.
- `render_scene_api` depends only on the lowest-level primitives (Vello, paint-types).

Clean. Engine ↔ renderer mediation through one trait crate.

---

## What this means for existing code

The post-audit state of paint crates ([per the audit snapshot](./2026-05-16_workspace_audit_snapshot.md)):

- `components/paint/` — `servo-paint` crate, implements NetRender.
- `components/shared/paint/` — `servo-paint-api` crate, embedder-facing cross-process paint API.
- `components/shared/paint-types/` — primitives (image keys, color, units).

Under this proposal:

- **`servo-paint` (netrender)** — input signature lifts from concrete `ServalDisplayList` to generic `<L: PaintList>`. Existing native-item rendering stays; adds extension dispatch path. Likely renames to `netrender` formally (it's already structurally that, post-audit; the package name should match).
- **`servo-paint-api`** — unchanged in concern (cross-process paint API for embedder integration is a different layer; stays as is).
- **`servo-paint-types`** — primitives stay, possibly consumed by the new `render_scene_api`.
- **New `render_scene_api` crate** at `components/shared/render-scene-api/` (or `paint-list-api/` — name TBD).
- **`ServalDisplayList`** (currently in serval-layout's planned output) renames to `ServalPaintList`. Items that map to common variants stop being Serval-specific; items that don't become `ServalPaintExtension` variants.

This is a focused refactor, not a rewrite. NetRender's vello-backed rendering machinery — text-shaping, glyph atlas, image cache, scene building — all keeps its current shape. Only the input dispatch layer changes.

---

## Open questions for review

1. **Crate name.** `render_scene_api` is working name. Alternatives: `paint_list_api`, `polyglot_paint_api`, `render_api`, `render_dom_api` (paralleling layout_dom_api), `paint_command_api`. Lean `paint_list_api` — most directly names the trait. Decide at scaffold time.

2. **Scene type abstraction.** `PaintContext::scene: &mut vello::Scene` commits NetRender's *renderer backend* (Vello) into the trait surface. That couples extensions to Vello specifically — an engine that wanted to talk to wgpu directly through NetRender couldn't, because `paint()` takes a `vello::Scene`. Alternatives:
   - (a) Stay coupled to Vello. Simple. Vello is mature, well-maintained, our committed renderer backend per the 2026-05-15 audit.
   - (b) Abstract the scene type via an associated type on PaintExtension. More flexible; more complex.
   - (c) Provide multiple PaintContext variants (one for Vello, one for raw wgpu). Most flexible; most code.
   
   Lean (a) — Vello is the renderer; if we ever swap renderers, that's a much bigger refactor than this trait surface anyway.

3. **Extension lifetime model.** `&'a dyn PaintExtension` ties extensions to PaintList's lifetime. For engines that build their PaintList from a long-lived document (Serval), this is fine. For engines that produce PaintList lazily (a hypothetical streaming source), it might constrain shape. Defer; lazy PaintList isn't a v1 concern.

4. **Layer caching strategy.** `PaintCmd::PushLayer` is the hook for renderers to cache layer output (for scroll containers, opacity layers, etc.). The cache key would include `(layer_id, generation_id)`. Detailed design deferred; the v1 PaintList shape just needs the push/pop primitives present.

5. **Generation_id collision across PaintLists.** If multiple PaintLists feed a multi-lane composite (Serval page with embedded Scrying iframe), each has its own generation_id. NetRender needs to track them per-source. Probably a `(source_id, generation_id)` cache key. Defer.

6. **Extension Send + Sync requirement.** Extensions must be `Send + Sync` per the sketch. Most should be (they're owned data); could be relaxed if any common extension type wants interior mutability with `!Sync` state. Lean keep Send+Sync as the default; revisit if a concrete extension can't satisfy it.

7. **TextItem<'a> details.** TextItem needs to carry parley's shaped glyph runs (or equivalent). The borrowing model — does TextItem borrow from a shaped-text storage owned by the PaintList? — affects ergonomics. Detailed design at first impl.

---

## Review checklist

For Mark, codex, or whoever reviews next:

- [ ] Is the common-minimum vocabulary the right set (compositor primitives + text/rect/stroke/image/external-texture)? Missing items the common minimum should include: line break primitives, paragraph boxes, soft hyphens? Or are those parley/text-layout-internal? Lean parley-internal — by the time text gets to PaintList it's already a `DrawText` glyph run.
- [ ] Is the extension model (`&'a dyn PaintExtension` carrying `paint(ctx)`) the right shape, or should it be enum-discriminated (`PaintCmd::Extension(EngineId, &'a dyn EngineSpecific)`) with the engine_id at the discriminant level rather than inside the trait? Lean current shape — extensions self-identify; the discriminant pattern adds machinery without much gain.
- [ ] Is graduating extensions to common variants the right evolution strategy, or should some items just stay "engine-specific" permanently? Probably some are permanent (paint worklets are CSS-specific; no other engine will use them) — that's fine, graduation is opt-in.
- [ ] Is the runtime cost of dyn dispatch on extensions acceptable, or should we look at static dispatch via generic associated types on PaintList? Lean dyn — paint extensions should be infrequent compared to common items; perf parity not load-bearing.
- [ ] Does the proposed crate name (`render_scene_api` / `paint_list_api` / etc.) resonate? Or is there a better name that captures "polyglot renderer input vocabulary"?
- [ ] Does this design correctly support the multi-lane composition case (Serval page with embedded Scrying iframe)? Sketch: Serval's PaintList contains an `ExternalTextureItem` at the iframe location with a texture key pointing at Scrying's rendered output. Verify the texture-key routing works across lane boundaries.
- [ ] Should NetRender expose its own observable surface (rendered-frame events, paint timing, etc.) for apparatus / debugging? Separate concern; defer.

---

## Decision log

- **Decided 2026-05-17:** NetRender becomes a polyglot structured-document renderer. Input is a `PaintList` trait family, not a concrete type. Each engine produces its own concrete PaintList; NetRender consumes any impl.
- **Decided 2026-05-17:** Common-minimum vocabulary + engine-specific extensions, mirroring SemanticQuery pattern. NetRender handles common items natively; extensions paint themselves into a `PaintContext` the renderer supplies.
- **Decided 2026-05-17:** New shared crate (working name `paint_list_api` or similar) in `serval/components/shared/`. Owned by neither Serval nor Nematic; consumed by NetRender + every engine.
- **Decided 2026-05-17:** NetRender lives on its own terms in its own crate (already structurally true post-audit). Renames to `netrender` formally if not already.
- **Decided 2026-05-17:** Graduation path — extension items prove cross-engine useful → promote to common variant via PR. Keeps common vocab small + primitive without locking out evolution.
- **Decided 2026-05-17:** Vello is the committed scene type in `PaintContext` — extensions can talk to Vello directly. Renderer-backend abstraction deferred; not load-bearing for current scope.
- **Decided 2026-05-17:** Existing `ServalDisplayList` renames to `ServalPaintList` and implements PaintList. Items mapping to common variants stop being Serval-specific; items that don't become `ServalPaintExtension` variants.
- **Open:** crate name (lean `paint_list_api`); extension lifetime model details; layer caching strategy; generation_id collision across multi-source composites; TextItem borrowing model. Defer detailed design to first implementation cycle.
