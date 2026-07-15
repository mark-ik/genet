use genet_livery::{Device, InteractionStates, StyleSet, emit_paint_list, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use paint_list_api::{PaintCmd, PaintList};

fn render(html: &str, css: &str) -> genet_livery::LiveryPaintList {
    let document = StaticDocument::parse(html);
    let styles = resolve_styles(
        &document,
        &StyleSet::cambium(&[css]),
        &Device::screen(320.0, 240.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &styles, 320.0, 240.0).unwrap();
    emit_paint_list(
        &document,
        &styles,
        &fragments,
        paint_list_api::DeviceIntSize::new(320, 240),
        1,
    )
}

fn command_signature(list: &genet_livery::LiveryPaintList) -> Vec<String> {
    list.commands()
        .iter()
        .map(|command| match command {
            PaintCmd::DrawRect(rect) => format!("rect:{rect:?}"),
            PaintCmd::DrawLinearGradient(gradient) => format!("linear-gradient:{gradient:?}"),
            PaintCmd::DrawBorder(border) => format!("border:{border:?}"),
            PaintCmd::PushClip(clip) => format!("push-clip:{clip:?}"),
            PaintCmd::PopClip => "pop-clip".to_owned(),
            other => format!("other:{other:?}"),
        })
        .collect()
}

#[test]
fn equivalent_inline_and_stylesheet_cases_share_a_native_paint_receipt() {
    let actual = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-color: #101010; \
                 background-image: linear-gradient(red, blue); \
                 border: 2px solid white; border-radius: 8px; }",
    );
    let reference = render(
        r#"<html><body><div style="width: 80px; height: 40px; background-color: #101010; background-image: linear-gradient(red, blue); border: 2px solid white; border-radius: 8px;"></div></body></html>"#,
        "",
    );

    assert_eq!(command_signature(&actual), command_signature(&reference));
}
