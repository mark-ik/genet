# Genet compatibility

Cambium's public core consumes Genet through registry seam packages. The
unpublished `cambium-nematic` retained-document adapter still resolves its
optional layout and render providers from a sibling Genet checkout.

## Current verified set

Verified on 2026-07-14:

- `genet-scripted-dom = 0.1.0`
- `layout-dom-api = 0.1.0`
- optional document adapter: `errand = 0.1.3`, `genet-layout = 0.2.0`,
  and `genet-render = 0.2.0`
- core provider release commit:
  `2e462fe8975`

| Package | Version | Current source |
| --- | --- | --- |
| `layout-dom-api` | 0.1.0 | crates.io and sibling path |
| `errand` | 0.1.3 | crates.io and sibling path |
| `genet-paint-types` | 0.1.0 | crates.io |
| `engine-observables-api` | 0.1.1 | crates.io |
| `genet-static-dom` | 0.1.0 | crates.io |
| `genet-scripted-dom` | 0.1.0 | crates.io and sibling path |
| `genet-layout` | 0.2.0 | optional sibling path; publication pending |
| `genet-render` | 0.2.0 | optional sibling path; publication pending |

The public Cambium stack is `meristem 0.1.0`, `sprigging 0.1.0`,
`cambium 0.1.0`, and `cambium-winit 0.1.0`. `cambium-nematic` remains a local
adapter until the two broad engine providers above have a deliberate release
boundary.

## Custom-leaf protocol

Cambium emits Genet's neutral `<custom-leaf>` element and related attribute
vocabulary. Genet temporarily accepts `<chisel-leaf>` as a read-side
compatibility alias for older documents.

## Direction rule

Cambium may depend on Genet seam crates. Genet engine crates must remain free
of Cambium, Meristem, and Sprigging dependencies. Reference applications such as
Pelt may depend on all three.
