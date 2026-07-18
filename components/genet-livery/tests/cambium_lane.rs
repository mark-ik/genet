use genet_livery::{InteractionStates, StyleSet, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use livery::{
    media::Device,
    selector::StatePseudoClass,
    values::{Color, FontSize, Length, LengthPercentage, Size},
};

const THEME: &str = include_str!("../../livery/tests/fixtures/cambium-component-catalog.css");

const CATALOG: &str = r#"
<!doctype html>
<html>
  <body>
    <main class="component-catalog">
      <h1 class="catalog-title">Cambium component catalog</h1>
      <div class="catalog-label">Controls</div>
      <section class="catalog-section">
        <div class="catalog-row"><button class="catalog-button" style="width: 100px">Apply</button></div>
        <div class="catalog-row"><div class="slider-track"><div class="slider-thumb"></div></div></div>
      </section>
    </main>
  </body>
</html>
"#;

#[test]
fn cambium_catalog_resolves_through_the_layout_dom_boundary() {
    let document = StaticDocument::parse(CATALOG);
    let styles = StyleSet::cambium(&[THEME]);
    assert!(
        styles.diagnostics().is_empty(),
        "{:?}",
        styles.diagnostics()
    );

    let states = InteractionStates::default();
    let plane = resolve_styles(&document, &styles, &Device::screen(800.0, 600.0), &states);
    let main = document
        .first_with_class(document.document(), "component-catalog")
        .unwrap();
    let computed = plane.get(main).unwrap();

    assert_eq!(
        computed.width,
        Size::Value(LengthPercentage::Length(Length::rem(42.0)))
    );
    assert_eq!(computed.color, "#202733".parse::<Color>().unwrap());
}

#[test]
fn host_state_and_inline_style_enter_the_same_cascade() {
    let document = StaticDocument::parse(CATALOG);
    let styles = StyleSet::cambium(&[THEME]);
    let button = document
        .first_with_class(document.document(), "catalog-button")
        .unwrap();
    let mut states = InteractionStates::default();
    states.set(button, StatePseudoClass::Hover, true);

    let plane = resolve_styles(&document, &styles, &Device::screen(800.0, 600.0), &states);
    let button_style = plane.get(button).unwrap();
    assert_eq!(
        button_style.background_color,
        "#e8eef8".parse::<Color>().unwrap()
    );
    assert_eq!(
        button_style.width,
        Size::Value(LengthPercentage::Length(Length::px(100.0)))
    );
}

#[test]
fn standalone_layout_consumes_livery_values_without_stylo() {
    let document = StaticDocument::parse(CATALOG);
    let styles = StyleSet::cambium(&[THEME]);
    let plane = resolve_styles(
        &document,
        &styles,
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 800.0, 600.0).unwrap();

    let main = document
        .first_with_class(document.document(), "component-catalog")
        .unwrap();
    let track = document
        .first_with_class(document.document(), "slider-track")
        .unwrap();
    let thumb = document
        .first_with_class(document.document(), "slider-thumb")
        .unwrap();

    let main_fragment = fragments.get(main).unwrap();
    let track_fragment = fragments.get(track).unwrap();
    let thumb_fragment = fragments.get(thumb).unwrap();

    assert_eq!(main_fragment.width, 720.0);
    assert_eq!((track_fragment.width, track_fragment.height), (256.0, 8.0));
    assert_eq!((thumb_fragment.width, thumb_fragment.height), (18.0, 18.0));
    assert!(track_fragment.y > main_fragment.y);
}

#[test]
fn inherited_relative_font_sizes_are_computed_once() {
    let document =
        StaticDocument::parse(r#"<html><body><main><span>text</span></main></body></html>"#);
    let plane = resolve_styles(
        &document,
        &StyleSet::cambium(&["main { font-size: 1.5em; }"]),
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let main = document.first_tag(document.document(), "main").unwrap();
    let span = document.first_tag(document.document(), "span").unwrap();
    let expected = FontSize::Value(LengthPercentage::Length(Length::px(24.0)));

    assert_eq!(plane.get(main).unwrap().font_size, expected);
    assert_eq!(plane.get(span).unwrap().font_size, expected);
}

#[test]
fn geometry_properties_reach_taffy() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="stage"><div class="box"></div></div></body></html>"#,
    );
    let plane = resolve_styles(
        &document,
        &StyleSet::cambium(&[
            ".stage { position: relative; width: 200px; height: 120px; } \
             .box { position: absolute; right: 12px; bottom: 8px; width: 40px; \
                    min-height: 20px; max-height: 60px; box-sizing: border-box; \
                    height: 40px; }",
        ]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 320.0, 240.0).unwrap();
    let stage = document
        .first_with_class(document.document(), "stage")
        .unwrap();
    let box_node = document
        .first_with_class(document.document(), "box")
        .unwrap();
    let stage_fragment = fragments.get(stage).unwrap();
    let box_fragment = fragments.get(box_node).unwrap();

    assert_eq!(
        (stage_fragment.width, stage_fragment.height),
        (200.0, 120.0)
    );
    assert_eq!((box_fragment.width, box_fragment.height), (40.0, 40.0));
    assert!((box_fragment.x - 156.0).abs() <= 0.5);
    assert!((box_fragment.y - 80.0).abs() <= 0.5);
}

#[test]
fn flex_properties_reach_taffy() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="row"><div class="first"></div><div class="second"></div></div></body></html>"#,
    );
    let plane = resolve_styles(
        &document,
        &StyleSet::cambium(&[
            ".row { display: flex; width: 200px; height: 40px; gap: 10px; \
                    flex-direction: row; align-items: center; justify-content: start; } \
             .first { width: 40px; height: 20px; } \
             .second { flex-grow: 1; min-width: 30px; height: 20px; }",
        ]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 320.0, 240.0).unwrap();
    let first = document
        .first_with_class(document.document(), "first")
        .unwrap();
    let second = document
        .first_with_class(document.document(), "second")
        .unwrap();
    let first_fragment = fragments.get(first).unwrap();
    let second_fragment = fragments.get(second).unwrap();

    assert_eq!((first_fragment.width, first_fragment.height), (40.0, 20.0));
    assert_eq!(second_fragment.height, 20.0);
    assert!(second_fragment.x >= first_fragment.x + first_fragment.width + 9.5);
    assert!(second_fragment.width >= 140.0);
}

#[test]
fn flex_order_reorders_layout_items() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="row"><div class="first"></div><div class="second"></div></div></body></html>"#,
    );
    let plane = resolve_styles(
        &document,
        &StyleSet::cambium(&[".row { display: flex; width: 100px; height: 20px; } \
             .first { width: 20px; height: 20px; order: 2; } \
             .second { width: 20px; height: 20px; order: 1; }"]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 320.0, 240.0).unwrap();
    let first = document
        .first_with_class(document.document(), "first")
        .unwrap();
    let second = document
        .first_with_class(document.document(), "second")
        .unwrap();
    assert!(fragments.get(second).unwrap().x < fragments.get(first).unwrap().x);
}

#[test]
fn grid_tracks_and_placements_reach_taffy() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="grid"><div class="first"></div><div class="second"></div></div></body></html>"#,
    );
    let plane = resolve_styles(
        &document,
        &StyleSet::cambium(&[".grid { display: grid; width: 200px; height: 100px; \
                     grid-template-columns: 40px 1fr; grid-template-rows: 30px 1fr; \
                     column-gap: 10px; row-gap: 5px; } \
             .first { grid-column-start: 1; grid-column-end: 2; \
                     grid-row-start: 1; grid-row-end: 2; } \
             .second { grid-column-start: 2; grid-column-end: 3; \
                       grid-row-start: 2; grid-row-end: 3; }"]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 320.0, 240.0).unwrap();
    let first = document
        .first_with_class(document.document(), "first")
        .unwrap();
    let second = document
        .first_with_class(document.document(), "second")
        .unwrap();
    let first_fragment = fragments.get(first).unwrap();
    let second_fragment = fragments.get(second).unwrap();

    assert_eq!((first_fragment.width, first_fragment.height), (40.0, 30.0));
    assert_eq!(
        (second_fragment.width, second_fragment.height),
        (150.0, 65.0)
    );
    assert!((second_fragment.x - (first_fragment.x + 50.0)).abs() <= 0.5);
    assert!((second_fragment.y - (first_fragment.y + 35.0)).abs() <= 0.5);
}
