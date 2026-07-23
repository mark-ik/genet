//! Harvest H3: CSSOM-shaped mutation and getComputedStyle serialization
//! against the retained Livery style plane, the bounded corpus the
//! genet-scripted bridge will drive.

use genet_livery::{LiveryDocument, StyleSet};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use livery::media::Device;
use livery::stylesheet::RuleMutationError;

fn retained(
    author: &str,
) -> (
    LiveryDocument<StaticDocument>,
    <StaticDocument as LayoutDom>::NodeId,
) {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let styles = StyleSet::cambium(&[author]);
    let card = document
        .first_with_class(document.document(), "card")
        .expect("card node");
    (
        LiveryDocument::new(document, styles, Device::screen(200.0, 100.0)),
        card,
    )
}

#[test]
fn computed_style_serializes_longhands_and_custom_properties() {
    let (mut retained, card) = retained(
        ".card { --accent: #ff0000; color: var(--accent); width: 50px; margin-top: 1em; }",
    );
    retained.frame(200, 100).unwrap();

    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#ff0000")
    );
    assert_eq!(
        retained.computed_style(card, "--accent").as_deref(),
        Some("#ff0000")
    );
    assert_eq!(
        retained.computed_style(card, "width").as_deref(),
        Some("50px")
    );
    assert_eq!(retained.computed_style(card, "--missing"), None);
    assert_eq!(retained.computed_style(card, "not-a-property"), None);
}

#[test]
fn computed_style_resolves_without_a_prior_frame() {
    let (retained, card) = retained(".card { color: #00ff00; }");
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#00ff00")
    );
}

#[test]
fn computed_transform_serializes_as_a_resolved_2d_matrix() {
    let (retained, card) =
        retained(".card { font-size: 10px; transform: translate(2em, 4px) skewX(45deg); }");
    assert_eq!(
        retained.computed_style(card, "transform").as_deref(),
        Some("matrix(1, 0, 1, 1, 20, 4)")
    );
}

#[test]
fn computed_transform_resolves_percentages_against_a_definite_box() {
    let (retained, card) =
        retained(".card { width: 100px; height: 50px; transform: translate(25%, 50%); }");
    assert_eq!(
        retained.computed_style(card, "transform").as_deref(),
        Some("matrix(1, 0, 0, 1, 25, 25)")
    );
}

#[test]
fn insert_and_delete_author_rules_restyle_the_retained_document() {
    let (mut retained, card) = retained(".card { color: #111111; }");
    retained.frame(200, 100).unwrap();
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#111111")
    );

    // A later same-specificity rule wins the cascade tie.
    let index = retained
        .insert_author_rule(0, ".card { color: #222222; }", 1)
        .expect("insert");
    assert_eq!(index, 1);
    retained.frame(200, 100).unwrap();
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#222222")
    );

    retained.delete_author_rule(0, 1).expect("delete");
    retained.frame(200, 100).unwrap();
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#111111")
    );
}

#[test]
fn inserted_media_rules_respect_the_device() {
    let (mut retained, card) = retained(".card { color: #111111; }");
    retained
        .insert_author_rule(
            0,
            "@media (min-width: 500px) { .card { color: #333333; } }",
            1,
        )
        .expect("insert non-matching media");
    retained.frame(200, 100).unwrap();
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#111111")
    );

    retained
        .insert_author_rule(
            0,
            "@media (min-width: 100px) { .card { color: #444444; } }",
            2,
        )
        .expect("insert matching media");
    retained.frame(200, 100).unwrap();
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#444444")
    );
}

#[test]
fn rule_mutation_errors_surface_and_leave_the_document_intact() {
    let (mut retained, card) = retained(".card { color: #111111; }");
    retained.frame(200, 100).unwrap();

    assert_eq!(
        retained.insert_author_rule(3, ".card { color: red; }", 0),
        Err(RuleMutationError::IndexSize)
    );
    assert_eq!(
        retained.insert_author_rule(0, ".card { color: red; }", 9),
        Err(RuleMutationError::IndexSize)
    );
    assert!(matches!(
        retained.insert_author_rule(0, "not a rule", 0),
        Err(RuleMutationError::Syntax(_))
    ));
    assert_eq!(
        retained.computed_style(card, "color").as_deref(),
        Some("#111111")
    );
}
