use genet_layout::{ImagePlane, StylePlane, layout as stylo_layout, run_cascade};
use genet_livery::{Device, InteractionStates, StyleSet, layout as livery_layout, resolve_styles};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use taffy::prelude::{AvailableSpace, Size};

const HTML: &str = r#"
<!doctype html>
<html><body>
  <main class="catalog">
    <section class="panel">
      <div class="track"><div class="thumb"></div></div>
    </section>
  </main>
</body></html>
"#;

const CSS: &str = r#"
html, body, main, section, div { display: block; }
body { margin: 0; }
.catalog { width: 42rem; padding: 1.5rem; }
.panel { border: 1px solid #cdd3de; padding: 1rem; }
.track { width: 16rem; height: 0.5rem; }
.thumb { width: 1rem; height: 1rem; border: 1px solid #234b88; }
"#;

#[test]
fn explicit_cambium_boxes_have_cross_engine_size_parity() {
    let document = StaticDocument::parse(HTML);
    let viewport = Size {
        width: AvailableSpace::Definite(800.0),
        height: AvailableSpace::Definite(600.0),
    };

    let mut stylo_styles = StylePlane::new();
    run_cascade(
        &document,
        &mut stylo_styles,
        euclid::Size2D::new(800.0, 600.0),
        &[CSS],
        None,
    );
    let (stylo_fragments, _, _) =
        stylo_layout(&document, &stylo_styles, &ImagePlane::new(), viewport);

    let livery_styles = resolve_styles(
        &document,
        &StyleSet::cambium(&[CSS]),
        &Device::screen(800.0, 600.0),
        &InteractionStates::default(),
    );
    let livery_fragments = livery_layout(&document, &livery_styles, 800.0, 600.0).unwrap();

    for (class, compare_height) in [
        ("catalog", false),
        ("panel", false),
        ("track", true),
        ("thumb", true),
    ] {
        let id = document
            .first_with_class(document.document(), class)
            .unwrap();
        let stylo = stylo_fragments.rect_of(id).unwrap();
        let livery = livery_fragments.get(id).unwrap();
        assert_eq!(
            livery.width, stylo.size.width,
            "{class} explicit/available width"
        );
        if compare_height {
            assert_eq!(livery.height, stylo.size.height, "{class} explicit height");
        }
    }
}
