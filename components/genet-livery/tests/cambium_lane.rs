use genet_livery::{InteractionStates, LiveryDocument, StyleSet, layout, resolve_styles};
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
fn viewport_units_resolve_from_the_current_device_before_layout() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="viewport-box"></div></body></html>"#);
    let styles = StyleSet::cambium(
        &[".viewport-box { width: calc(10px + 10vmin); height: 10vh; \
                         font-size: 2vmin; transform: translate(10vw, 10vh); \
                         display: grid; grid-template-columns: 10vmin 10vw; }"],
    );
    let box_node = document
        .first_with_class(document.document(), "viewport-box")
        .unwrap();
    let plane = resolve_styles(
        &document,
        &styles,
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 800.0, 600.0).unwrap();

    assert_eq!(
        plane.computed_style(box_node, "width").as_deref(),
        Some("calc(70px)")
    );
    assert_eq!(
        plane.computed_style(box_node, "font-size").as_deref(),
        Some("12px")
    );
    assert_eq!(
        plane.computed_style(box_node, "transform").as_deref(),
        Some("matrix(1, 0, 0, 1, 80, 60)")
    );
    assert_eq!(
        plane
            .computed_style(box_node, "grid-template-columns")
            .as_deref(),
        Some("60px 80px")
    );
    assert_eq!(
        (
            fragments.get(box_node).unwrap().width,
            fragments.get(box_node).unwrap().height,
        ),
        (70.0, 60.0)
    );

    let narrower = resolve_styles(
        &document,
        &styles,
        &Device::screen(400.0, 100.0),
        &InteractionStates::default(),
    );
    assert_eq!(
        narrower.computed_style(box_node, "width").as_deref(),
        Some("calc(20px)")
    );
}

#[test]
fn container_units_select_axes_independently_and_use_content_box_sizes() {
    let document = StaticDocument::parse(
        r#"<html><body>
          <div class="inline outer">
            <div class="size outer">
              <div class="inline inner"><div class="nested-target"></div></div>
            </div>
          </div>
          <div class="inline fallback"><div class="fallback-target"></div></div>
          <div class="size bordered"><div class="bordered-target"></div></div>
        </body></html>"#,
    );
    let nested = document
        .first_with_class(document.document(), "nested-target")
        .unwrap();
    let fallback = document
        .first_with_class(document.document(), "fallback-target")
        .unwrap();
    let bordered = document
        .first_with_class(document.document(), "bordered-target")
        .unwrap();
    let styles = StyleSet::cambium(&[".inline { container-type: inline-size; } \
         .size { container-type: size; } \
         .inline.outer { width: 500px; } \
         .size.outer { height: 400px; } \
         .inline.inner { width: 300px; } \
         .nested-target { width: 10cqi; height: 10cqb; margin-left: 10cqmin; \
                          padding-left: max(10cqi, 10cqb); } \
         .fallback { width: 70px; height: 30px; } \
         .fallback-target { left: 10cqw; top: 10cqh; \
                            margin-left: 10cqmax; margin-right: 10cqmin; } \
         .bordered { width: 100px; height: 50px; box-sizing: border-box; \
                     border: 10px solid green; padding: 10px; } \
         .bordered-target { width: 10cqi; height: 10cqb; }"]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 80.0));

    assert_eq!(
        retained.computed_style(nested, "width").as_deref(),
        Some("30px")
    );
    assert_eq!(
        retained.computed_style(nested, "height").as_deref(),
        Some("40px")
    );
    assert_eq!(
        retained.computed_style(nested, "margin-left").as_deref(),
        Some("30px")
    );
    assert_eq!(
        retained.computed_style(nested, "padding-left").as_deref(),
        Some("40px")
    );
    assert_eq!(
        retained.computed_style(fallback, "left").as_deref(),
        Some("7px")
    );
    assert_eq!(
        retained.computed_style(fallback, "top").as_deref(),
        Some("8px")
    );
    assert_eq!(
        retained.computed_style(fallback, "margin-left").as_deref(),
        Some("8px")
    );
    assert_eq!(
        retained.computed_style(fallback, "margin-right").as_deref(),
        Some("7px")
    );
    assert_eq!(
        retained.computed_style(bordered, "width").as_deref(),
        Some("6px")
    );
    assert_eq!(
        retained.computed_style(bordered, "height").as_deref(),
        Some("1px")
    );

    retained
        .frame(200, 80)
        .expect("retained container-unit frame");
    assert_eq!(
        retained.computed_style(nested, "width").as_deref(),
        Some("30px")
    );
}

#[test]
fn logical_units_follow_vertical_writing_and_container_axes() {
    let document = StaticDocument::parse(
        r#"<html><body>
          <div class="size"><div class="inline"><div class="target"></div></div></div>
          <div class="viewport-target"></div>
        </body></html>"#,
    );
    let target = document
        .first_with_class(document.document(), "target")
        .unwrap();
    let viewport_target = document
        .first_with_class(document.document(), "viewport-target")
        .unwrap();
    let styles = StyleSet::cambium(
        &[".size { writing-mode: vertical-rl; container-type: size; \
                 width: 400px; height: 500px; } \
         .inline { container-type: inline-size; height: 300px; } \
         .target { width: 10cqi; height: 10cqb; \
                   margin-left: 10cqw; margin-right: 10cqh; } \
         .viewport-target { writing-mode: vertical-lr; width: 10vi; height: 10vb; }"],
    );
    let mut retained = LiveryDocument::new(document, styles, Device::screen(600.0, 400.0));

    for (node, property, expected) in [
        (target, "width", "30px"),
        (target, "height", "40px"),
        (target, "margin-left", "40px"),
        (target, "margin-right", "30px"),
        (viewport_target, "width", "40px"),
        (viewport_target, "height", "60px"),
    ] {
        assert_eq!(
            retained.computed_style(node, property).as_deref(),
            Some(expected),
            "{property}"
        );
    }

    retained.frame(600, 400).expect("vertical logical frame");
    assert_eq!(
        retained.computed_style(target, "width").as_deref(),
        Some("30px")
    );
}

#[test]
fn named_container_queries_recascade_from_laid_out_sizes() {
    let document = StaticDocument::parse(
        r#"<html><body>
          <div class="panel wide"><div class="wide-card card"></div></div>
          <div class="panel narrow"><div class="narrow-card card"></div></div>
        </body></html>"#,
    );
    let wide = document
        .first_with_class(document.document(), "wide-card")
        .unwrap();
    let narrow = document
        .first_with_class(document.document(), "narrow-card")
        .unwrap();
    let styles = StyleSet::cambium(&[".panel { container-type: size; container-name: sidebar; \
                  height: 200px; } \
         .wide { width: 320px; } .narrow { width: 200px; } \
         .card { color: red; width: 10px; } \
         @container sidebar (width >= 300px) and (height < 250px) { \
           .card { color: green; width: 42px; } \
         }"]);
    assert!(
        styles.diagnostics().is_empty(),
        "{:?}",
        styles.diagnostics()
    );
    let retained = LiveryDocument::new(document, styles, Device::screen(800.0, 600.0));

    assert_eq!(
        retained.computed_style(wide, "color").as_deref(),
        Some("#008000")
    );
    assert_eq!(
        retained.computed_style(wide, "width").as_deref(),
        Some("42px")
    );
    assert_eq!(
        retained.computed_style(narrow, "color").as_deref(),
        Some("#ff0000")
    );
    assert_eq!(
        retained.computed_style(narrow, "width").as_deref(),
        Some("10px")
    );
}

#[test]
fn container_type_applies_physical_size_containment() {
    let document = StaticDocument::parse(
        r#"<html><body>
          <div class="size block"><div class="child"></div></div>
          <div class="inline horizontal"><div class="child"></div></div>
          <div class="inline vertical"><div class="child"></div></div>
          <div class="size flex"><div class="child"></div></div>
          <div class="size grid"><div class="child"></div></div>
        </body></html>"#,
    );
    let styles = StyleSet::cambium(&[".child { width: 80px; height: 40px; } \
         .size { container-type: size; width: 100px; } \
         .block { padding: 5px; border: 2px solid green; } \
         .inline { container-type: inline-size; width: 100px; } \
         .vertical { writing-mode: vertical-rl; } \
         .flex { display: flex; } \
         .grid { display: grid; }"]);
    let plane = resolve_styles(
        &document,
        &styles,
        &Device::screen(400.0, 300.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 400.0, 300.0).unwrap();

    let block = document
        .first_with_class(document.document(), "block")
        .unwrap();
    let horizontal = document
        .first_with_class(document.document(), "horizontal")
        .unwrap();
    let vertical = document
        .first_with_class(document.document(), "vertical")
        .unwrap();
    let flex = document
        .first_with_class(document.document(), "flex")
        .unwrap();
    let grid = document
        .first_with_class(document.document(), "grid")
        .unwrap();

    assert_eq!(fragments.get(block).unwrap().height, 14.0);
    assert_eq!(fragments.get(horizontal).unwrap().height, 40.0);
    assert_eq!(fragments.get(vertical).unwrap().height, 0.0);
    assert_eq!(fragments.get(flex).unwrap().height, 0.0);
    assert_eq!(fragments.get(grid).unwrap().height, 0.0);
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
