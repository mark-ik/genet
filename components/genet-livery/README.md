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
session rung. Flex and grid containers lower their bounded direction, track,
gap, alignment, order, and placement values into Taffy. Box shadows lower into
the neutral shadow primitive before backgrounds.

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
losing shared line geometry or bidi visual order. Opacity values below one
establish an atomic level-zero context and wrap the whole subtree in a neutral
compositing layer. Non-`none` transforms on block and inline-block boxes also
establish an atomic level-zero context and emit a neutral coordinate-space
scope around the subtree. The first transform grammar supports 2D translate,
scale, and rotate functions composed around the default center origin; ordinary
inline spans remain non-transformable. The current property ratchet also
lowers inset and min/max geometry into Taffy, carries corner radii through
neutral border commands, applies Parley text alignment and spacing, and keeps
hidden boxes in layout while suppressing their paint. The retained document
now supplies bounded viewport scrolling, pointer-events-aware hit testing,
retained link rectangles, fragment scrolling, and focus-state routing. A
host-driven opacity clock can invalidate and repaint intermediate frames through
the same retained session. Bounded `transition-property`/`transition-duration`
metadata also starts opacity, background-color, text-color, and border-top-color
transitions from that clock; `transition: all` and explicit two- and
three-property lists interpolate those paint values together on the same clock.
Two-stop
linear-gradient backgrounds paint as an ordered neutral layer over the color
fill and share the rounded clip. Raster `data:` background URLs now lower into
the neutral image side-table and stretch to the element box. Nested scroll
containers route wheel deltas into their retained offsets, chain at their
boundaries to the viewport, and replay descendant paint through transforms.
Bounded `@keyframes` declarations animate opacity through the same retained
clock, with linear, ease, ease-in, ease-out, and ease-in-out timing functions.
Host-resolved local image bytes use the same neutral image side-table, with
bounded intrinsic tiling and position/repeat modes. Replaced `<img>` elements
now use intrinsic data/local dimensions, preserve their aspect ratio under a
bounded CSS width or height, and paint through the neutral image side-table.
The session consumes host-supplied image bytes, including remote URLs resolved
by `genet-documents`; fetch policy and caching remain outside the engine.
`genet.livery` therefore stays an explicit pin rather than the default static
route.
