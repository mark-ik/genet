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

Retained sessions first use Taffy to resolve atomic `inline-block` sizes, then
represent each consecutive inline group as one Parley-measured Taffy leaf. The
same shared line layout therefore drives paint geometry and parent block
height, including mixed text, styled spans, and atomic boxes. Inline padding
and border edges enter Parley as zero-height atoms, consume horizontal advance,
paint vertical overflow without changing line height, and use default slice
semantics across wrapped fragments. Parley's visual item order is retained as
one paint stream per inline group, including bidi runs, while the group's
decorations paint first. Block containers with non-visible overflow emit
axis-aware padding-box clips around their descendants; nested clips balance and
intersect through the neutral paint-list stack. Within each numeric or document
stacking context, positioned block and inline-level elements with numeric
`z-index` paint in stable negative, normal-flow, then nonnegative order while
each reordered subtree stays atomic. Numeric stacking roots flatten through
intervening ancestors into their nearest context, replaying any ancestor
overflow clips around the extracted subtree. Shaped inline commands retain
their ownership so positioned spans and atomic inline-blocks reorder without
losing shared line geometry or bidi visual order. Context triggers such as
opacity and transforms remain open, along with links, scrolling, and focus.
`genet.livery` therefore stays an explicit pin rather than the default static
route.
