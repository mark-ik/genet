# Object-Fit / Object-Position Receipt

Date: 2026-06-30

## Scope

Closed the CSS Images object-fit / object-position engine lever for the local
`css/css-images` WPT reftest corpus: replaced-box sizing, intrinsic-ratio
resolution, object-fit concrete object rects, object-position offsets, and
decoded image-backed replaced content across `<img>`, `<embed>`, `<object>`,
and `<video poster>`.

This receipt covers the CSS/layout/paint engine behavior. The remaining skipped
files are script-gated runner coverage, not active object-fit reftest failures.

## Landed Behavior

- Replaced content now includes `<video>`, `<object>`, and `<embed>` alongside
  `<img>`, `<canvas>`, `<iframe>`, and host external textures.
- Image decoding now covers `<embed src>`, `<object data>`, and `<video poster>`
  in addition to `<img src>`.
- Replaced sizing resolves CSS width/height against intrinsic/default ratios.
- `contain: size` replaced elements can use `contain-intrinsic-width` and
  `contain-intrinsic-height` for sizing while preserving the real content
  intrinsic size for paint-time object-fit.
- Floated replaced elements and cleared elements take the block/float path
  instead of being folded into inline text runs.
- Paint emission applies `object-fit` and `object-position` to decoded images
  and host-composited external texture content.
- Oversized no-repeat background reference tiles preserve negative
  `background-position` offsets when clipping is required.

## Validation

Rust:

```powershell
cargo test -p genet-layout --lib
# 230 passed

cargo test -p servo-paint
# passed

cargo build -p genet-wpt
# passed
```

WPT:

```powershell
# Non-reference object-fit/object-position files in tests/wpt/tests/css/css-images,
# including tentative contain-intrinsic-size files.
ran=198 passed=182 skipped=16 failed=0
```

The 16 skipped files are the script-gated canvas and dynamic-aspect-ratio cases:

- `object-fit-*-png-*c.html`
- `object-position-png-*c.html`
- `object-fit-dyn-aspect-ratio-001.html`
- `object-fit-dyn-aspect-ratio-002.html`
- `object-fit-containcontainintrinsicsize-png-001c.tentative.html`
- `object-fit-containsize-png-001c.tentative.html`

## Status

Complete for the object-fit/object-position CSS engine lever: zero local WPT
failures remain in the object-fit/object-position corpus. Remaining skips belong
to script/canvas/dynamic runner coverage.
