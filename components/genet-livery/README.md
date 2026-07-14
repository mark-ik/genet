# Genet Livery

`genet-livery` is Genet's integration path for the clean-room Livery CSS
engine. It adapts any `LayoutDom` to Livery's selector substrate, resolves a
concrete Livery style plane, lays the bounded Cambium lane out, and emits box
backgrounds, physical borders, and shared inline text runs through the neutral
`PaintList` API without importing Stylo. Text shaping uses the
MIT/Apache Parley crate directly; the MPL `netrender_text` adapter is not part
of this path.

Fullweb documents continue through `genet-layout` and Genet Stylo. Runtime
document routing stays above both concrete paths.

The retained `LiveryDocument` owns Parley's font database, shaping scratch
space, stable font resources, and a cached paint frame. Consecutive text and
inline-element children shape together, sharing line breaks, baselines, style
spans, and collapsed whitespace. Parley's positioned output also supplies the
paint geometry for inline elements: wrapped spans receive one fragment per
line, and `inline-block` children occupy atomic space in that line.
`genet-documents` can register this path as the opt-in `genet.livery` static
session rung.

Feeding shaped line height back into Taffy's parent block flow, inline box-edge
decoration, bidi paint ordering, clipping, stacking contexts, links, scrolling,
and focus remain open. `genet.livery` therefore stays an explicit pin rather
than the default static route.
