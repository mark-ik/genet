use livery::media::{
    AnyPointerCapabilities, ColorGamut, ColorScheme, ContrastPreference, Device, DisplayMode,
    ForcedColors, InvertedColors, MediaQueryList, PointerAccuracy, PointerCapabilities,
    ReducedMotion, ReducedTransparency, Scripting, UpdateFrequency,
};

fn matches(device: &Device, query: &str) -> bool {
    query
        .parse::<MediaQueryList>()
        .unwrap_or_else(|error| panic!("{query}: {error}"))
        .matches(device)
}

#[test]
fn viewport_type_and_query_lists_evaluate() {
    let device = Device::screen(800.0, 600.0);

    assert!(matches(&device, "screen and (min-width: 500px)"));
    assert!(!matches(&device, "print and (min-width: 500px)"));
    assert!(matches(&device, "(orientation: landscape)"));
    assert!(!matches(&device, "not screen"));
    assert!(matches(&device, "(min-width: 1200px), (max-height: 600px)"));
}

#[test]
fn preferences_share_one_environment_without_clobbering() {
    let device = Device {
        color_scheme: ColorScheme::Dark,
        reduced_motion: ReducedMotion::Reduce,
        contrast: ContrastPreference::More,
        reduced_transparency: ReducedTransparency::Reduce,
        inverted_colors: InvertedColors::Inverted,
        forced_colors: ForcedColors::Active,
        display_mode: DisplayMode::Standalone,
        scripting: Scripting::None,
        ..Device::screen(640.0, 480.0)
    };

    for query in [
        "(prefers-color-scheme: dark)",
        "(prefers-reduced-motion: reduce)",
        "(prefers-contrast: more)",
        "(prefers-reduced-transparency: reduce)",
        "(inverted-colors: inverted)",
        "(forced-colors: active)",
        "(display-mode: standalone)",
        "(scripting: none)",
    ] {
        assert!(matches(&device, query), "{query}");
    }
}

#[test]
fn hybrid_pointer_capabilities_remain_multi_valued() {
    let device = Device {
        primary_pointer: PointerCapabilities::MOUSE,
        any_pointer: AnyPointerCapabilities {
            coarse: true,
            fine: true,
            hover: true,
        },
        ..Device::screen(800.0, 600.0)
    };

    assert!(matches(&device, "(pointer: fine) and (hover: hover)"));
    assert!(matches(
        &device,
        "(any-pointer: coarse) and (any-pointer: fine)"
    ));
    assert!(!matches(&device, "(pointer: coarse)"));

    let touch = Device {
        primary_pointer: PointerCapabilities::TOUCH,
        any_pointer: AnyPointerCapabilities {
            coarse: true,
            fine: false,
            hover: false,
        },
        ..device
    };
    assert!(matches(&touch, "(pointer: coarse) and (hover: none)"));
    assert_eq!(touch.primary_pointer.accuracy, PointerAccuracy::Coarse);
}

#[test]
fn ordered_capabilities_and_update_frequency_evaluate() {
    let device = Device {
        color_gamut: ColorGamut::P3,
        update: UpdateFrequency::Slow,
        ..Device::screen(800.0, 600.0)
    };

    assert!(matches(&device, "(color-gamut: srgb)"));
    assert!(matches(&device, "(color-gamut: p3)"));
    assert!(!matches(&device, "(color-gamut: rec2020)"));
    assert!(matches(&device, "(update: slow)"));
}

#[test]
fn malformed_queries_are_rejected_and_unknown_features_do_not_match() {
    assert!("".parse::<MediaQueryList>().is_err());
    assert!(!matches(
        &Device::screen(800.0, 600.0),
        "(unknown-feature: yes)"
    ));
    assert!("(min-width: 2em)".parse::<MediaQueryList>().is_err());
    assert!(
        "screen and min-width: 20px"
            .parse::<MediaQueryList>()
            .is_err()
    );
}
