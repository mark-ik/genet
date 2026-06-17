# Element `filter` plan (CSS `filter` on an element's own rendering)

**Date:** 2026-06-17. **Parent:** `2026-06-16_real_web_layout_fidelity_plan.md`
item 7. **Scope:** CSS `filter` applied to an element (blur, the color ops,
later drop-shadow) — distinct from `backdrop-filter` (which filters content
*behind* a layer; D1 machinery, separate).

## Why it is not a wire-up (verified)

- `paint_list_api::LayerSpec.filters: Vec<FilterOp>` already carries the full
  chain (`Blur`, `Brightness`, `Contrast`, `Grayscale`, `HueRotate`, `Invert`,
  `Opacity`, `Saturate`, `Sepia`), but the translator (`paint_list_render`
  `emit_push_layer`, ~872) applies **only `Opacity`** (as layer alpha); the rest
  are dropped.
- `netrender` `SceneFilter` has only `Blur`. `SceneLayer.backdrop_filter`
  filters the *backdrop* under the layer's clip (D1) — not the layer's own
  output. There is **no element-output filter path**, and `blur_pass_callback`
  (`filter.rs`) currently has no caller (D1 appears unwired; the working blur is
  the box-shadow mask path).
- Correctness rules out a per-op color shortcut: `filter` is *post-rasterization*
  (images, gradients, overlapping content, opacity interactions), so it must run
  on the element's composited pixels — an offscreen pass.

## Architecture (mirrors the box-shadow-mask side-list)

The proven GPU pattern: the translator emits a side-list of requests; the
painter pre-builds textures via the `RenderGraph` (topo-sorted encode callbacks)
and `ImageCache::insert_gpu`; the main scene composites them by image key
(`box_shadow_masks: Vec<BoxShadowMaskRequest>` in `paint_list_render`, built by
`Renderer::build_box_shadow_mask`).

Element filter is the same shape, but the request carries the layer's **content**
(a sub-scene of the ops between `PushLayer{filters}` and its `PopLayer`), not
shape params:

1. **Translator split.** On `PushLayer` with a non-empty output-filter chain,
   record the inner op range as a sub-scene and emit a `FilterLayerRequest {
   sub_scene, filters, content_bounds, dest }`. Replace the layer in the main
   scene with a `DrawImage(filter_result_key)` at `dest`.
2. **Painter pre-pass.** For each request: `scene_to_vello` the sub-scene →
   `render_to_texture` into a scratch target sized to `content_bounds` (+ blur
   padding for `Blur`) → apply the filter chain (RenderGraph: a color-matrix
   fragment pass per color op; the separable 2-pass `brush_blur` for `Blur`) →
   `ImageCache::insert_gpu` under `filter_result_key`.
3. **Composite.** The main scene's `DrawImage` draws the filtered texture where
   the layer was (carrying the layer's alpha/blend/clip).

## Increments

1. **Data-path foundation (this session).** `SceneFilter` gains the color
   variants; `SceneLayer` gains `filters: Vec<SceneFilter>` (hashed in
   `hash_push_layer`); the translator maps `LayerSpec.filters` → `SceneLayer
   .filters` (beyond `Opacity`); serval `paint_emit` reads
   `cv.get_effects().filter` and opens the stacking layer for a filter chain
   (alongside opacity / blend). The rasterizer ignores `SceneLayer.filters` for
   now, so render is unchanged (an alpha-1 normal layer is a visual no-op) — the
   data flows end to end, de-risking the GPU pass.
2. **Color filters (rasterizer pass).** Smaller than blur: no bounds change.
   Sub-scene → offscreen texture → one color-matrix pass per op → substitute.
   `grayscale` / `invert` make the clearest e2e tests. New WGSL: a 4x5
   color-matrix fragment shader (mirror `clip_rectangle_callback`).
3. **`blur()`.** Adds bounds expansion (pad the scratch by the blur radius) and
   reuses `brush_blur` (separable H then V). e2e: a hard-edged box blurs to a
   soft edge.
4. **`drop-shadow()`.** Like box-shadow but follows the element's alpha; its own
   increment (a shadow of the offscreen alpha, offset + blurred + colored).

## Turnkey recipe for the rasterizer pass (mirror the backdrop loop)

The backdrop-filter pass in `renderer/mod.rs` (~690-790) is the exact template;
element filter is the same loop with two changes (render the layer's *content*,
and *replace* it rather than inject after it):

| step | backdrop (existing) | element filter (new) |
|---|---|---|
| collect | `PushLayer` with `backdrop_filter` → `(i, filter, bounds)` | `PushLayer` with non-empty `filters` → `(push_i, pop_i, filters, bounds)` |
| sub-scene | `build_prefix_scene(scene, i)` (ops `[0..i)`, balanced) | `build_layer_content_scene(scene, push_i, pop_i)` (ops `(push_i..pop_i)`) — new, mirror `build_prefix_scene` |
| render | `render_scene_to_texture(rast, tc, &sub)` | same |
| filter | `build_blurred_image(tex, w, r)` | per op: `build_blurred_image` for `Blur`; `build_color_matrix_image(tex, m)` for color ops (new) |
| register | `register_texture(key, tex)` | same |
| splice | **insert** `SceneOp::Image` after the `PushLayer` | **replace** ops `[push_i..=pop_i]` with one `SceneOp::Image` (suppress the original content so it doesn't double-render) |

New artifacts: `build_layer_content_scene`, `build_color_matrix_image` +
`cs_color_matrix.wgsl` (one fragment pass sampling the input texture and applying
a 4x5 matrix uniform — mirror `clip_rectangle_callback` / `cs_clip_rectangle`),
and a `scene_filter_to_matrix(SceneFilter) -> [f32; 20]` using the CSS Filter
Effects §formulas (grayscale/sepia/saturate/hue-rotate are linear-RGB matrices;
brightness/contrast/invert are simple per-channel). Compose a chain of color ops
into one matrix; keep `Blur` as a separate spatial pass.

`bounds` for element filter is the layer's content AABB (use the layer clip rect
when present, else the union of the inner ops' bounds; pad by the blur radius for
`Blur`). The injected image carries the layer's `alpha`/`blend_mode`.

## Seams (file:line)

- Translator output-filter handling: `paint_list_render/src/lib.rs`
  `emit_push_layer` (~847), filter loop (~872).
- Side-list precedent: `box_shadow_masks` (`paint_list_render` ~414, ~772);
  `RenderPlan`-style struct (~94).
- Offscreen render primitive: `vello_tile_rasterizer.rs` `render_to_texture`
  (422) + `scene_to_vello_with_overrides` (460) + `render_overlay_fragment` (450)
  as the "render a scene to a scratch target" template.
- GPU sub-graph: `render_graph.rs` (`Task`/`EncodeCallback`/`execute`),
  `filter.rs` (`blur_pass_callback` 104, `clip_rectangle_callback` 42 as the
  single-pass shader template), `ImageCache::insert_gpu`.
- serval emission: `serval-layout/paint_emit.rs` stacking-layer push/pop
  (the opacity + mix-blend-mode site), `cv.get_effects().filter`.
