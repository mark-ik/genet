# Genet Livery

`genet-livery` is Genet's integration path for the clean-room Livery CSS
engine. It adapts any `LayoutDom` to Livery's selector substrate, resolves a
concrete Livery style plane, lays the bounded Cambium lane out, and emits box
backgrounds and physical borders through the neutral `PaintList` API without
importing Stylo.

Fullweb documents continue through `genet-layout` and Genet Stylo. Runtime
document routing stays above both concrete paths.

The first layout and paint cuts cover the audited physical box subset. Inline
formatting, shaped text and glyph paint, clipping and stacking contexts, and
session-engine registration remain before Cambium can select this path in
production.
