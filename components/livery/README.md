# Livery

Livery is Genet's clean-room CSS property and cascade engine. It is generated
from a declarative property catalog and licensed under MIT or Apache-2.0.

The first lane is Cambium structural UI. Fullweb documents continue to use
Genet Stylo.

The 87-property native lane catalog generates concrete property metadata and a
typed `ComputedValues`. The current ratchet adds box geometry (`right`,
`bottom`, min/max sizing, `box-sizing`, and `aspect-ratio`), corner radii,
visibility and pointer-event state, text alignment and spacing, box shadows,
two-stop linear-gradient and raster `data:` backgrounds, bounded opacity and
background-color transition metadata (including simultaneous `all` sampling),
a bounded `@keyframes` opacity animation with linear and named easing
functions, intrinsic image tiling with bounded position/repeat modes, flexbox,
and a bounded grid track/placement family.
The seed value layer
covers the audited Cambium and UA
stylesheet values, including lengths, percentages, linear `calc()`, colors,
and the lane's keyword families. The bounded E2 resolver adds declaration and
shorthand parsing, selector matching, cascade ordering and inheritance, and
media evaluation on a Genet-shaped `Device`. Cambium lane integration lives in
the separate `genet-livery` crate: a `LayoutDom` adapter, concrete style plane,
and standalone Taffy box path with neutral paint emission.
