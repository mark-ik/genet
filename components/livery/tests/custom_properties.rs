//! Harvest H1: custom properties, `var()` substitution, fallbacks, and
//! cycle-scoped invalidation through the cascade.

use livery::PropertyValue;
use livery::cascade::{
    CascadeLayer, DeclarationErrorKind, MatchedCustomDeclaration, MatchedDeclaration, Origin,
    Specificity, cascade_with_custom, parse_declaration_block,
};
use livery::custom::{CustomProperties, contains_var, substitute};
use livery::values::{Color, Length, LengthPercentage, Margin, Size};

fn matched_block(css: &str, specificity: u32) -> (Vec<MatchedDeclaration>, Vec<MatchedCustomDeclaration>) {
    let block = parse_declaration_block(css);
    assert!(block.errors.is_empty(), "{css}: {:?}", block.errors);
    let declarations = block
        .declarations
        .into_iter()
        .enumerate()
        .map(|(index, declaration)| MatchedDeclaration {
            declaration,
            origin: Origin::Author,
            layer: CascadeLayer::Unlayered,
            specificity: Specificity(specificity),
            source_order: index as u64,
        })
        .collect();
    let custom = block
        .custom
        .into_iter()
        .enumerate()
        .map(|(index, declaration)| MatchedCustomDeclaration {
            declaration,
            origin: Origin::Author,
            layer: CascadeLayer::Unlayered,
            specificity: Specificity(specificity),
            source_order: index as u64,
        })
        .collect();
    (declarations, custom)
}

fn resolve(css: &str) -> (livery::ComputedValues, CustomProperties) {
    let (declarations, custom) = matched_block(css, 1);
    cascade_with_custom(None, None, declarations, custom)
}

#[test]
fn var_substitutes_into_longhands() {
    let (computed, custom) = resolve("--accent: #ff0000; color: var(--accent)");
    assert_eq!(custom.get("--accent").map(String::as_str), Some("#ff0000"));
    assert_eq!(computed.color, "#ff0000".parse::<Color>().unwrap());
}

#[test]
fn var_fallback_applies_when_the_name_is_missing() {
    let (computed, _) = resolve("color: var(--missing, #0000ff)");
    assert_eq!(computed.color, "#0000ff".parse::<Color>().unwrap());
}

#[test]
fn nested_var_in_fallback_substitutes() {
    let (computed, _) = resolve("--b: #00ff00; color: var(--missing, var(--b))");
    assert_eq!(computed.color, "#00ff00".parse::<Color>().unwrap());
}

#[test]
fn custom_names_are_case_sensitive() {
    let (computed, custom) = resolve("--Accent: #ff0000; color: var(--accent, #123456)");
    assert!(custom.contains_key("--Accent"));
    assert_eq!(computed.color, "#123456".parse::<Color>().unwrap());
}

#[test]
fn custom_properties_reference_each_other() {
    let (computed, custom) =
        resolve("--base: 4px; --double: var(--base) var(--base); margin-top: var(--base)");
    assert_eq!(custom.get("--double").map(String::as_str), Some("4px 4px"));
    assert_eq!(
        computed.margin_top,
        Margin::Value(LengthPercentage::Length(Length::px(4.0)))
    );
}

#[test]
fn cycles_invalidate_only_their_members() {
    // --a and --b form the cycle; --c references it with a fallback and
    // recovers, per the fork's member-wise poisoning.
    let (computed, custom) = resolve(
        "--a: var(--b); --b: var(--a); --c: var(--a, #00ff00); \
         color: var(--c); background-color: var(--a, #0000ff)",
    );
    assert!(!custom.contains_key("--a"));
    assert!(!custom.contains_key("--b"));
    assert_eq!(custom.get("--c").map(String::as_str), Some("#00ff00"));
    assert_eq!(computed.color, "#00ff00".parse::<Color>().unwrap());
    assert_eq!(computed.background_color, "#0000ff".parse::<Color>().unwrap());
}

#[test]
fn invalid_at_computed_value_time_behaves_as_unset() {
    // width is not inherited: unset = initial (auto).
    let (computed, _) = resolve("width: 50px; width: var(--missing)");
    assert_eq!(computed.width, Size::Auto);

    // color is inherited: unset = inherit from the parent.
    let parent = {
        let (parent, _) = resolve("color: #112233");
        parent
    };
    let (declarations, custom) = matched_block("color: var(--missing)", 1);
    let (computed, _) = cascade_with_custom(Some(&parent), None, declarations, custom);
    assert_eq!(computed.color, "#112233".parse::<Color>().unwrap());
}

#[test]
fn shorthand_with_var_expands_after_substitution() {
    let (computed, _) = resolve("--m: 1px 2px; margin: var(--m)");
    assert_eq!(
        computed.margin_top,
        Margin::Value(LengthPercentage::Length(Length::px(1.0)))
    );
    assert_eq!(
        computed.margin_right,
        Margin::Value(LengthPercentage::Length(Length::px(2.0)))
    );
    assert_eq!(
        computed.margin_bottom,
        Margin::Value(LengthPercentage::Length(Length::px(1.0)))
    );
}

#[test]
fn custom_properties_inherit_and_child_declarations_override() {
    let (_, parent_custom) = resolve("--accent: #ff0000; --keep: 1px");
    let (declarations, custom) = matched_block("--accent: #00ff00; color: var(--accent)", 1);
    let (computed, child_custom) =
        cascade_with_custom(None, Some(&parent_custom), declarations, custom);
    assert_eq!(computed.color, "#00ff00".parse::<Color>().unwrap());
    assert_eq!(child_custom.get("--keep").map(String::as_str), Some("1px"));

    // `initial` removes the inherited value; `unset` restores it.
    let (declarations, custom) = matched_block("--accent: initial; --keep: unset", 1);
    let (_, child_custom) = cascade_with_custom(None, Some(&parent_custom), declarations, custom);
    assert!(!child_custom.contains_key("--accent"));
    assert_eq!(child_custom.get("--keep").map(String::as_str), Some("1px"));
}

#[test]
fn important_custom_declarations_outrank_later_normal_ones() {
    let (computed, custom) = resolve("--x: #ff0000 !important; --x: #00ff00; color: var(--x)");
    assert_eq!(custom.get("--x").map(String::as_str), Some("#ff0000"));
    assert_eq!(computed.color, "#ff0000".parse::<Color>().unwrap());
}

#[test]
fn substituted_css_wide_keyword_applies() {
    let parent = {
        let (parent, _) = resolve("color: #445566");
        parent
    };
    let (declarations, custom) = matched_block("--k: inherit; color: var(--k)", 1);
    let (computed, _) = cascade_with_custom(Some(&parent), None, declarations, custom);
    assert_eq!(computed.color, "#445566".parse::<Color>().unwrap());
}

#[test]
fn var_detection_ignores_quoted_text_and_other_identifiers() {
    assert!(contains_var("var(--x)"));
    assert!(contains_var("calc(var(--x) + 1px)"));
    assert!(!contains_var("\"var(--x)\""));
    assert!(!contains_var("somevar(--x)"));
    assert!(!contains_var("variant"));
}

#[test]
fn substitute_reports_unresolvable_and_malformed_references() {
    let map = CustomProperties::new();
    assert!(substitute("var(--missing)", &map).is_err());
    assert!(substitute("var(notdashed)", &map).is_err());
    assert!(substitute("var(--x", &map).is_err());
    let mut map = CustomProperties::new();
    map.insert("--x".to_owned(), "7px".to_owned());
    assert_eq!(substitute("calc(var(--x) * 2)", &map).unwrap(), "calc(7px * 2)");
}

#[test]
fn empty_and_whitespace_custom_values_are_preserved_as_declared() {
    let (_, custom) = resolve("--empty:; margin-top: 3px");
    assert_eq!(custom.get("--empty").map(String::as_str), Some(""));
    let (computed, _) = resolve("--pad: ; margin-top: var(--pad) 3px");
    // Substituting an empty value leaves " 3px", one component: valid.
    assert_eq!(
        computed.margin_top,
        Margin::Value(LengthPercentage::Length(Length::px(3.0)))
    );
}

#[test]
fn pending_values_do_not_error_at_parse_time() {
    let block = parse_declaration_block("color: var(--x); width: var(--w, 4px)");
    assert!(block.errors.is_empty());
    assert_eq!(block.declarations.len(), 2);
    let _ = PropertyValue::parse; // corpus uses the generated surface elsewhere

    let bad = parse_declaration_block("--: nope");
    assert_eq!(bad.errors.len(), 1);
    assert_eq!(bad.errors[0].kind, DeclarationErrorKind::MalformedDeclaration);
}
