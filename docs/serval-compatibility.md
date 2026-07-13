# Serval compatibility

Cambium is a consumer of published Serval seams. It does not require a Serval
source checkout.

## Current verified set

Verified on 2026-07-13:

- `serval-scripted-dom = 0.1.0`
- `layout-dom-api = 0.1.0`
- `paint_list_api = 0.1.0`
- extraction source: Serval
  `6b955ff96ed8b2912d04f7a36a85a36b401bb780`

`cargo check -p cambium -p sprigging --all-features` and the focused test wall
pass against the registry packages above.

## Temporary custom-leaf protocol

Cambium still emits Serval's current `<chisel-leaf>` element and related
attribute vocabulary. This is a compatibility protocol, not Sprigging's product
name. The next boundary stage renames the engine-owned vocabulary to neutral
`custom-leaf` terms in Serval, then updates Cambium in lockstep.

## Direction rule

Cambium may depend on Serval seam crates. Serval engine crates must remain free
of Cambium, Meristem, and Sprigging dependencies. Reference applications such as
Pelt may depend on all three.
