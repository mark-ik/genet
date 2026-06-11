# Pseudo-element follow-ups: scope (post ::before/::after + :checked)

**Status (2026-06-11):** scoping, for review. Sibling to the
[layout infrastructure scope](2026-06-07_serval_layout_infrastructure_scope.md)
(§4 of which this completes and supersedes for pseudo-elements). The tractable
serval-layout slice shipped this round: inline `::before`/`::after` generated
content (`0694833a3ed`, via `push_pseudo_content`, construct.rs:263-265) and
`:checked` (`e1fb3680db9`) — the cascade resolves eager pseudos for free, so
the content slice was small. The five remainders each hit a verified wall and
are scoped here as their own sub-projects, in value-over-effort order. (No
boa gate: **pelt-live builds** — it has no script-engine dependency, only
`serval-wpt` pulls the boa fork's WIP — so the host-side wiring halves land
now alongside their serval-layout halves. Earlier drafts of this doc gated §1
on a pelt-live build failure that was actually serval-wpt's; corrected
2026-06-11 against `cargo build -p pelt-live`.)

Verified grounding for all five: the pinned stylo's servo-mode eager set is
**4** (`EAGER_PSEUDO_COUNT = 4` — After / Before / FirstLetter / Selection;
`stylo @ 572ecba`, `style/servo/selector_parser.rs:130`), so `::marker` is
lazy and `::first-line` does not exist in the servo `PseudoElement` enum at
all. `selection_rects` (single-node) already exists in serval-layout and
pelt-live already paints it with a hardcoded color
(`pelt-live/render.rs:42,159`).

---

## 1. `::selection` colors (small; fully landable now)

**Unblocks:** themed selection highlights; removes the
`SELECTION_COLOR` hardcode.

**Current state:** Selection is eager and readable. The highlight paint path
exists end to end (`selection_rects` → `push_selection`); only the color is
hardcoded (`render.rs:42`).

**Approach:** a serval-layout read-back helper (`selection_style(styles,
node) -> Option<(bg, fg)>`, nearest ancestor with a `::selection` slot);
pelt-live replaces the constant with the resolved background. The
**foreground** half (recoloring selected glyphs) needs per-range glyph
recolor and rides §5's range work — ship background-only first.

**Touch-points:** serval-layout (helper + unit test); `pelt-live/render.rs`
(one constant replaced); meerkat chrome inherits via the same `TextCursor`
path for free.

**Effort / risk:** Small. The ~15-line helper plus a one-constant pelt-live
swap; no gate.

**Done when:** a focused field's highlight uses `::selection {
background-color }` when present and the theme default otherwise, in
pelt-live and meerkat chrome.

## 2. `::marker` styling (lazy pseudo resolution)

**Unblocks:** `li::marker { color/font-* }`; properly closes the
infra-scope §4 "migrate the marker" deferral.

**Current blocker (verified):** Marker is a *lazy* pseudo in this stylo;
reading it from the eager map panics.

**Approach:** on-demand resolution through the persistent Stylist
(`lazily_compute_pseudo_element_style`-shaped, parent = the `li`'s primary
style, inside the TLS cascade guard), cached in a lazy-slot map on
`StylePlane` beside the eager slots and invalidated with the element's
restyle. Apply the resolved style in the existing, well-factored marker
surface: `list_marker_content` / `list_marker_inline_run`
(construct.rs:824-869) styles its run from the `::marker` style when present
instead of li-style-with-cleared-decoration; `marker_kind` reads
`list-style-type` from it.

**Touch-points:** cascade.rs (lazy-resolve helper on both the session and
stateless paths), style.rs / StylePlane (lazy storage), construct.rs (the
marker functions).

**Effort / risk:** Medium-small. The marker machinery is already factored;
the new piece is the lazy-resolution plumbing, which the persistent Stylist
makes sound (same soundness story as the inline-style replacement path).

**Done when:** `li::marker { color: red }` recolors bullets and ordinals in
a reftest, and absent `::marker` rules behave exactly as today.

## 3. Selection-range geometry (DOM ranges; design it as cheap-path C1 API)

**Unblocks:** content selection read-back (`ServalLaneView::selection`
rects), multi-node highlight, and §1's foreground half.

**Current state:** the "large split-out" shrank: single-node
`selection_rects` shipped and is painted. The remaining gap is **multi-node
ranges** (anchor/focus across elements).

**Approach:** a DOM-range walker over the box tree: order (anchor, focus) in
tree order, and for each text leaf intersecting the range, collect per-line
rects via its cached parley layout (the same machinery the single-node
helper uses), unioned. Expose as `range_rects(range) -> Vec<Rect>` **on the
cheap-path C1 laid-out-document query object**, not a free function — C1 is
exactly the retained-artifacts query home (mere's host cheap-path plan), and
this becomes its first new query.

**Touch-points:** serval-layout (range walk), the C1 query seam (pelt-live),
serval_lane (selection read-back).

**Effort / risk:** Medium. Range ordering across block boundaries is the
fiddly part; the per-leaf rect math exists.

**Done when:** an (anchor, focus) range spanning two paragraphs yields
correct per-line rects through the C1 seam, single-field behavior unchanged.

## 4. `::first-letter` (and the `::first-line` verdict)

**Unblocks:** first-letter styling (size/color/weight; the classic
typographic opener).

**Current blocker (verified):** FirstLetter is eager and readable; the work
is splitting the block's first formatted text at the first-letter boundary.

**Approach:** at InlineContent gathering (construct), when the block's first
text run exists and a FirstLetter style is present, split that run at the
CSS first-letter boundary (first letter cluster plus attached leading
punctuation) into its own `InlineRun` carrying the pseudo style. Run
injection at construct is proven (it is how `push_pseudo_content` works);
shaping and paint already handle per-run style. **Out of scope v1:**
`float`ed first-letter (drop caps) — that is block-ish layout, document the
limit. **`::first-line`: deferred indefinitely** — it is not in the servo
`PseudoElement` enum, so it would require patching stylo itself (a fork we
deliberately do not carry), for per-line restyle machinery with near-zero
value in serval's lanes.

**Touch-points:** construct.rs (split helper + boundary rules),
text_measure (verify run-boundary mapping), paint (no change expected).

**Effort / risk:** Medium. The boundary rules (punctuation classes,
combining marks) are where the edge cases live.

**Done when:** `p::first-letter { font-size: 2em; color: … }` renders the
styled first letter inline, punctuation handled per spec, in a reftest.

## 5. Block-display generated content (the structural one)

**Unblocks:** `::before`/`::after` with `display: block` (and eventually
`list-style-image` via a block-ish marker), i.e. generated content that
participates in block layout.

**Current blocker (verified):** the box tree styles boxes by DOM id
(`TaffyStyloStyle` reads `StylePlane[dom_id]`); a block pseudo needs a
synthetic box whose style comes from a pseudo slot, not a DOM node.

**Approach:** a style *source* on the box node —
`StyleSource::Element(NodeId) | Pseudo(NodeId, PseudoKind)` — resolved by
the Taffy adapter; construct synthesizes block children for Before/After
when the pseudo's computed display is block-level. Pseudo boxes take no
`FragmentPlane` identity of their own (not script-visible): paint walks the
box tree so painting is natural, and hit-testing skips them. Incremental
note: pseudo boxes are construct-time derivatives of their element's style,
so they regenerate with the element's restyle and add no new `Spliced`
semantics.

**Touch-points:** box_tree.rs (BoxNode style source + construction),
construct.rs (`push_pseudo_content` branches on computed display),
adapter / TaffyStyloStyle, paint_emit (style lookups via source),
serval_lane (skip pseudo boxes).

**Effort / risk:** Large — the one genuine architectural change here, as the
infra scope predicted. Land it in slices: the `StyleSource` plumbing first
(behavior-identical), then block Before/After.

**Done when:** `p::before { content: "x"; display: block; padding: … }`
lays out and paints as a block child; inline behavior unchanged; hit-tests
never return a pseudo.

---

## Recommended order (value over effort)

§1 selection-bg (helper now, wiring when boa builds) → §2 `::marker` →
§3 range geometry (with C1) → §4 first-letter → §5 block generated content
(when a content lane pulls on it). `::first-line` stays deferred
indefinitely with its rationale recorded in §4.
