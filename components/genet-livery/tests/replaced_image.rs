use genet_livery::{Device, InteractionStates, StyleSet, emit_paint_list, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};

#[test]
fn html_image_dimensions_override_intrinsic_size_when_css_is_auto() {
    use base64::Engine as _;

    let image = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let document = StaticDocument::parse(&format!(
        r#"<html><body><img src="{data_uri}" width="20" height="96"></body></html>"#
    ));
    let styles = resolve_styles(
        &document,
        &StyleSet::cambium(&["body { margin: 0; }"]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &styles, 320.0, 240.0).expect("layout image");
    let list = emit_paint_list(
        &document,
        &styles,
        &fragments,
        DeviceIntSize::new(320, 240),
        1,
    );
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("replaced image paints")
    else {
        unreachable!();
    };
    assert_eq!(image.placement.bounds.size().width, 20.0);
    assert_eq!(image.placement.bounds.size().height, 96.0);
}
