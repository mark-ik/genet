use genet_livery::{Device, InteractionStates, StyleSet, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;

#[test]
fn probe_positioned_grid_items() {
    // css/css-grid/abspos/positioned-grid-items-001.html, reduced.
    let html = r#"<html><body>
      <div id="grid">
        <div id="first">First item</div>
        <div id="second">Second item</div>
        <div id="third">Third item</div>
        <div id="fourth">Fourth item</div>
      </div>
    </body></html>"#;
    let css = r#"
      #grid {
        display: grid;
        grid-template-rows: 150px 100px;
        grid-template-columns: 200px 300px;
        margin: 1px 2px 3px 4px;
        padding: 20px 15px 10px 5px;
        border-width: 9px 3px 12px 6px;
        border-style: solid;
        width: 550px;
        height: 400px;
        position: relative;
      }
      #grid > div { position: absolute; }
      #first  { grid-column: 1 / 2; grid-row: 1 / 2; }
      #second { grid-column: 2 / 3; grid-row: 1 / 2; }
      #third  { grid-column: 1 / 2; grid-row: 2 / 3; }
      #fourth { grid-column: 2 / 3; grid-row: 2 / 3; }
    "#;
    let document = StaticDocument::parse(html);
    let styles = resolve_styles(
        &document,
        &StyleSet::cambium(&[css]),
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &styles, 800.0, 600.0).unwrap();

    let by_id = |needle: &str| {
        document
            .all_nodes()
            .find(|&id| document.attribute(id, "id").as_deref() == Some(needle))
            .expect(needle)
    };
    for name in ["grid", "first", "second", "third", "fourth"] {
        let id = by_id(name);
        let fragment = fragments.get(id).copied().unwrap_or_default();
        println!(
            "{name:8} x={:7.2} y={:7.2} w={:7.2} h={:7.2}",
            fragment.x, fragment.y, fragment.width, fragment.height,
        );
    }
    // Spec: each item's containing block is its grid area. With auto insets
    // the static position is the grid-area origin. Grid content origin is
    // border(6,9)+padding(5,20) inside the container, which sits at the
    // body margin plus its own margin.
    panic!("probe only");
}
