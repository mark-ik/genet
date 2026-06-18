# Pelt follow-ups (spun out at archive)

**Date:** 2026-06-18. **Parent (archived):**
`archive/2026-06-12_pelt_development_plan.md`. **Why:** the pelt plan reached
all-phases-done (V0–V6) and was archived; this carries its residual open points
so they stay live. Most are small or owned by another plan (cross-referenced);
none blocks pelt's reference-shell or embedded-surface roles.

## Owned elsewhere (pointer only)

- **External `<script src>`** (pelt V4 is inline-only by design). This is
  residual-scripted-tier item #1 in
  `2026-06-16_element_view_and_scripted_tier_plan.md` (the real home); it is the
  most common reason a real scripted page does nothing today.
- **V6 host-side: forme-canonical authority inversion** (meerkat keeps the
  `Pane` tree canonical, forme + `TreeGeometry` persistence-only) and **routing
  meerkat tiles through pelt's in-surface `Document` lane** (today every tile is
  an `ExternalTexture` actor-texture). Both are mere window-composition work, not
  pelt-engine work.

## Pelt-specific residuals

1. **The pane-module contract write-up.** V6 is the second instance of the
   orrery-host pattern; the generalizable standalone-or-hosted surface contract
   (frame / input / resize / content-source) is still unwritten, and
   roster/gloss/apparatus want the same shape under the window-composition pane
   model. The highest-value residual: it turns an ad-hoc pattern into a reusable
   one. Likely lives near `pelt-core` (the contract leaf) or a host-shared doc.
2. **Profile honesty** (open-Q#3). `EngineProfile`'s capabilities printout still
   predates the wiring (e.g. headless prints `webgpu=false` etc. from a static
   table); derive the flags from what each profile actually wires.
3. **PNG-lane maturity.** The `png-reftest` lane ships render + `--out *.png` +
   fuzz-thresholded optional `name.png` compare, but no `name.png` fixtures are
   committed (GPU-jitter across machines). If a deterministic raster case is
   wanted, commit one with a `name.fuzz` sidecar. Also: a `--size` override for
   the reftest viewport (today fixed at 800x600).
4. **C1 query-object adoption** (open-Q#2). When mere's C1 laid-out-document
   query object lands, adopt it in pelt so the reference shell demonstrates the
   cheap path, not pelt-live's free functions.
5. **`static_viewer` scaffold fate** (open-Q#4, pelt-desktop). Fold into V1's
   viewer or keep as the smoke-shaped probe; decide when next touching it.
