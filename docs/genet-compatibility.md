# Genet compatibility

Cambium consumes Genet through versioned seam packages. During extraction,
unpublished provider packages still resolve from a sibling Genet checkout.

## Current verified set

Verified on 2026-07-14:

- `genet-scripted-dom = 0.1.0`
- `layout-dom-api = 0.1.0`
- optional document adapter: `errand = 0.1.3`, `genet-layout = 0.2.0`,
  and `genet-render = 0.2.0`
- verified provider revision:
  `d1f31bf6ad171ee89d35ff91ff95494e020f0332`

| Package | Version | Current source |
| --- | --- | --- |
| `layout-dom-api` | 0.1.0 | crates.io and sibling path |
| `errand` | 0.1.3 | crates.io and sibling path |
| `genet-scripted-dom` | 0.1.0 | sibling path; publication pending |
| `genet-layout` | 0.2.0 | optional sibling path; publication pending |
| `genet-render` | 0.2.0 | optional sibling path; publication pending |

An exact Git pin to the full Genet repository was rejected after a bounded
clean-source check spent five minutes in Cargo resolution without reaching
compilation. C5 remains partial until the unpublished seams have a narrow
provider source or registry releases.

## Custom-leaf protocol

Cambium emits Genet's neutral `<custom-leaf>` element and related attribute
vocabulary. Genet temporarily accepts `<chisel-leaf>` as a read-side
compatibility alias for older documents.

## Direction rule

Cambium may depend on Genet seam crates. Genet engine crates must remain free
of Cambium, Meristem, and Sprigging dependencies. Reference applications such as
Pelt may depend on all three.
