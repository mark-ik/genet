//! Harvest H0 guard: the imported servo-lane property space is known to
//! the catalog, disjoint from the implemented set, and rejected by the
//! parser with the known-unimplemented diagnostic.

use livery::{
    PropertyId, ShorthandId, UNIMPLEMENTED_LONGHANDS, UNIMPLEMENTED_SHORTHANDS,
    cascade::{DeclarationErrorKind, parse_declaration_block},
    unimplemented_longhand, unimplemented_shorthand,
};

#[test]
fn unimplemented_space_is_disjoint_and_resolvable() {
    assert!(!UNIMPLEMENTED_LONGHANDS.is_empty());
    assert!(!UNIMPLEMENTED_SHORTHANDS.is_empty());
    for entry in UNIMPLEMENTED_LONGHANDS {
        assert_eq!(
            PropertyId::from_css_name(entry.name),
            None,
            "{} is implemented and still listed as unimplemented",
            entry.name
        );
        assert_eq!(ShorthandId::from_css_name(entry.name), None, "{}", entry.name);
        assert_eq!(
            unimplemented_longhand(entry.name).map(|found| found.name),
            Some(entry.name)
        );
        assert_eq!(entry.name, entry.name.to_ascii_lowercase());
        assert!(!entry.group.is_empty() && !entry.spec_url.is_empty(), "{}", entry.name);
    }
    for entry in UNIMPLEMENTED_SHORTHANDS {
        assert_eq!(PropertyId::from_css_name(entry.name), None, "{}", entry.name);
        assert_eq!(ShorthandId::from_css_name(entry.name), None, "{}", entry.name);
        assert_eq!(
            unimplemented_shorthand(entry.name).map(|found| found.name),
            Some(entry.name)
        );
        assert!(!entry.sub_properties.is_empty(), "{}", entry.name);
    }
}

#[test]
fn inherited_flag_follows_the_fork_struct_table() {
    for entry in UNIMPLEMENTED_LONGHANDS {
        let group_inherited = matches!(
            entry.group,
            "font"
                | "inherited_box"
                | "inherited_svg"
                | "inherited_table"
                | "inherited_text"
                | "inherited_ui"
                | "list"
        );
        assert_eq!(
            entry.inherited, group_inherited,
            "{} disagrees with its group {}",
            entry.name, entry.group
        );
    }
}

#[test]
fn parser_rejects_known_unimplemented_distinguishably() {
    let cursor = unimplemented_longhand("cursor").expect("cursor is not implemented yet");
    assert_eq!(cursor.group, "inherited_ui");

    let block = parse_declaration_block(
        "cursor: pointer; text-shadow: 1px 1px red; not-a-property: 1; color: red;",
    );
    let kinds: Vec<(&str, DeclarationErrorKind)> = block
        .errors
        .iter()
        .map(|error| (error.name.as_str(), error.kind))
        .collect();
    assert!(kinds.contains(&("cursor", DeclarationErrorKind::KnownUnimplemented)));
    assert!(kinds.contains(&("text-shadow", DeclarationErrorKind::KnownUnimplemented)));
    assert!(kinds.contains(&("not-a-property", DeclarationErrorKind::UnknownProperty)));
    assert!(!kinds.iter().any(|(name, _)| *name == "color"));
}

#[test]
fn aliases_resolve_to_their_canonical_entry() {
    // word-wrap is the spec alias of overflow-wrap; both arrive via H0.
    let canonical = unimplemented_longhand("overflow-wrap").expect("overflow-wrap pending");
    assert_eq!(
        unimplemented_longhand("word-wrap").map(|entry| entry.name),
        Some(canonical.name)
    );
}

#[test]
fn generated_interpolation_dispatches_by_family() {
    use livery::values::{Color, Display, Opacity};
    use livery::{ComputedValues, PropertyValue};

    // A family with defined interpolation blends.
    let from = PropertyValue::Color("#000000".parse::<Color>().unwrap());
    let to = PropertyValue::Color("#0000ff".parse::<Color>().unwrap());
    let mid = from.interpolate(&to, 0.5);
    assert_eq!(mid, PropertyValue::Color(Color::interpolate("#000000".parse().unwrap(), "#0000ff".parse().unwrap(), 0.5)));

    // A discrete family flips at the midpoint.
    let from = PropertyValue::Display(Display::Block);
    let to = PropertyValue::Display(Display::Flex);
    assert_eq!(from.interpolate(&to, 0.49), PropertyValue::Display(Display::Block));
    assert_eq!(from.interpolate(&to, 0.5), PropertyValue::Display(Display::Flex));

    // Tagged reads round-trip through set().
    let mut style = ComputedValues::default();
    let value = PropertyValue::Opacity("0.25".parse::<Opacity>().unwrap());
    style.set(PropertyId::Opacity, value.clone()).unwrap();
    assert_eq!(style.get(PropertyId::Opacity), value);
}

#[test]
fn transition_property_generic_membership_matches_the_flags() {
    use livery::values::TransitionProperty;

    let list: TransitionProperty = "opacity, border-radius".parse().unwrap();
    assert!(list.includes_property(PropertyId::Opacity));
    assert!(list.includes_property(PropertyId::BorderTopLeftRadius));
    assert!(!list.includes_property(PropertyId::Color));
    assert!(!list.includes_property(PropertyId::Width));
    let all: TransitionProperty = "all".parse().unwrap();
    assert!(
        TransitionProperty::TRANSITIONABLE
            .iter()
            .all(|&property| all.includes_property(property))
    );
}
