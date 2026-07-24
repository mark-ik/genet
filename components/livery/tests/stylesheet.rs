use livery::{
    PropertyValue,
    cascade::{DeclaredValue, Origin, parse_declaration_block},
    media::Device,
    stylesheet::{ContainerSnapshot, CssRule, Stylesheet},
    values::{ContainerType, WritingMode},
};

#[test]
fn stylesheet_parser_recovers_rules_and_media_groups() {
    let sheet = Stylesheet::parse(
        r#"
        /* comment with a fake } */
        main, section { display: block; color: #202733; }
        @media screen and (min-width: 700px) {
            .wide { width: 42rem; }
        }
        @supports (display: grid) { .future { display: block; } }
        :not( { color: red; }
        footer { display: block; }
        "#,
        Origin::Author,
    );

    assert_eq!(sheet.rules().len(), 3);
    assert_eq!(sheet.diagnostics().len(), 2);
    assert!(
        sheet
            .diagnostics()
            .iter()
            .any(|error| error.message == "unsupported at-rule")
    );
}

#[test]
fn border_radius_shorthand_expands_to_corner_longhands() {
    let block = parse_declaration_block("border-radius: 2px 4px 6px 8px");
    assert!(block.errors.is_empty(), "{:?}", block.errors);
    assert_eq!(block.declarations.len(), 4);
    let values = block
        .declarations
        .iter()
        .map(|declaration| declaration.value.clone())
        .collect::<Vec<_>>();
    assert!(matches!(
        values[0],
        DeclaredValue::Value(PropertyValue::Radius(_))
    ));
    assert!(matches!(
        values[3],
        DeclaredValue::Value(PropertyValue::Radius(_))
    ));
}

#[test]
fn malformed_tail_is_retained_as_a_diagnostic() {
    let sheet = Stylesheet::parse("main { display: block } trailing", Origin::Author);

    assert_eq!(sheet.rules().len(), 1);
    assert_eq!(sheet.diagnostics().len(), 1);
    assert_eq!(sheet.diagnostics()[0].message, "expected a rule block");
}

#[test]
fn stylesheet_parser_retains_opacity_keyframes() {
    let sheet = Stylesheet::parse(
        "@keyframes fade { from { opacity: 0; } 50% { opacity: 0.5; } to { opacity: 1; } }",
        Origin::Author,
    );

    assert!(sheet.diagnostics().is_empty(), "{:?}", sheet.diagnostics());
    assert_eq!(sheet.keyframes().len(), 1);
    let keyframes = &sheet.keyframes()[0];
    assert_eq!(keyframes.name(), "fade");
    assert_eq!(
        keyframes
            .frames()
            .iter()
            .map(|frame| frame.offset())
            .collect::<Vec<_>>(),
        vec![0.0, 0.5, 1.0]
    );
    assert_eq!(keyframes.frames()[0].declarations().declarations.len(), 1);
}

#[test]
fn object_model_retains_top_level_rule_identity() {
    let sheet = Stylesheet::parse(
        r#"
        main { color: #202733; }
        @media (min-width: 700px) { .wide { width: 42rem; } .wider { width: 44rem; } }
        @keyframes fade { from { opacity: 0; } to { opacity: 1; } }
        footer { display: block; }
        "#,
        Origin::Author,
    );

    assert_eq!(sheet.items().len(), 4);
    assert!(matches!(sheet.items()[0], CssRule::Style(_)));
    let CssRule::Media(media) = &sheet.items()[1] else {
        panic!("expected a media rule");
    };
    assert_eq!(media.condition(), "(min-width: 700px)");
    assert_eq!(media.rules().len(), 2);
    assert!(matches!(sheet.items()[2], CssRule::Keyframes(_)));
    // Flattened cascade view: three top-level + two nested style rules.
    assert_eq!(sheet.rules().len(), 4);
    assert_eq!(sheet.keyframes().len(), 1);
}

#[test]
fn container_rules_retain_boolean_size_queries_and_names() {
    let sheet = Stylesheet::parse(
        "@container sidebar (width >= 300px) and (100px < height < 500px) { \
           .card { color: green; } \
         }",
        Origin::Author,
    );
    assert!(sheet.diagnostics().is_empty(), "{:?}", sheet.diagnostics());
    let CssRule::Container(container) = &sheet.items()[0] else {
        panic!("expected container rule");
    };
    assert_eq!(container.query().name(), Some("sidebar"));
    assert_eq!(container.rules().len(), 1);
    assert_eq!(sheet.rules().len(), 1);

    let snapshot = ContainerSnapshot {
        names: vec!["sidebar".to_owned()],
        container_type: ContainerType::Size,
        writing_mode: WritingMode::HorizontalTb,
        width: 320.0,
        height: 240.0,
        inline_size: 320.0,
        block_size: 240.0,
    };
    assert!(container.query().matches(
        std::slice::from_ref(&snapshot),
        &Device::screen(800.0, 600.0)
    ));
    assert!(!container.query().matches(
        &[ContainerSnapshot {
            width: 280.0,
            ..snapshot
        }],
        &Device::screen(800.0, 600.0)
    ));
}

#[test]
fn insert_rule_reindexes_the_cascade_view() {
    let mut sheet = Stylesheet::parse("main { color: #111111; }", Origin::Author);
    let generation = sheet.generation();

    let index = sheet
        .insert_rule("main { color: #222222; }", 1)
        .expect("insert at end");
    assert_eq!(index, 1);
    assert!(sheet.generation() > generation);
    assert_eq!(sheet.rules().len(), 2);

    // Inserting ahead of an existing rule shifts source order: the original
    // rule now cascades later and wins the tie.
    sheet
        .insert_rule("main { color: #333333; }", 0)
        .expect("insert at front");
    let orders: Vec<u64> = (0..sheet.rules().len() as u64).collect();
    let actual: Vec<u64> = sheet
        .rules()
        .iter()
        .map(|rule| rule.source_order())
        .collect();
    assert_eq!(actual, orders);
}

#[test]
fn delete_rule_removes_a_media_group_whole() {
    let mut sheet = Stylesheet::parse(
        "main { color: #111111; } @media (min-width: 1px) { .a { width: 1px; } .b { width: 2px; } }",
        Origin::Author,
    );
    assert_eq!(sheet.rules().len(), 3);
    sheet.delete_rule(1).expect("delete the media group");
    assert_eq!(sheet.items().len(), 1);
    assert_eq!(sheet.rules().len(), 1);
}

#[test]
fn rule_mutation_rejects_bad_input_without_a_generation_bump() {
    use livery::stylesheet::RuleMutationError;

    let mut sheet = Stylesheet::parse("main { color: #111111; }", Origin::Author);
    let generation = sheet.generation();
    assert_eq!(
        sheet.insert_rule("main { color: #222222; }", 5),
        Err(RuleMutationError::IndexSize)
    );
    assert!(matches!(
        sheet.insert_rule("a { color: red; } b { color: blue; }", 0),
        Err(RuleMutationError::Syntax(_))
    ));
    assert!(matches!(
        sheet.insert_rule(":not( { color: red; }", 0),
        Err(RuleMutationError::Syntax(_))
    ));
    assert_eq!(sheet.delete_rule(9), Err(RuleMutationError::IndexSize));
    assert_eq!(sheet.generation(), generation);
    assert_eq!(sheet.rules().len(), 1);
}
