use std::collections::{BTreeMap, BTreeSet};

const PROPERTY_DB: &str = include_str!("../../../docs/second-css-engine/properties.toml");
const CAMBIUM_CATALOG: &str =
    include_str!("../../../docs/second-css-engine/fixtures/cambium-component-catalog.css");

fn quoted_values(line: &str) -> impl Iterator<Item = &str> {
    line.split('"').skip(1).step_by(2)
}

fn property_names(toml: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_property = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_property = line == "[[property]]";
            continue;
        }
        if in_property && line.starts_with("name =") {
            names.extend(quoted_values(line).next().map(str::to_owned));
            in_property = false;
        }
    }
    names
}

fn shorthand_expansions(toml: &str) -> BTreeMap<String, Vec<String>> {
    let mut expansions = BTreeMap::new();
    let mut name: Option<String> = None;
    let mut longhands = Vec::new();
    let mut reading_longhands = false;

    let finish = |name: &mut Option<String>,
                  longhands: &mut Vec<String>,
                  expansions: &mut BTreeMap<String, Vec<String>>| {
        if let Some(name) = name.take() {
            expansions.insert(name, std::mem::take(longhands));
        }
    };

    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            finish(&mut name, &mut longhands, &mut expansions);
            reading_longhands = false;
            name = line
                .strip_prefix("[shorthands.")
                .and_then(|section| section.strip_suffix(']'))
                .map(|section| section.replace('_', "-"));
            continue;
        }
        if name.is_none() {
            continue;
        }
        if line.starts_with("css_name =") {
            name = quoted_values(line).next().map(str::to_owned);
        }
        if line.starts_with("longhands =") {
            reading_longhands = true;
        }
        if reading_longhands {
            longhands.extend(quoted_values(line).map(str::to_owned));
            if line.contains(']') {
                reading_longhands = false;
            }
        }
    }
    finish(&mut name, &mut longhands, &mut expansions);
    expansions
}

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
    let properties = property_names(PROPERTY_DB);
    let shorthands = shorthand_expansions(PROPERTY_DB);
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
        if properties.contains(declaration) {
            continue;
        }
        if let Some(longhands) = shorthands.get(declaration) {
            missing.extend(
                longhands
                    .iter()
                    .filter(|longhand| !properties.contains(*longhand))
                    .cloned(),
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
