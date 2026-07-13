# Livery

Livery is Genet's clean-room CSS property and cascade engine. It is generated
from a declarative property catalog and licensed under MIT or Apache-2.0.

The first lane is Cambium structural UI. Fullweb documents continue to use
Genet Stylo.

The 40-property first-lane catalog generates concrete property metadata and a
typed `ComputedValues`. The seed value layer covers the audited Cambium and UA
stylesheet values, including lengths, percentages, linear `calc()`, colors,
and the lane's keyword families. Cascade and media evaluation are the next
stage.
