# Livery contextual color computation plan

**Date:** 2026-07-28  
**Status:** corrective F0 subplan. C0 is complete; C1 is next.

**Parent:** `2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md`

This seam must close before K4g, whose collapsed-border paint needs a resolved
contextual winner for `currentcolor` border colors.

## Why this is a separate seam

The modern color parser and retained `SpecifiedColor` syntax are real, but
they do not yet form Livery's computed color model. Generated declarations
still parse color-bearing properties directly into the eager, `Copy`
`Color` leaf. That leaf can represent an absolute color, bare
`currentcolor`, or a system-color keyword, but it cannot retain a function
whose result depends on either one.

This makes the current behavior structurally wrong in two directions:

- `color-mix()`, relative colors, `alpha()`, `contrast-color()`, and
  `color-layers()` reject valid `currentcolor` operands before cascade.
- system colors resolve through one built-in light palette without the
  element's used `color-scheme`.

The CSS model requires contextual expressions to survive until their
specified dependencies are known. Extending the eager leaf parser or adding
hidden per-property sidecars would create two authorities for one value.
Inheritance, generated copies, CSS-wide keywords, animation, CSSOM, and paint
would then have to keep an absolute cache and a retained tree synchronized.
This plan instead gives every color-bearing property one authoritative
computed expression.

Normative anchors:

- CSS Color 4, [resolving other colors](https://drafts.csswg.org/css-color-4/#resolving-other-colors)
- CSS Color 5, [`color-mix()`](https://drafts.csswg.org/css-color-5/#color-mix)
  and [contextual resolution](https://drafts.csswg.org/css-color-5/#resolving-color-mix)
- CSS Color 6, [resolving `color-layers()`](https://drafts.csswg.org/css-color-6/#resolving-layers)
- CSS Color Adjustment, [element `color-scheme`](https://drafts.csswg.org/css-color-adjust-1/#color-scheme-prop)

## Owning model

Keep `Color` as the resolved numeric leaf used by color algorithms and paint.
Generalize the retained syntax into a shared expression shape:

```text
ColorExpression<Leaf>
  Absolute(Color)
  CurrentColor
  System(SystemColor)
  Relative(...)
  Mix(...)
  Alpha(...)
  Contrast(...)
  Layers(...)
```

`SpecifiedColor` is the parsed form. `ComputedColor` is the sole value stored
by computed color-bearing properties. It contains computed children, retains
`CurrentColor` where inheritance requires it, and contains no unresolved
system-color leaf.

The boundary APIs are:

```text
SpecifiedColor::to_computed(ColorComputeContext, ColorRole) -> ComputedColor
ComputedColor::resolve_used(UsedColorContext) -> Color
ComputedColor::to_computed_css(UsedColorContext) -> String
```

`ColorRole::Foreground` is special. A `currentcolor` expression used by the
`color` property resolves against the inherited used foreground before the
new foreground is inherited. Other roles retain `CurrentColor` in the
computed expression so an inherited background, border, decoration, shadow,
or gradient re-resolves against the descendant's foreground.

## C0: correct the contract

**Status:** complete on `main` at the merge of `codex/livery-color-functions`.

- Replace tests and plan language that describe rejecting contextual
  functions as correct.
- Add declaration-level red tests for all five contextual families. A
  `SpecifiedColor` CSSOM round trip is not a cascade receipt.
- Pin the absolute-function result counts from the 2026-07-28 color slice as
  the non-regression baseline while contextual tests remain red.

### Measured state, 2026-07-31

The two layers disagree, which is the whole seam in one observation.

`tests/color.rs` shows the specified parser retaining contextual expressions
correctly: `alpha(from currentcolor / 0.5)` round trips and `resolved()`
returns `None`. But every one of the five families is rejected one layer
lower, at the declaration parser:

```text
DeclarationError { name: "background-color",
  value: "color-mix(in srgb, currentcolor 100%, white)", kind: InvalidValue }
```

So the retained syntax is real and unreachable. Any receipt drawn from
`SpecifiedColor` serialization overstates what the cascade actually accepts.
This is the specific trap C0 exists to close, and reading the round-trip tests
alone is enough to fall into it.

### Receipt

- `components/livery/tests/contextual_color.rs`: six declaration-level and
  cascade-level tests covering `color-mix()`, relative colors, `alpha()`,
  `contrast-color()`, and `color-layers()`. All six are `#[ignore]`d with the
  sub-gate that unblocks each, so the wall stays green and the seam stays
  visible. Run them with `--include-ignored`.
- Non-regression baseline: `cargo test -p livery --test color` is 55 passed,
  0 failed. `cargo test -p livery` is 0 failed across all targets.

### Correction to this gate's done-condition

C0 was written with the done-condition "valid contextual declarations enter the
cascade without being discarded or replaced by black." That is C1's outcome,
not C0's: no amount of contract correction makes a declaration parse while the
computed property is still an eager `Color`. C0's own work is the contract and
the red tests, and that is what is complete. The original sentence now serves
as C1's acceptance condition.

## C1: one authoritative computed color

Ownership:

- `components/livery/src/values/color/{specified,mod,relative,mix,alpha,contrast,layers}.rs`
- `components/livery/build.rs`
- `components/livery/src/cascade.rs`
- `components/livery/src/values/property.rs`

Work:

- Generalize the retained tree into specified and computed expressions with
  recursive computation and used-value resolution.
- Remove parse-time palette lookup and context-free
  `SystemColor::used_srgb()`.
- Make generated color fields non-`Copy`; generated `for_child`, get, set,
  and CSS-wide-keyword paths clone the one authoritative value.
- Parse declarations and `var()` substitutions as `SpecifiedColor`, not
  eager `Color`.
- Migrate nested color owners in the same slice: gradients in
  `BackgroundImage`, `BoxShadow`, and text-decoration aliases. Record an
  explicit scope knockout for any additional owner rather than silently
  routing it through eager `Color`.

Done when direct declarations, `var()`, `inherit`, `unset`, generated copies,
and nested color owners all preserve contextual expressions through cascade.

## C2: explicit computed-value context

Ownership:

- `components/livery/properties.toml`
- `components/livery/src/media.rs`
- `components/livery/src/cascade.rs`
- `components/genet-livery/src/style.rs`

Work:

- Implement inherited `color-scheme` as `normal` or an ordered supported
  scheme list with the `only` flag.
- Separate the host preference used by `prefers-color-scheme` from each
  element's used color scheme.
- Add a host-owned `SystemPalette` keyed by used scheme and `SystemColor`.
- Compute in dependency order: custom values and the `color-scheme` winner;
  element used scheme; foreground `color` against inherited foreground; then
  all remaining color-bearing values.
- Resolve system colors to absolute colors before inheritance. Preserve
  non-foreground `CurrentColor` expressions across inheritance.

Done when a directly specified system color follows the child's scheme while
an inherited system color remains the parent's already-computed absolute
value.

## C3: observables and consumers

Ownership:

- `components/genet-livery/src/style.rs`
- `components/genet-livery/src/paint.rs`
- `components/genet-livery/src/text.rs`
- `components/genet-livery/src/document.rs`
- animation and interpolation paths in `document.rs` and
  `components/livery/src/values/mod.rs`

Work:

- Serialize computed colors with the element's used foreground context.
- Route backgrounds, borders, decoration, gradients, shadows, and text
  through `ComputedColor::resolve_used`.
- Remove the black fallback for unresolved valid colors.
- Expose checked preferred-scheme and palette setters that invalidate style
  and paint.
- Interpolate resolved computed endpoints. Contextual endpoints must not fall
  into an accidental discrete-animation path.

Done when CSSOM and headed paint agree after inheritance, element-scheme
changes, palette changes, and an animation sample.

## Acceptance receipts

Focused unit and retained-document receipts:

1. `background-color: contrast-color(currentcolor)` changes with the
   element's used foreground and produces no diagnostic.
2. An inherited `color-mix(... currentcolor ...)` background re-evaluates on
   a child with a different `color`.
3. A nested `color: color-mix(... currentcolor ...)` resolves against the
   inherited foreground before its result is inherited.
4. Injected light and dark palettes distinguish direct system-color use from
   inherited system-color use; computed CSS contains no system keyword.
5. Preference and palette mutation invalidate both CSSOM and headed paint.

Selected WPT gates:

- `css/css-color/color-mix-currentcolor-{001,002,003}.html`
- `css/css-color/color-mix-currentcolor-nested-for-color-property.html`
- `css/css-color/contrast-color-currentcolor-inherited.html`
- `css/css-color/relative-currentcolor-*`
- `css/css-color/parsing/alpha-color-{parsing-valid,computed}.html`
- computed contrast, relative-color, and valid `color-layers()` tests
- `css/css-color/system-color-compute.html`
- `css/css-color-adjust/parsing/color-scheme-*`
- `css/css-color-adjust/rendering/dark-color-scheme/color-scheme-system-colors.html`

## Stop rules

- Stop if the implementation adds a public `Color` field plus a hidden AST
  sidecar.
- Stop if any system color resolves during parsing or remains unresolved
  after computed-value generation.
- Stop if an inherited non-foreground `currentcolor` expression freezes to
  the parent's RGBA.
- Stop if a foreground `currentcolor` expression re-evaluates on every
  descendant.
- Stop if element scheme is inferred only from the media preference.
- Stop if paint retains a black fallback for a valid unresolved expression.
- Do not call the seam complete while gradients, shadows, or animation
  silently retain the eager `Color` path.
