use genet_livery::{Device, InteractionStates, StyleSet, emit_paint_list, layout, resolve_styles};
use genet_static_dom::StaticDocument;
use paint_list_api::{
    BorderDetails, ColorF, DeviceIntSize, EngineId, PaintCmd, PaintEnvelope, PaintList,
};

fn render(html: &str, css: &str, generation: u64) -> genet_livery::LiveryPaintList {
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
        DeviceIntSize::new(320, 240),
        generation,
    )
}

#[test]
fn backgrounds_and_borders_follow_dom_paint_order() {
    let list = render(
        r#"<html><body><div class="parent"><div class="child"></div></div></body></html>"#,
        r#"
        .parent {
            background-color: #ff0000;
            border: 2px solid currentcolor;
            color: #123456;
            height: 100px;
            width: 100px;
        }
        .child { background-color: #0000ff; height: 10px; width: 10px; }
        "#,
        7,
    );

    assert_eq!(list.engine_id(), EngineId::GENET);
    assert_eq!(list.viewport(), DeviceIntSize::new(320, 240));
    assert_eq!(list.generation_id(), 7);
    assert_eq!(list.commands().len(), 3);

    let PaintCmd::DrawRect(parent) = &list.commands()[0] else {
        panic!("parent background paints first");
    };
    assert_eq!(parent.color, ColorF::new(1.0, 0.0, 0.0, 1.0));

    let PaintCmd::DrawBorder(border) = &list.commands()[1] else {
        panic!("parent border follows its background");
    };
    assert_eq!((border.widths.top, border.widths.left), (2.0, 2.0));
    let BorderDetails::Normal(border) = &border.details else {
        panic!("first lane emits normal borders");
    };
    let current = ColorF::new(
        f32::from(0x12_u8) / 255.0,
        f32::from(0x34_u8) / 255.0,
        f32::from(0x56_u8) / 255.0,
        1.0,
    );
    assert_eq!(border.top.color, current);
    assert_eq!(border.right.color, current);
    assert_eq!(border.bottom.color, current);
    assert_eq!(border.left.color, current);

    let PaintCmd::DrawRect(child) = &list.commands()[2] else {
        panic!("child background follows the parent box");
    };
    assert_eq!(child.color, ColorF::new(0.0, 0.0, 1.0, 1.0));
}

#[test]
fn display_none_subtrees_and_transparent_boxes_emit_nothing() {
    let list = render(
        r#"<html><body><div class="hidden"><div class="paint"></div></div><div></div></body></html>"#,
        ".hidden { display: none; } .paint { background-color: red; width: 10px; height: 10px; }",
        1,
    );

    assert!(list.commands().is_empty());
}

#[test]
fn paint_output_crosses_the_neutral_envelope() {
    let list = render(
        r#"<html><body><div class="box"></div></body></html>"#,
        ".box { background-color: rgba(10, 20, 30, 0.5); width: 8px; height: 6px; }",
        42,
    );
    let envelope = PaintEnvelope::from_list(&list);

    assert_eq!(envelope.engine_id(), EngineId::GENET);
    assert_eq!(envelope.generation_id(), 42);
    assert_eq!(envelope.commands().len(), 1);
}

#[test]
fn text_nodes_emit_positioned_glyphs_with_font_resources() {
    let list = render(
        r#"<html><body><div class="label">Livery</div></body></html>"#,
        ".label { color: #123456; font-size: 20px; font-weight: 700; width: 120px; }",
        9,
    );
    let runs = list
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawText(run) => Some(run),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(!runs.is_empty());
    assert!(runs.iter().all(|run| !run.glyphs.is_empty()));
    assert!(runs.iter().all(|run| run.font_size == 20.0));
    assert!(runs.iter().all(|run| {
        run.color
            == ColorF::new(
                f32::from(0x12_u8) / 255.0,
                f32::from(0x34_u8) / 255.0,
                f32::from(0x56_u8) / 255.0,
                1.0,
            )
    }));
    assert!(!list.fonts().is_empty());
    assert!(list.fonts().iter().all(|font| !font.data.is_empty()));
    assert!(runs.iter().all(|run| {
        list.fonts()
            .iter()
            .any(|font| font.key == run.font_instance)
    }));

    let envelope = PaintEnvelope::from_list(&list);
    assert_eq!(envelope.fonts.len(), list.fonts().len());
}

#[test]
fn inherited_text_styles_and_font_keys_are_stable() {
    let html = r#"<html><body><div class="parent">red<span>blue</span></div></body></html>"#;
    let css = ".parent { color: red; font-size: 16px; } span { color: blue; }";
    let first = render(html, css, 1);
    let second = render(html, css, 2);
    let colors = first
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawText(run) => Some(run.color),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(colors.len(), 2);
    assert_eq!(colors[0], ColorF::new(1.0, 0.0, 0.0, 1.0));
    assert_eq!(colors[1], ColorF::new(0.0, 0.0, 1.0, 1.0));
    assert_eq!(
        first
            .fonts()
            .iter()
            .map(|font| font.key)
            .collect::<Vec<_>>(),
        second
            .fonts()
            .iter()
            .map(|font| font.key)
            .collect::<Vec<_>>()
    );
}
