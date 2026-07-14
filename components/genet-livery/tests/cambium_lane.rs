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
