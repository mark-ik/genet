use livery::{cascade::Origin, stylesheet::Stylesheet};

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
fn malformed_tail_is_retained_as_a_diagnostic() {
    let sheet = Stylesheet::parse("main { display: block } trailing", Origin::Author);

    assert_eq!(sheet.rules().len(), 1);
    assert_eq!(sheet.diagnostics().len(), 1);
    assert_eq!(sheet.diagnostics()[0].message, "expected a rule block");
}
