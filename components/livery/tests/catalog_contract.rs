use std::collections::{BTreeMap, BTreeSet};

use livery::{PropertyId, ShorthandId};

const CAMBIUM_CATALOG: &str = include_str!("fixtures/cambium-component-catalog.css");

fn without_comments(css: &str) -> String {
    let mut clean = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    let mut in_comment = false;
    while let Some(ch) = chars.next() {
        if in_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_comment = false;
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_comment = true;
        } else {
            clean.push(ch);
        }
    }
    clean
}

fn declaration_names(css: &str) -> BTreeSet<String> {
    let clean = without_comments(css);
    let mut names = BTreeSet::new();
    for block in clean
        .split('{')
        .skip(1)
        .filter_map(|tail| tail.split_once('}'))
    {
        for declaration in block.0.split(';') {
            let Some((name, _value)) = declaration.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

#[test]
fn cambium_catalog_declarations_are_covered_by_the_native_seed() {
    let properties = PropertyId::ALL
        .iter()
        .map(|id| id.metadata().name)
        .collect::<BTreeSet<_>>();
    let shorthands = ShorthandId::ALL
        .iter()
        .map(|id| {
            let metadata = id.metadata();
            (
                metadata.name,
                metadata
                    .longhands
                    .iter()
                    .map(|longhand| longhand.metadata().name)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let declarations = declaration_names(CAMBIUM_CATALOG);
    let mut missing = BTreeSet::new();

    assert!(
        !declarations.is_empty(),
        "catalog CSS produced no declarations"
    );
    for shorthand in ["border", "margin", "padding"] {
        assert!(
            declarations.contains(shorthand),
            "catalog fixture stopped exercising the {shorthand} shorthand"
        );
    }

    for declaration in &declarations {
        if properties.contains(declaration.as_str()) {
            continue;
        }
        if let Some(longhands) = shorthands.get(declaration.as_str()) {
            missing.extend(
                longhands
                    .iter()
                    .filter(|longhand| !properties.contains(**longhand))
                    .map(|longhand| (*longhand).to_owned()),
            );
        } else {
            missing.insert(declaration.clone());
        }
    }

    assert_eq!(properties.len(), 40, "the audited seed count changed");
    assert!(
        missing.is_empty(),
        "Cambium catalog declarations missing from properties.toml: {missing:?}"
    );
}

#[test]
fn generated_property_names_round_trip() {
    assert_eq!(PropertyId::ALL.len(), 40);
    for &property in PropertyId::ALL {
        let metadata = property.metadata();
        assert_eq!(PropertyId::from_css_name(metadata.name), Some(property));
        assert!(!metadata.initial.is_empty());
        assert!(!metadata.grammar.is_empty());
        assert!(!metadata.seed_values.is_empty());
        assert!(metadata.source_url.starts_with("https://www.w3.org/"));
    }
}

#[test]
fn generated_shorthand_names_round_trip() {
    assert_eq!(ShorthandId::ALL.len(), 4);
    for &shorthand in ShorthandId::ALL {
        let metadata = shorthand.metadata();
        assert_eq!(ShorthandId::from_css_name(metadata.name), Some(shorthand));
        assert!(!metadata.longhands.is_empty());
    }
}
