# Genet Livery

`genet-livery` is Genet's integration path for the clean-room Livery CSS
engine. It adapts any `LayoutDom` to Livery's selector substrate, resolves a
concrete Livery style plane, lays the bounded Cambium lane out, and emits box
backgrounds, physical borders, and independently shaped text nodes through the
neutral `PaintList` API without importing Stylo. Text shaping uses the
MIT/Apache Parley crate directly; the MPL `netrender_text` adapter is not part
of this path.

Fullweb documents continue through `genet-layout` and Genet Stylo. Runtime
document routing stays above both concrete paths.

The first layout and paint cuts cover the audited physical box subset and emit
glyph-bearing text runs with a self-contained font side table. A shared inline
formatting context across text and inline elements, clipping and stacking
contexts, retained font/shaping ownership, and session-engine registration
remain before Cambium can select this path in production.
