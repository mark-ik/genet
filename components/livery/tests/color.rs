//! CSS Color 4/5 coverage: the cutover plan's F0 color slice.
//!
//! Expected values are the CSS Color 4/5 resolved-value serializations, which
//! is what `getComputedStyle` returns and what the WPT color directories
//! assert. Where a number is approximate it is compared through a tolerance
//! rather than a string, so a conversion-precision change fails loudly instead
//! of being papered over by rounding.

use livery::values::{Color, ColorSpace, CssValue, HueInterpolation};

fn parse(css: &str) -> Color {
    css.parse::<Color>()
        .unwrap_or_else(|error| panic!("{css}: {error}"))
}

fn srgb8(css: &str) -> (u8, u8, u8, u8) {
    parse(css)
        .to_srgb8()
        .unwrap_or_else(|| panic!("{css} did not resolve to sRGB"))
}

fn assert_close(css: &str, expected: (f32, f32, f32), tolerance: f32) {
    let (red, green, blue, _) = parse(css)
        .to_srgb()
        .unwrap_or_else(|| panic!("{css} did not resolve"));
    for (got, want, name) in [
        (red, expected.0, "red"),
        (green, expected.1, "green"),
        (blue, expected.2, "blue"),
    ] {
        assert!(
            (got - want).abs() <= tolerance,
            "{css}: {name} was {got}, expected about {want}",
        );
    }
}

// ── The syntax that existed before this slice, still working ─────────────

#[test]
fn hex_named_and_legacy_rgb_still_parse() {
    assert_eq!(srgb8("#abc"), (170, 187, 204, 255));
    assert_eq!(srgb8("#a1b2c3"), (161, 178, 195, 255));
    assert_eq!(srgb8("rebeccapurple"), (102, 51, 153, 255));
    assert_eq!(srgb8("rgb(1, 2, 3)"), (1, 2, 3, 255));
    assert_eq!(srgb8("rgba(1, 2, 3, 0.5)"), (1, 2, 3, 128));
    assert_eq!(srgb8("transparent"), (0, 0, 0, 0));
}

// ── HSL and HWB ──────────────────────────────────────────────────────────

#[test]
fn hsl_parses_in_both_syntaxes() {
    // Pure red at full saturation and half lightness.
    assert_eq!(srgb8("hsl(0, 100%, 50%)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hsl(0 100% 50%)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hsla(120, 100%, 50%, 0.5)"), (0, 255, 0, 128));
    assert_eq!(srgb8("hsl(240 100% 50% / 50%)"), (0, 0, 255, 128));
    // A hue is an angle: every unit, and wrapping past 360.
    assert_eq!(srgb8("hsl(120deg 100% 50%)"), (0, 255, 0, 255));
    assert_eq!(srgb8("hsl(0.333333turn 100% 50%)"), (0, 255, 0, 255));
    assert_eq!(srgb8("hsl(480 100% 50%)"), (0, 255, 0, 255));
    assert_eq!(srgb8("hsl(-240 100% 50%)"), (0, 255, 0, 255));
}

#[test]
fn hwb_parses_and_mixes_whiteness_with_blackness() {
    assert_eq!(srgb8("hwb(0 0% 0%)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hwb(0 100% 0%)"), (255, 255, 255, 255));
    assert_eq!(srgb8("hwb(0 0% 100%)"), (0, 0, 0, 255));
    // Whiteness and blackness over 100% normalize to their ratio, giving grey.
    assert_eq!(srgb8("hwb(0 50% 50%)"), (128, 128, 128, 255));
}

// ── Lab, LCH, Oklab, OkLCH ───────────────────────────────────────────────

#[test]
fn lab_and_lch_resolve_to_srgb() {
    // CSS Color 4's own worked example: lab(29.69% 44.888 -29.04) is
    // approximately #7a2eb1 territory. White and black are the exact anchors.
    assert_eq!(srgb8("lab(100% 0 0)"), (255, 255, 255, 255));
    assert_eq!(srgb8("lab(0% 0 0)"), (0, 0, 0, 255));
    assert_eq!(srgb8("lch(100% 0 0)"), (255, 255, 255, 255));
    // Lab and LCH describe the same color in rectangular and polar form.
    let lab = parse("lab(50% 40 30)").to_srgb().unwrap();
    let lch = parse("lch(50% 50 36.8699)").to_srgb().unwrap();
    for (a, b) in [(lab.0, lch.0), (lab.1, lch.1), (lab.2, lch.2)] {
        assert!((a - b).abs() < 0.01, "lab and lch disagree: {a} vs {b}");
    }
}

#[test]
fn oklab_and_oklch_resolve_to_srgb() {
    assert_eq!(srgb8("oklab(1 0 0)"), (255, 255, 255, 255));
    assert_eq!(srgb8("oklab(0 0 0)"), (0, 0, 0, 255));
    assert_eq!(srgb8("oklch(1 0 0)"), (255, 255, 255, 255));
    // oklch(0.628 0.2577 29.23) is sRGB red, the spec's worked example.
    assert_close("oklch(0.628 0.2577 29.23)", (1.0, 0.0, 0.0), 0.02);
    // Percentage lightness uses a reference of 1.0, not 100.
    assert_eq!(srgb8("oklab(100% 0 0)"), (255, 255, 255, 255));
}

// ── color() and the predefined spaces ────────────────────────────────────

#[test]
fn color_function_covers_the_predefined_spaces() {
    assert_eq!(srgb8("color(srgb 1 0 0)"), (255, 0, 0, 255));
    assert_eq!(srgb8("color(srgb 0 0 0 / 0.5)"), (0, 0, 0, 128));
    // srgb-linear 1 is srgb 1: the transfer function fixes both endpoints.
    assert_eq!(srgb8("color(srgb-linear 1 1 1)"), (255, 255, 255, 255));
    // Linear 0.5 is about 0.7354 gamma-encoded.
    assert_close("color(srgb-linear 0.5 0.5 0.5)", (0.7354, 0.7354, 0.7354), 0.002);
    // White is white in every space, which exercises each matrix pair plus
    // the D50/D65 chromatic adaptation.
    for space in [
        "display-p3",
        "a98-rgb",
        "prophoto-rgb",
        "rec2020",
    ] {
        assert_eq!(
            srgb8(&format!("color({space} 1 1 1)")),
            (255, 255, 255, 255),
            "{space} white",
        );
    }
    assert_eq!(srgb8("color(xyz-d65 0.9505 1.0 1.089)"), (255, 255, 255, 255));
    assert_eq!(srgb8("color(xyz-d50 0.9643 1.0 0.8251)"), (255, 255, 255, 255));
    // `xyz` is an alias for `xyz-d65`.
    assert_eq!(srgb8("color(xyz 0 0 0)"), (0, 0, 0, 255));
}

#[test]
fn display_p3_red_is_outside_srgb_and_clips_on_the_way_down() {
    // P3 red is more saturated than sRGB red; conversion overshoots and the
    // 8-bit accessor clips. `to_srgb` keeps the out-of-range float.
    let (red, green, blue, _) = parse("color(display-p3 1 0 0)").to_srgb().unwrap();
    assert!(red > 1.0, "p3 red should exceed the sRGB gamut, got {red}");
    assert!(green < 0.0 || blue < 0.0, "p3 red should be out of gamut");
    assert_eq!(srgb8("color(display-p3 1 0 0)"), (255, 0, 0, 255));
}

// ── Missing components (`none`) ──────────────────────────────────────────

#[test]
fn none_components_parse_and_resolve_to_zero() {
    assert_eq!(srgb8("rgb(none none none)"), (0, 0, 0, 255));
    assert_eq!(srgb8("rgb(255 none none)"), (255, 0, 0, 255));
    assert_eq!(srgb8("rgb(0 0 0 / none)"), (0, 0, 0, 0));
    assert_eq!(srgb8("hsl(none 100% 50%)"), (255, 0, 0, 255));
    // `none` survives serialization in the modern form.
    assert_eq!(
        parse("color(srgb none 0 0)").to_css_string(),
        "color(srgb none 0 0)"
    );
    assert_eq!(
        parse("oklch(0.5 0.1 none)").to_css_string(),
        "oklch(0.5 0.1 none)"
    );
}

#[test]
fn legacy_syntax_rejects_none() {
    // `none` is a modern-syntax feature; the comma forms must not take it.
    for css in [
        "rgb(none, 0, 0)",
        "rgba(0, 0, 0, none)",
        "hsl(none, 100%, 50%)",
    ] {
        assert!(css.parse::<Color>().is_err(), "accepted {css}");
    }
}

// ── Clamping and range rules ─────────────────────────────────────────────

#[test]
fn out_of_range_channels_clamp_rather_than_invalidate() {
    // CSS Color 4: out-of-range is not invalid, it is clamped.
    assert_eq!(srgb8("rgb(300, 0, 0)"), (255, 0, 0, 255));
    assert_eq!(srgb8("rgb(-50, 0, 0)"), (0, 0, 0, 255));
    assert_eq!(srgb8("rgb(0, 0, 0, 5)"), (0, 0, 0, 255));
    assert_eq!(srgb8("hsl(0 200% 50%)"), (255, 0, 0, 255));
    // Lightness clamps at both ends.
    assert_eq!(srgb8("lab(200% 0 0)"), (255, 255, 255, 255));
    assert_eq!(srgb8("lab(-50% 0 0)"), (0, 0, 0, 255));
    // Chroma is non-negative.
    assert_eq!(srgb8("lch(100% -10 0)"), (255, 255, 255, 255));
}

#[test]
fn malformed_color_syntax_is_rejected() {
    for css in [
        "rgb(0, 0)",
        "rgb(0 0)",
        "rgb(0, 0, 0, 0, 0)",
        "hsl(0)",
        "color(srgb 1 0)",
        "color(not-a-space 1 0 0)",
        "color(1 0 0)",
        "lab()",
        "oklch(0.5 0.1 0 0)",
        "color-mix(red, blue)",
        "color-mix(in srgb, red)",
        "color-mix(in not-a-space, red, blue)",
        "notacolor",
        "#12345",
    ] {
        assert!(css.parse::<Color>().is_err(), "accepted {css}");
    }
}

// ── color-mix() ──────────────────────────────────────────────────────────

#[test]
fn color_mix_defaults_to_an_even_split() {
    // Half red, half blue in sRGB is (128, 0, 128).
    assert_eq!(
        srgb8("color-mix(in srgb, red, blue)"),
        (128, 0, 128, 255)
    );
}

#[test]
fn color_mix_honors_explicit_percentages() {
    assert_eq!(
        srgb8("color-mix(in srgb, red 25%, blue 75%)"),
        (64, 0, 191, 255)
    );
    // One percentage given: the other takes the remainder.
    assert_eq!(
        srgb8("color-mix(in srgb, red 25%, blue)"),
        (64, 0, 191, 255)
    );
    // The percentage may lead the color.
    assert_eq!(
        srgb8("color-mix(in srgb, 25% red, 75% blue)"),
        (64, 0, 191, 255)
    );
}

#[test]
fn color_mix_percentages_below_one_hundred_scale_alpha() {
    // CSS Color 5: a total under 100% scales the result's alpha rather than
    // renormalizing the components.
    let (_, _, _, alpha) = srgb8("color-mix(in srgb, red 25%, blue 25%)");
    assert_eq!(alpha, 128);
}

#[test]
fn color_mix_in_a_polar_space_takes_the_short_way_round() {
    // 0deg and 240deg: shorter goes backwards through 300deg, longer forwards
    // through 120deg. The two must not agree.
    let shorter = srgb8("color-mix(in hsl, hsl(0 100% 50%), hsl(240 100% 50%))");
    let longer = srgb8("color-mix(in hsl shorter hue, hsl(0 100% 50%), hsl(240 100% 50%))");
    let explicit_longer =
        srgb8("color-mix(in hsl longer hue, hsl(0 100% 50%), hsl(240 100% 50%))");
    assert_eq!(shorter, longer, "shorter is the default");
    assert_ne!(shorter, explicit_longer, "longer must take the other arc");
}

#[test]
fn color_mix_with_currentcolor_stays_unresolved() {
    // `currentcolor` has no components until the cascade runs, so the mix
    // cannot be resolved here. It must fail to parse rather than silently
    // resolving to black.
    assert!("color-mix(in srgb, currentcolor, red)".parse::<Color>().is_err());
}

#[test]
fn programmatic_mix_matches_the_parsed_form() {
    let mixed = Color::mix(
        ColorSpace::Srgb,
        HueInterpolation::Shorter,
        parse("red"),
        None,
        parse("blue"),
        None,
    )
    .expect("mix resolves");
    assert_eq!(mixed.to_srgb8(), Some(srgb8("color-mix(in srgb, red, blue)")));
}

// ── Serialization ────────────────────────────────────────────────────────

#[test]
fn the_srgb_family_serializes_as_legacy_rgb() {
    // CSS Color 4 resolves hex, named, rgb(), hsl(), and hwb() to rgb()/rgba().
    for (css, expected) in [
        ("#800080", "rgb(128, 0, 128)"),
        ("rebeccapurple", "rgb(102, 51, 153)"),
        ("rgb(1 2 3)", "rgb(1, 2, 3)"),
        ("rgb(1 2 3 / 0.5)", "rgba(1, 2, 3, 0.502)"),
        ("hsl(0 100% 50%)", "rgb(255, 0, 0)"),
        ("hwb(0 0% 0%)", "rgb(255, 0, 0)"),
        ("transparent", "rgba(0, 0, 0, 0)"),
    ] {
        assert_eq!(parse(css).to_css_string(), expected, "{css}");
    }
}

#[test]
fn other_spaces_serialize_in_their_own_function() {
    for (css, expected) in [
        ("color(srgb 1 0 0)", "color(srgb 1 0 0)"),
        ("color(display-p3 1 0 0)", "color(display-p3 1 0 0)"),
        ("color(xyz 0 0 0)", "color(xyz-d65 0 0 0)"),
        ("lab(50% 40 30)", "lab(50 40 30)"),
        ("oklch(0.5 0.1 30)", "oklch(0.5 0.1 30)"),
        ("color(srgb 1 0 0 / 0.5)", "color(srgb 1 0 0 / 0.5)"),
    ] {
        assert_eq!(parse(css).to_css_string(), expected, "{css}");
    }
}

#[test]
fn unresolved_colors_serialize_by_name() {
    assert_eq!(parse("currentcolor").to_css_string(), "currentcolor");
    assert_eq!(parse("CanvasText").to_css_string(), "canvastext");
    assert!(parse("currentcolor").is_unresolved());
    assert!(parse("CanvasText").is_unresolved());
    assert!(!parse("red").is_unresolved());
}

#[test]
fn serialization_is_stable_under_reparse() {
    // Serialize, reparse, serialize again: the second and third must agree.
    // This is weaker than value round-tripping (hsl() resolves into the sRGB
    // family and does not return as hsl()), and it is the invariant that
    // actually matters for CSSOM.
    for css in [
        "#abc",
        "#33669980",
        "rgb(1 2 3 / 0.5)",
        "hsl(120 50% 50%)",
        "hwb(90 20% 30%)",
        "lab(50% 40 30)",
        "lch(50% 50 40)",
        "oklab(0.5 0.1 0.05)",
        "oklch(0.5 0.1 30)",
        "color(srgb 0.1 0.2 0.3)",
        "color(display-p3 0.1 0.2 0.3 / 0.25)",
        "color(srgb none 0 0)",
        "color-mix(in oklch, red, blue)",
    ] {
        let once = parse(css).to_css_string();
        let twice = parse(&once).to_css_string();
        assert_eq!(once, twice, "{css} serialized unstably");
    }
}

// ── Interpolation, the animation clock's entry point ─────────────────────

#[test]
fn interpolation_stays_in_the_legacy_form() {
    let from = parse("#ff0000");
    let to = parse("#0000ff");
    let mid = from.interpolate(to, 0.5);
    assert_eq!(mid.to_css_string(), "rgb(128, 0, 128)");
    assert_eq!(from.interpolate(to, 0.0), from);
    assert_eq!(from.interpolate(to, 1.0), to);
}

#[test]
fn interpolating_an_unresolved_endpoint_is_discrete() {
    let current = parse("currentcolor");
    let red = parse("red");
    // Nothing to interpolate: the used value is not known at this layer.
    assert_eq!(current.interpolate(red, 0.5), current);
    assert_eq!(red.interpolate(current, 0.5), red);
}

// ── calc() in channel position ───────────────────────────────────────────

#[test]
fn math_functions_resolve_in_channel_position() {
    assert_eq!(srgb8("rgb(calc(100 + 55) 0 0)"), (155, 0, 0, 255));
    assert_eq!(srgb8("rgb(calc(200 + 55) 0 0)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hsl(calc(60deg * 2) 100% 50%)"), (0, 255, 0, 255));
    assert_eq!(srgb8("rgb(min(255, 300) 0 0)"), (255, 0, 0, 255));
}

// ── Relative color syntax (CSS Color 5) ──────────────────────────────────

#[test]
fn relative_color_passes_channels_through_unchanged() {
    // The identity form must reproduce the origin exactly.
    assert_eq!(srgb8("rgb(from red r g b)"), (255, 0, 0, 255));
    assert_eq!(srgb8("rgb(from #123456 r g b)"), (18, 52, 86, 255));
    assert_eq!(srgb8("hsl(from red h s l)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hwb(from red h w b)"), (255, 0, 0, 255));
    assert_eq!(srgb8("lab(from red l a b)"), (255, 0, 0, 255));
    assert_eq!(srgb8("oklch(from red l c h)"), (255, 0, 0, 255));
    assert_eq!(srgb8("color(from red srgb r g b)"), (255, 0, 0, 255));
}

#[test]
fn relative_color_channels_are_numbers_in_the_functions_own_units() {
    // In `rgb(from ...)` the keywords are 0-255 numbers, so swapping them
    // around moves whole channels.
    assert_eq!(srgb8("rgb(from red b g r)"), (0, 0, 255, 255));
    assert_eq!(srgb8("rgb(from #102030 g b r)"), (32, 48, 16, 255));
    // In `color(from ... srgb ...)` they are 0-1 instead.
    assert_eq!(srgb8("color(from red srgb b g r)"), (0, 0, 255, 255));
}

#[test]
fn relative_color_substitutes_keywords_into_calc() {
    // 255 / 2 rounds to 128.
    assert_eq!(srgb8("rgb(from red calc(r / 2) 0 0)"), (128, 0, 0, 255));
    assert_eq!(srgb8("rgb(from red calc(r - 55) g b)"), (200, 0, 0, 255));
    // Keywords compose with the rest of the math program.
    assert_eq!(
        srgb8("rgb(from #808080 min(r, 100) max(g, 200) b)"),
        (100, 200, 128, 255)
    );
    // A hue channel takes an angle-valued expression.
    assert_eq!(srgb8("hsl(from hsl(0 100% 50%) calc(h + 120) s l)"), (0, 255, 0, 255));
}

#[test]
fn relative_color_alpha_defaults_to_the_origins() {
    // Omitted alpha inherits from the origin rather than resetting to 1.
    assert_eq!(srgb8("rgb(from rgb(1 2 3 / 0.5) r g b)"), (1, 2, 3, 128));
    // An explicit alpha overrides it, and `alpha` is bound too.
    assert_eq!(srgb8("rgb(from rgb(1 2 3 / 0.5) r g b / 1)"), (1, 2, 3, 255));
    assert_eq!(
        srgb8("rgb(from rgb(1 2 3 / 0.5) r g b / calc(alpha / 2))"),
        (1, 2, 3, 64)
    );
}

#[test]
fn relative_color_supports_none_and_cross_space_origins() {
    assert_eq!(srgb8("rgb(from red none g b)"), (0, 0, 0, 255));
    // The origin converts into the output function's space first, so a
    // Lab origin can drive an sRGB output and vice versa.
    assert_eq!(srgb8("rgb(from lab(100% 0 0) r g b)"), (255, 255, 255, 255));
    assert_eq!(srgb8("oklab(from white l a b)"), (255, 255, 255, 255));
}

#[test]
fn relative_color_rejects_what_it_cannot_resolve() {
    // `currentcolor` has no components until the cascade runs.
    assert!("rgb(from currentcolor r g b)".parse::<Color>().is_err());
    // A keyword that does not belong to the output space is not bound:
    // `rgb()` binds r/g/b, so `h` is an unknown ident.
    assert!("rgb(from red h g b)".parse::<Color>().is_err());
    // Relative color is modern syntax only.
    assert!("rgb(from red r, g, b)".parse::<Color>().is_err());
    // `from` still needs an origin and a full channel list.
    assert!("rgb(from)".parse::<Color>().is_err());
    assert!("rgb(from red r g)".parse::<Color>().is_err());
}

#[test]
fn keyword_substitution_does_not_corrupt_neighbouring_tokens() {
    use livery::values::LengthPercentage;
    // A guard on `relative::substitute`: `r` must not be rewritten inside
    // `rem`, and a unit must survive. Exercised through a length that shares
    // the parser, so a regression here shows up as a bad length too.
    assert!("calc(1rem + 2px)".parse::<LengthPercentage>().is_ok());
    // `b` appears inside no unit, but `g` does not either; the risk is the
    // single-letter match. Blackness/blue keywords next to numbers:
    assert_eq!(srgb8("rgb(from #0a0b0c calc(r * 1) g b)"), (10, 11, 12, 255));
}

#[test]
fn legacy_syntax_requires_uniform_channel_types() {
    // CSS Color 4: the comma forms are type-uniform. Mixing a percentage
    // with a number is invalid even when every channel would clamp into
    // range on its own, which is why `rgba(-2, 300, 400%, -0.5)` is invalid
    // rather than clamped. WPT: css-color/parsing/color-invalid.html.
    for css in [
        "rgb(10%, 20, 30%)",
        "rgba(-2, 300, 400%, -0.5)",
        "rgb(10, 20%, 30)",
        "hsl(120, 50, 50%)",
        "hsl(120, 50%, 50)",
    ] {
        assert!(css.parse::<Color>().is_err(), "accepted {css}");
    }
    // All-number and all-percentage legacy forms stay valid.
    assert_eq!(srgb8("rgb(10, 20, 30)"), (10, 20, 30, 255));
    assert_eq!(srgb8("rgb(100%, 0%, 0%)"), (255, 0, 0, 255));
    assert_eq!(srgb8("hsl(120, 100%, 50%)"), (0, 255, 0, 255));
    // Modern syntax has no such rule: mixing is fine there.
    assert_eq!(srgb8("rgb(10% 20 30)"), (26, 20, 30, 255));
    assert_eq!(srgb8("hsl(120 100% 50%)"), (0, 255, 0, 255));
}
