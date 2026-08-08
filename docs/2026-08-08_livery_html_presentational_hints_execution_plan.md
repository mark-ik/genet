# Livery HTML presentational hints execution plan

**Date:** 2026-08-08

**Status:** scoped. PH0 is the first gate after contextual-color C1. This is
an F0/F3 conformance sidequest and does not enter Buckram.

**Parent:** [Livery fullweb cutover and the servo-* retirement](2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md)

**Defect source:** [Buckram K4 CSS tables execution plan](2026-07-28_buckram_k4_css_tables_execution_plan.md#the-largest-remaining-family-is-not-what-its-name-says)

**Entry dependency:** [Livery contextual color computation plan](2026-07-28_livery_contextual_color_computation_plan.md). C1 lands first because
both slices change generated color fields and the cascade representation.

## Ruling

HTML presentational attributes become declarations before computed style.
They do not become layout corrections.

The HTML adapter derives typed declarations from element attributes and feeds
them into Livery's cascade at the author presentational-hint origin. Livery
remains DOM-language neutral; it knows the origin and priority, not the names
`cellpadding`, `bgcolor`, or `align`. Buckram sees only the resulting computed
CSS values.

The immediate defect is the 40-file `table-anonymous-objects-059` through
`-098` family. Buckram's anonymous table geometry is correct. The HTML test
table carries `cellpadding="0" cellspacing="0"`; Livery ignores both and
retains the UA defaults, causing a per-column drift.

## Standards boundary

CSS Cascade Level 5 gives presentational hints a special-purpose author
presentational-hint origin between normal user and normal author declarations.
It is an independent cascade origin, not an unlayered author rule with a
small specificity. This distinction matters because otherwise cascade layers
can accidentally make hints stronger than authored CSS.

HTML's rendering rules define the mappings, including:

- table `cellspacing` to table `border-spacing`;
- table `cellpadding` to the four paddings on corresponding `td`/`th` cells;
- table, group, row, and cell dimensions to CSS dimensions;
- table-part `align` values to text alignment and descendant alignment; and
- the wider legacy color, border, spacing, and embedded-content families.

Normative anchors:

- [CSS presentational-hint origin](https://drafts.csswg.org/css-cascade-5/#preshint)
- [HTML rendering and presentational hints](https://html.spec.whatwg.org/multipage/rendering.html#the-css-user-agent-style-sheet-and-presentational-hints)
- [HTML table rendering](https://html.spec.whatwg.org/multipage/rendering.html#tables-2)

HTML labels many of these as expected default rendering rather than document
conformance requirements. Genet's standards ledger records that distinction;
it does not use it as a reason to reinterpret the mapping.

## Live defects

1. `Origin` in `components/livery/src/cascade.rs` contains only `UserAgent`,
   `User`, and `Author`.
2. `genet-livery/src/style.rs` matches stylesheet rules and inline style, but
   exposes no document-language declaration provider.
3. table attributes used by the HTML model (`rowspan`, `colspan`, `span`) are
   correctly normalized into Buckram topology, while CSS-representable
   attributes never enter cascade.
4. replaced-image `width` and `height` are applied late in
   `genet-livery/src/layout.rs` when CSS computes to `auto`. This produces a
   useful bounded result but bypasses cascade, computed style, invalidation,
   and CSSOM.
5. `cellpadding` is cross-element: one table attribute contributes
   declarations to its corresponding cells. Treating it as a rule on the
   table cannot implement the HTML mapping.

## Target contracts

The exact names can change in PH0. The ownership may not:

```rust
pub enum Origin {
    UserAgent,
    User,
    AuthorPresentationalHint,
    Author,
}

pub trait PresentationalHintProvider<Id> {
    fn declarations_for(&self, id: Id) -> PresentationalDeclarations;
}

pub struct PresentationalDeclarations {
    pub declarations: Vec<Declaration>,
    pub diagnostics: Vec<PresentationalHintDiagnostic>,
}
```

Ownership: the `Origin` variant lands in `livery`'s cascade; the provider
trait and the HTML mapping implementation land in `genet-livery`; Buckram
sees neither.

Required invariants:

1. Hints cannot be `!important`, cannot define custom properties, and cannot
   enter a cascade layer. The seam asserts this on provider output rather
   than trusting adapters; `Declaration` carries an `important` flag today.
2. Normal author declarations and style attributes override hints.
3. Hints override normal user and UA declarations as CSS Cascade requires.
4. `revert` treats the hint origin as part of author rollback;
   `revert-layer` does not expose it as a layer. If those keywords are not yet
   implemented, fixtures remain red and named.
5. Hints affect computed style but do not appear in a stylesheet's rule list
   or the element's `style` attribute.
6. Attribute mutation invalidates the element and every dependent target,
   such as the cells affected by table `cellpadding`.
7. Invalid legacy values are ignored under the HTML mapping rules and remain
   diagnosable without becoming invalid CSS declarations.

## Execution gates

| Gate | Outcome |
|---|---|
| PH0 | cascade origin and adapter contract |
| PH1 | `cellspacing` and cross-element `cellpadding` |
| PH2 | table dimensions and alignment |
| PH3 | table color, border, frame, and rules families |
| PH4 | replaced and embedded element hints; layout fallback deletion |
| PH5 | broader HTML hint census, mutation closure, and ledger update |

### PH0. Cascade contract

Add the author presentational-hint origin to Livery's priority model and a
DOM-neutral provider seam to `genet-livery` style resolution. Use typed
`Declaration` values; do not create synthetic selector strings or a hidden
stylesheet.

Fixtures establish ordering against UA, user, layered and unlayered author
rules, inline style, animation, and `!important`. A hint's source order is
only a deterministic tie-breaker inside the hint origin.

**Receipt:** pure cascade tests prove every origin ordering, and a retained
document reports a hinted computed value without adding a CSSOM rule.

**Stop:** do not spell this as `Origin::Author + Specificity(0)`. CSS Cascade
Level 5 made the hint origin separate specifically to avoid layer ambiguity.

### PH1. Table spacing and cell padding

Implement the defect that exposed the seam:

- parse `table[cellspacing]` as a non-negative integer pixel length for
  `border-spacing`;
- parse `table[cellpadding]` as a non-negative integer pixel length;
- apply that padding to corresponding `td` and `th` cells, respecting nested
  tables and the HTML table association; and
- let any authored cell padding override the hint.

The provider may precompute a table-to-cell dependency index for one style
resolution. It must not ask Buckram's generated box tree which cells belong to
the table; hints precede box generation.

**Receipts:** direct cascade fixtures, nested-table fixtures, attribute
mutation, and the full `table-anonymous-objects-059` through `-098` family.
The WPT family is credited here, not to K4.

### PH2. Dimensions and alignment

Implement the HTML table mappings for:

- `table[width]` and `table[height]`;
- `col[width]` (the mapping table names `col` only, not `colgroup`);
- row-group and row `height`;
- cell `width` and `height`; and
- applicable table-part `align` values.

`table[align]` itself maps to `float` (left/right) or centering margins, not
to text alignment. Implement it as those declarations or defer it by name;
do not fold it into the text-align family.

Use HTML's non-negative-integer, dimension, and nonzero-dimension parsing
rules rather than CSS declaration parsing. Preserve percentage dimensions as
percentages until used-value resolution.

**Removal receipt:** delete any table-specific late geometry fallback for a
mapping now represented in computed CSS.

### PH3. Table colors and borders

Cover the table families currently ignored by the lane: `bgcolor`, `border`,
`frame`, `rules`, and their affected table parts. Implement the exact HTML
rendering mappings and selector conditions; do not reduce `frame` or `rules`
to one generic border rectangle.

PH3 begins only after contextual-color C1 so a color hint produces the same
authoritative computed color type as a CSS declaration. K4g remains the owner
of collapsed-border conflict resolution. A hint supplies candidates; it does
not select winners.

**Receipt:** computed-style fixtures distinguish the hinted declaration from
the K4g border result, and focused table border/color WPT has an exact
before/after map.

### PH4. Replaced and embedded elements

Migrate the CSS-representable hint families for `img`, `input[type=image]`,
`embed`, `object`, `iframe`, `video`, and `canvas`, including applicable
`width`, `height`, aspect-ratio, `align`, `hspace`, `vspace`, `border`, and
`frameborder` mappings.

Intrinsic replaced sizing remains layout work. The attribute-derived CSS
dimensions do not. Once width/height and aspect-ratio hints reach computed
style, delete `apply_replaced_image_size`'s direct attribute override and
retain only intrinsic sizing against those computed inputs.

**Receipt:** computed CSS, layout geometry, intrinsic aspect ratio, attribute
mutation, and authored override all agree through one style path.

### PH5. Census and closure

Build a checked-in census from the HTML rendering section, grouped by:

- implemented property and adapter;
- known Livery property gap;
- unsupported element or browsing-context capability; and
- deliberate UA-origin default rather than author hint.

Cover the remaining low-cost mappings only when their target property already
has a real consumer. Route forms, media, browsing-context, and quirks-only
behavior to their owning plans instead of adding dormant declarations.

Run absolute conformance and Stylo-differential ledgers separately. A family
that matches Stylo but remains wrong against HTML is not a closure receipt.

## Verification ladder

Every behavior gate runs:

```powershell
cargo test -p livery --offline
cargo test -p genet-livery --all-targets --offline
cargo clippy -p livery -p genet-livery --no-deps --offline -- -D warnings
cargo build -p genet-wpt --release --all-features --offline
rustfmt --edition 2024 --check <touched Rust files>
git diff --check
```

Focused WPT maps are stored outside Git under a gate-specific
`testing/genet/wpt-ledger` directory. Each receipt reports absolute Livery
movement and Stylo differential movement separately.

## Stop rules

- Stop if an HTML attribute is read inside Buckram.
- Stop if a CSS-representable hint is applied after computed style.
- Stop if hints are modeled as ordinary unlayered author rules.
- Stop if `cellpadding` is inherited from the table instead of attributed to
  its corresponding cells.
- Stop if invalid HTML values are fed to the CSS parser and accepted under
  different grammar.
- Stop if presentational hints appear as authored CSSOM rules or inline style.
- Stop if PH3 duplicates K4g conflict resolution.
- Stop if a differential gain is called HTML conformance without the absolute
  receipt.

## Done condition

The plan closes when every HTML presentational hint applicable to Genet's
implemented static/fullweb elements is either represented at the correct
cascade origin or assigned to a named property, element, quirks, or
browsing-context gap. The table-anonymous family passes for the attributed
reason, direct layout-side attribute overrides are deleted where computed CSS
now owns them, and mutation/CSSOM receipts prove one authority.
