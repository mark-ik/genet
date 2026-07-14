# Livery

Livery is Genet's clean-room CSS property and cascade engine. It is generated
from a declarative property catalog and licensed under MIT or Apache-2.0.

The first lane is Cambium structural UI. Fullweb documents continue to use
Genet Stylo.

The 40-property first-lane catalog generates concrete property metadata and a
typed `ComputedValues`. The seed value layer covers the audited Cambium and UA
stylesheet values, including lengths, percentages, linear `calc()`, colors,
and the lane's keyword families. The bounded E2 resolver adds declaration and
shorthand parsing, selector matching, cascade ordering and inheritance, and
media evaluation on a Genet-shaped `Device`. Cambium lane integration is the
next stage. Its first slice lives in the separate `genet-livery` crate: a
`LayoutDom` adapter, concrete style plane, and standalone Taffy box path.
