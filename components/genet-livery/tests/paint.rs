use std::sync::Arc;

use genet_livery::{
    Device, InteractionStates, LiveryDocument, StyleSet, emit_paint_list, layout, resolve_styles,
};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use paint_list_api::{
    BorderDetails, ClipKind, ColorF, DeviceIntSize, EngineId, PaintCmd, PaintEnvelope, PaintList,
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
fn border_radii_reach_the_neutral_border_primitive() {
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-color: lime; \
                 border: 2px solid blue; border-top-left-radius: 8px; \
                 border-bottom-right-radius: 12px; }",
        1,
    );
    let radius = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some(border.radius),
                _ => None,
            },
            _ => None,
        })
        .expect("rounded border paints through the neutral primitive");
    assert_eq!(radius.top_left.width, 8.0);
    assert_eq!(radius.top_left.height, 8.0);
    assert_eq!(radius.bottom_right.width, 12.0);
    assert_eq!(radius.bottom_right.height, 12.0);
    assert_eq!(
        (radius.top_right.width, radius.top_right.height),
        (0.0, 0.0)
    );
}

#[test]
fn border_radii_clip_background_fills() {
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-color: lime; border-radius: 8px; }",
        1,
    );
    let rounded = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushClip(spec) => match &spec.kind {
                ClipKind::RoundedRect { radius, .. } => Some(*radius),
                _ => None,
            },
            _ => None,
        })
        .expect("rounded background emits a rounded clip");
    assert_eq!(rounded.top_left.width, 8.0);
    assert!(
        list.commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PopClip))
    );
}

#[test]
fn linear_gradient_background_reaches_neutral_primitive() {
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-image: linear-gradient(red, blue); }",
        1,
    );
    let gradient = list.commands().iter().find_map(|command| match command {
        PaintCmd::DrawLinearGradient(item) => Some(item),
        _ => None,
    });
    let gradient = gradient.expect("linear-gradient lowers through PaintList");
    assert_eq!(gradient.gradient.stops.len(), 2);
    assert_eq!(gradient.gradient.stops[0].offset, 0.0);
    assert_eq!(gradient.gradient.stops[1].offset, 1.0);
    assert_eq!(
        gradient.gradient.stops[0].color,
        ColorF::new(1.0, 0.0, 0.0, 1.0)
    );
    assert_eq!(
        gradient.gradient.stops[1].color,
        ColorF::new(0.0, 0.0, 1.0, 1.0)
    );
}

#[test]
fn data_uri_background_image_reaches_neutral_image_side_table() {
    use base64::Engine as _;

    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let css = format!(".card {{ width: 80px; height: 40px; background-image: url({data_uri}); }}");
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        &css,
        1,
    );
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("data URI background lowers to an image primitive")
    else {
        unreachable!();
    };
    let resource = list
        .images()
        .iter()
        .find(|resource| resource.key == image.image_key)
        .expect("image command resolves through the paint side table");
    assert_eq!((resource.width, resource.height), (2, 3));
    assert_eq!(resource.data.len(), 2 * 3 * 4);
    assert_eq!(image.placement.bounds.size().width, 2.0);
    assert_eq!(image.placement.bounds.size().height, 3.0);
    assert!(
        list.commands()
            .iter()
            .filter(|command| matches!(command, PaintCmd::DrawImage(_)))
            .count()
            > 1
    );
}

#[test]
fn background_position_and_no_repeat_place_the_intrinsic_image() {
    use base64::Engine as _;

    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let css = format!(
        "body {{ margin: 0; }} .card {{ width: 80px; height: 40px; background-repeat: no-repeat; \
                 background-position: center center; background-image: url({data_uri}); }}"
    );
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        &css,
        1,
    );
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("centered no-repeat image lowers to one primitive")
    else {
        unreachable!();
    };
    assert_eq!(
        image.placement.bounds.min,
        paint_list_api::LayoutPoint::new(39.0, 18.5)
    );
    assert_eq!(
        image.placement.bounds.size(),
        paint_list_api::LayoutSize::new(2.0, 3.0)
    );
    assert_eq!(
        list.commands()
            .iter()
            .filter(|command| matches!(command, PaintCmd::DrawImage(_)))
            .count(),
        1
    );
}

#[test]
fn host_image_resource_resolves_a_non_data_background_url() {
    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let document = StaticDocument::parse(r#"<html><body><div class="card"></div></body></html>"#);
    let mut retained = LiveryDocument::new(
        document,
        StyleSet::cambium(&[
            ".card { width: 80px; height: 40px; background-repeat: no-repeat; background-image: url(support/blue.png); }",
        ]),
        Device::screen(320.0, 240.0),
    );
    retained.set_image_resource("support/blue.png", png);
    let list = retained.frame(320, 240).expect("frame with host image");
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("host image URL lowers to an image primitive")
    else {
        unreachable!();
    };
    let resource = list
        .images()
        .iter()
        .find(|resource| resource.key == image.image_key)
        .expect("host image command resolves through the paint side table");
    assert_eq!((resource.width, resource.height), (2, 3));
}

#[test]
fn host_image_resource_resolves_a_remote_url_without_engine_fetching() {
    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let url = "https://cdn.example.test/blue.png";
    let document = StaticDocument::parse(r#"<html><body><div class="card"></div></body></html>"#);
    let mut retained = LiveryDocument::new(
        document,
        StyleSet::cambium(&[&format!(
            ".card {{ width: 80px; height: 40px; background-repeat: no-repeat; background-image: url({url}); }}"
        )]),
        Device::screen(320.0, 240.0),
    );
    retained.set_image_resource(url, png);
    let list = retained
        .frame(320, 240)
        .expect("frame with host-supplied remote image");
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("host-supplied remote URL lowers to an image primitive")
    else {
        unreachable!();
    };
    let resource = list
        .images()
        .iter()
        .find(|resource| resource.key == image.image_key)
        .expect("remote URL command resolves through the paint side table");
    assert_eq!((resource.width, resource.height), (2, 3));
}

#[test]
fn formatting_whitespace_does_not_shift_following_block_flow() {
    let css = ".card { width: 80px; height: 40px; background-color: lime; }";
    let mut compact = LiveryDocument::new(
        StaticDocument::parse(
            "<html><body><p>hello</p><div class=\"card\"></div></body></html>",
        ),
        StyleSet::cambium(&[css]),
        Device::screen(320.0, 240.0),
    );
    let mut formatted = LiveryDocument::new(
        StaticDocument::parse(
            "<html><body>\n  <p>hello</p>\n  <div class=\"card\"></div>\n</body></html>",
        ),
        StyleSet::cambium(&[css]),
        Device::screen(320.0, 240.0),
    );
    let card_bounds = |document: &mut LiveryDocument<StaticDocument>| {
        document
            .frame(320, 240)
            .expect("formatting-whitespace frame")
            .commands()
            .iter()
            .find_map(|command| match command {
                PaintCmd::DrawRect(item) => Some(item.placement.bounds),
                _ => None,
            })
            .expect("card background")
    };
    assert_eq!(card_bounds(&mut compact), card_bounds(&mut formatted));
}

#[test]
fn replaced_img_uses_intrinsic_size_and_paints_a_neutral_image() {
    use base64::Engine as _;

    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let list = render(
        &format!(r#"<html><body><img src="{data_uri}"></body></html>"#),
        "body { margin: 0; }",
        1,
    );
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("replaced image lowers to a neutral image primitive")
    else {
        unreachable!();
    };
    assert_eq!(
        image.placement.bounds.size(),
        paint_list_api::LayoutSize::new(2.0, 3.0)
    );
    let resource = list
        .images()
        .iter()
        .find(|resource| resource.key == image.image_key)
        .expect("replaced image command resolves through the image side table");
    assert_eq!((resource.width, resource.height), (2, 3));
}

#[test]
fn replaced_img_width_preserves_intrinsic_ratio() {
    use base64::Engine as _;

    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let list = render(
        &format!(r#"<html><body><img src="{data_uri}" class="scaled"></body></html>"#),
        ".scaled { width: 10px; } body { margin: 0; }",
        1,
    );
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("sized replaced image lowers to a neutral image primitive")
    else {
        unreachable!();
    };
    assert_eq!(
        image.placement.bounds.size(),
        paint_list_api::LayoutSize::new(10.0, 15.0)
    );
}

#[test]
fn retained_replaced_img_uses_host_resolved_bytes_for_intrinsic_size() {
    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let document =
        StaticDocument::parse(r#"<html><body><img src="support/blue.png"></body></html>"#);
    let mut retained = LiveryDocument::new(
        document,
        StyleSet::cambium(&["body { margin: 0; }"]),
        Device::screen(320.0, 240.0),
    );
    retained.set_image_resource("support/blue.png", png);
    let list = retained.frame(320, 240).expect("frame with host image");
    let PaintCmd::DrawImage(image) = list
        .commands()
        .iter()
        .find(|command| matches!(command, PaintCmd::DrawImage(_)))
        .expect("host-resolved replaced image lowers to a neutral image primitive")
    else {
        unreachable!();
    };
    assert_eq!(
        image.placement.bounds.size(),
        paint_list_api::LayoutSize::new(2.0, 3.0)
    );
}

#[test]
fn retained_opacity_clock_samples_and_settles() {
    let document = StaticDocument::parse(r#"<html><body><div class="card"></div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let mut retained = LiveryDocument::new(
        document,
        StyleSet::cambium(&[".card { width: 40px; height: 20px; background-color: lime; }"]),
        Device::screen(320.0, 240.0),
    );
    retained.frame(320, 240).unwrap();
    assert!(retained.animate_opacity(card, 0.0, 1.0, 0.0, 1_000.0));
    assert!(!retained.settled());

    let start = retained.frame(320, 240).unwrap();
    let start_layer = start.commands().iter().find_map(|command| match command {
        PaintCmd::PushLayer(layer) => Some(layer.opacity),
        _ => None,
    });
    assert_eq!(start_layer, Some(0.0));

    assert!(retained.pump(500.0));
    let halfway = retained.frame(320, 240).unwrap();
    let halfway_layer = halfway.commands().iter().find_map(|command| match command {
        PaintCmd::PushLayer(layer) => Some(layer.opacity),
        _ => None,
    });
    assert_eq!(halfway_layer, Some(0.5));

    assert!(retained.pump(1_000.0));
    assert!(retained.settled());
    let end = retained.frame(320, 240).unwrap();
    assert!(
        !end.commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(_)))
    );
}

#[test]
fn linear_gradient_layers_over_background_fill() {
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-color: #101010; \
                 background-image: linear-gradient(red, blue); }",
        1,
    );
    let fill = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::DrawRect(_)))
        .expect("background color paints");
    let gradient = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::DrawLinearGradient(_)))
        .expect("background image lowers to a gradient primitive");
    assert!(gradient > fill, "the image layer paints over the fill");
    let PaintCmd::DrawLinearGradient(item) = &list.commands()[gradient] else {
        unreachable!();
    };
    assert_eq!(item.gradient.stops.len(), 2);
    assert_eq!(
        item.gradient.stops[0].color,
        ColorF::new(1.0, 0.0, 0.0, 1.0)
    );
    assert_eq!(
        item.gradient.stops[1].color,
        ColorF::new(0.0, 0.0, 1.0, 1.0)
    );
}

#[test]
fn box_shadow_reaches_the_neutral_shadow_primitive() {
    let list = render(
        r#"<html><body><div class="card"></div></body></html>"#,
        ".card { width: 80px; height: 40px; background-color: white; \
                 border-radius: 6px; box-shadow: 2px 3px 4px #00000080; }",
        1,
    );
    let shadow = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawShadow(shadow) => Some(shadow),
            _ => None,
        })
        .expect("box-shadow lowers through the neutral primitive");
    assert_eq!((shadow.offset.x, shadow.offset.y), (2.0, 3.0));
    assert_eq!(shadow.blur_radius, 4.0);
    assert_eq!(shadow.border_radius.top_left.width, 6.0);
    assert_eq!(shadow.color.a, 128.0 / 255.0);
}

#[test]
fn hidden_visibility_keeps_layout_space_but_suppresses_paint() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="hidden"></div><div class="after"></div></body></html>"#,
    );
    let mut document = LiveryDocument::new(
        document,
        StyleSet::cambium(&[
            ".hidden { width: 40px; height: 30px; visibility: hidden; background-color: red; } \
             .after { width: 10px; height: 10px; background-color: lime; }",
        ]),
        Device::screen(320.0, 240.0),
    );
    let frame = document.frame(320, 240).unwrap();
    let green = frame
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0) => {
                Some(rect.placement.bounds.min.y)
            },
            _ => None,
        })
        .expect("visible following box paints");
    assert!(green >= 30.0);
    assert!(!frame.commands().iter().any(|command| {
        matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0))
    }));
}

#[test]
fn text_alignment_and_spacing_reach_parley() {
    fn first_glyph(css: &str) -> (f32, f32) {
        let list = render(
            r#"<html><body><div class="label">word word</div></body></html>"#,
            css,
            1,
        );
        let run = list
            .commands()
            .iter()
            .find_map(|command| match command {
                PaintCmd::DrawText(run) => Some(run),
                _ => None,
            })
            .expect("text run paints");
        let first = run.glyphs.first().expect("glyphs paint");
        let last = run.glyphs.last().expect("multiple glyphs paint");
        (first.point.x, last.point.x)
    }

    let start = first_glyph(".label { width: 200px; font-size: 16px; text-align: start; }");
    let centered = first_glyph(".label { width: 200px; font-size: 16px; text-align: center; }");
    let spaced = first_glyph(
        ".label { width: 200px; font-size: 16px; letter-spacing: 2px; word-spacing: 3px; }",
    );

    assert!(centered.0 > start.0 + 20.0);
    assert!(spaced.1 > start.1 + 8.0);
}

#[test]
fn positioned_children_paint_in_stable_z_index_order() {
    let list = render(
        r#"<html><body><div class="stage"><div class="high"></div><div class="normal"></div><div class="low"><div class="escape"></div></div><div class="negative"></div><div class="tie"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; background-color: black; }
        .stage > div { width: 20px; height: 20px; }
        .high { position: absolute; z-index: 2; background-color: blue; }
        .normal { z-index: 100; background-color: #ffff00; }
        .low { position: absolute; z-index: 1; background-color: red; }
        .escape { position: absolute; z-index: 999; width: 5px; height: 5px; background-color: #ff00ff; }
        .negative { position: absolute; z-index: -1; background-color: lime; }
        .tie { position: absolute; z-index: 2; background-color: #00ffff; }
        "#,
        1,
    );
    let colors = list
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawRect(rect) => Some(rect.color),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        colors,
        vec![
            ColorF::new(0.0, 0.0, 0.0, 1.0),
            ColorF::new(0.0, 1.0, 0.0, 1.0),
            ColorF::new(1.0, 1.0, 0.0, 1.0),
            ColorF::new(1.0, 0.0, 0.0, 1.0),
            ColorF::new(1.0, 0.0, 1.0, 1.0),
            ColorF::new(0.0, 0.0, 1.0, 1.0),
            ColorF::new(0.0, 1.0, 1.0, 1.0),
        ]
    );
}

#[test]
fn positioned_descendants_flatten_into_the_nearest_stacking_context() {
    let list = render(
        r#"<html><body><div class="stage"><div class="wrapper"><div class="highest"></div><div class="negative"></div></div><div class="middle"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; background-color: black; }
        .wrapper { width: 40px; height: 40px; background-color: #ffff00; }
        .highest { position: absolute; z-index: 5; width: 10px; height: 10px; background-color: #ff00ff; }
        .negative { position: absolute; z-index: -2; width: 10px; height: 10px; background-color: lime; }
        .middle { position: absolute; z-index: 2; width: 10px; height: 10px; background-color: blue; }
        "#,
        1,
    );
    let colors = list
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawRect(rect) => Some(rect.color),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        colors,
        vec![
            ColorF::new(0.0, 0.0, 0.0, 1.0),
            ColorF::new(0.0, 1.0, 0.0, 1.0),
            ColorF::new(1.0, 1.0, 0.0, 1.0),
            ColorF::new(0.0, 0.0, 1.0, 1.0),
            ColorF::new(1.0, 0.0, 1.0, 1.0),
        ]
    );
}

#[test]
fn flattened_positioned_descendants_replay_ancestor_clips() {
    let list = render(
        r#"<html><body><div class="stage"><div class="clipper"><div class="raised"></div></div><div class="middle"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; }
        .clipper {
            width: 20px; height: 20px;
            overflow-x: hidden; overflow-y: hidden;
            background-color: #ffff00;
        }
        .raised { position: absolute; z-index: 5; width: 40px; height: 40px; background-color: #ff00ff; }
        .middle { position: absolute; z-index: 2; width: 10px; height: 10px; background-color: blue; }
        "#,
        1,
    );
    let raised = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 1.0, 1.0)),
        )
        .expect("flattened descendant paints");
    let middle = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0)),
        )
        .expect("middle stacking item paints");

    assert!(middle < raised);
    assert!(matches!(list.commands()[raised - 1], PaintCmd::PushClip(_)));
    assert!(matches!(list.commands()[raised + 1], PaintCmd::PopClip));
}

#[test]
fn inline_positioned_contexts_retain_shaping_and_compete_by_z_index() {
    let list = render(
        r#"<html><body><div class="stage"><span class="wrapper"><span class="high">H</span><span class="normal">N</span><span class="atom"></span></span><div class="middle"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; }
        .high { position: relative; z-index: 5; color: #ff00ff; }
        .normal { color: #010101; }
        .atom {
            display: inline-block; position: relative; z-index: 1;
            width: 8px; height: 8px; background-color: red;
        }
        .middle {
            position: absolute; z-index: 2;
            width: 10px; height: 10px; background-color: blue;
        }
        "#,
        1,
    );
    let command_index =
        |predicate: &dyn Fn(&PaintCmd) -> bool| list.commands().iter().position(predicate).unwrap();
    let normal = command_index(
        &|command| matches!(command, PaintCmd::DrawText(run) if run.color == ColorF::new(1.0 / 255.0, 1.0 / 255.0, 1.0 / 255.0, 1.0)),
    );
    let atom = command_index(
        &|command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0)),
    );
    let middle = command_index(
        &|command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0)),
    );
    let high = command_index(
        &|command| matches!(command, PaintCmd::DrawText(run) if run.color == ColorF::new(1.0, 0.0, 1.0, 1.0)),
    );

    assert!(normal < atom && atom < middle && middle < high);
}

#[test]
fn opacity_creates_an_atomic_level_zero_context_and_compositing_layer() {
    let list = render(
        r#"<html><body><div class="stage"><div class="faded"><div class="escape"></div></div><div class="middle"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; }
        .faded { opacity: 0.5; width: 20px; height: 20px; background-color: red; }
        .escape {
            position: absolute; z-index: 999;
            width: 5px; height: 5px; background-color: #ff00ff;
        }
        .middle {
            position: absolute; z-index: 2;
            width: 10px; height: 10px; background-color: blue;
        }
        "#,
        1,
    );
    let push = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::PushLayer(_)))
        .expect("opacity opens a compositing layer");
    let faded = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0)),
        )
        .expect("opacity context paints its background");
    let escape = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 1.0, 1.0)),
        )
        .expect("high descendant remains inside opacity context");
    let pop = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::PopLayer))
        .expect("opacity closes its compositing layer");
    let middle = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0)),
        )
        .expect("positive sibling context paints");

    assert!(push < faded && faded < escape && escape < pop && pop < middle);
    let PaintCmd::PushLayer(layer) = &list.commands()[push] else {
        unreachable!()
    };
    assert_eq!(layer.opacity, 0.5);
}

#[test]
fn transform_creates_an_atomic_level_zero_coordinate_space() {
    let list = render(
        r#"<html><body><div class="stage"><div class="moved"><div class="escape"></div></div><div class="middle"></div></div></body></html>"#,
        r#"
        .stage { position: relative; z-index: 0; width: 100px; height: 100px; }
        .moved {
            transform: translate(12px, 4px) rotate(0deg);
            width: 20px; height: 20px; background-color: red;
        }
        .escape {
            position: absolute; z-index: 999;
            width: 5px; height: 5px; background-color: #ff00ff;
        }
        .middle {
            position: absolute; z-index: 2;
            width: 10px; height: 10px; background-color: blue;
        }
        "#,
        1,
    );
    let push = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::PushTransform(_)))
        .expect("transform opens a coordinate space");
    let moved = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0)),
        )
        .expect("transformed context paints its background");
    let escape = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 1.0, 1.0)),
        )
        .expect("high descendant remains inside transform context");
    let pop = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::PopTransform))
        .expect("transform closes its coordinate space");
    let middle = list
        .commands()
        .iter()
        .position(
            |command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0)),
        )
        .expect("positive sibling context paints");

    assert!(push < moved && moved < escape && escape < pop && pop < middle);
    assert!(
        !list
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(_))),
        "a transform changes coordinates without allocating an opacity layer"
    );
    let PaintCmd::PushTransform(spec) = &list.commands()[push] else {
        unreachable!()
    };
    assert!((spec.origin.x + spec.transform.m41 - 12.0).abs() < 0.001);
    assert!((spec.origin.y + spec.transform.m42 - 4.0).abs() < 0.001);
}

#[test]
fn transform_wraps_opacity_layer_in_coordinate_space() {
    let list = render(
        r#"<html><body><div class="box"></div></body></html>"#,
        r#"
        .box {
            opacity: 0.5; transform: scale(1.25);
            width: 20px; height: 20px; background-color: red;
        }
        "#,
        1,
    );
    assert!(matches!(list.commands()[0], PaintCmd::PushTransform(_)));
    assert!(matches!(list.commands()[1], PaintCmd::PushLayer(_)));
    assert!(matches!(list.commands()[2], PaintCmd::DrawRect(_)));
    assert!(matches!(list.commands()[3], PaintCmd::PopLayer));
    assert!(matches!(list.commands()[4], PaintCmd::PopTransform));
}

#[test]
fn overflow_clips_wrap_descendants_and_nest() {
    let list = render(
        r#"<html><body><div class="outer"><div class="inner"><div class="grand"></div></div></div></body></html>"#,
        r#"
        .outer {
            width: 40px; height: 20px; padding: 3px;
            border: 2px solid black; background-color: red;
            overflow-x: hidden; overflow-y: hidden;
        }
        .inner {
            width: 80px; height: 40px; background-color: blue;
            overflow-x: clip; overflow-y: clip;
        }
        .grand { width: 100px; height: 60px; background-color: lime; }
        "#,
        1,
    );
    let command_index =
        |predicate: &dyn Fn(&PaintCmd) -> bool| list.commands().iter().position(predicate).unwrap();
    let outer_index = command_index(
        &|command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0)),
    );
    let inner_index = command_index(
        &|command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0)),
    );
    let grand_index = command_index(
        &|command| matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0)),
    );
    let pushes = list
        .commands()
        .iter()
        .enumerate()
        .filter_map(|(index, command)| matches!(command, PaintCmd::PushClip(_)).then_some(index))
        .collect::<Vec<_>>();
    let pops = list
        .commands()
        .iter()
        .enumerate()
        .filter_map(|(index, command)| matches!(command, PaintCmd::PopClip).then_some(index))
        .collect::<Vec<_>>();

    assert_eq!((pushes.len(), pops.len()), (2, 2));
    assert!(outer_index < pushes[0]);
    assert!(pushes[0] < inner_index && inner_index < pushes[1]);
    assert!(pushes[1] < grand_index && grand_index < pops[0]);
    assert!(pops[0] < pops[1]);

    let PaintCmd::DrawRect(outer) = &list.commands()[outer_index] else {
        unreachable!()
    };
    let PaintCmd::PushClip(outer_clip) = &list.commands()[pushes[0]] else {
        unreachable!()
    };
    let ClipKind::Rect(clip) = &outer_clip.kind else {
        panic!("overflow uses a rectangular clip")
    };
    assert_eq!(clip.min.x, outer.placement.bounds.min.x + 2.0);
    assert_eq!(clip.min.y, outer.placement.bounds.min.y + 2.0);
    assert_eq!(clip.max.x, outer.placement.bounds.max.x - 2.0);
    assert_eq!(clip.max.y, outer.placement.bounds.max.y - 2.0);
}

#[test]
fn overflow_clips_only_the_non_visible_axis() {
    let list = render(
        r#"<html><body><div class="outer"><div class="child"></div></div></body></html>"#,
        ".outer { width: 40px; height: 20px; overflow-x: hidden; overflow-y: visible; \
                  background-color: red; } \
         .child { width: 80px; height: 40px; background-color: blue; }",
        1,
    );
    let clip = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushClip(clip) => Some(clip),
            _ => None,
        })
        .expect("x overflow establishes a clip");
    let ClipKind::Rect(rect) = &clip.kind else {
        panic!("overflow uses a rectangular clip")
    };

    let outer = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0) => {
                Some(rect.placement.bounds)
            },
            _ => None,
        })
        .expect("outer box paints");
    assert_eq!((rect.min.x, rect.max.x), (outer.min.x, outer.max.x));
    assert_eq!((rect.min.y, rect.max.y), (0.0, 240.0));
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

    let red = ColorF::new(1.0, 0.0, 0.0, 1.0);
    let blue = ColorF::new(0.0, 0.0, 1.0, 1.0);
    assert!(colors.contains(&red));
    assert!(colors.contains(&blue));

    let runs = first
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawText(run) => Some(run),
            _ => None,
        })
        .collect::<Vec<_>>();
    let red_baseline = runs
        .iter()
        .find(|run| run.color == red)
        .and_then(|run| run.glyphs.first())
        .unwrap()
        .point
        .y;
    let blue_baseline = runs
        .iter()
        .find(|run| run.color == blue)
        .and_then(|run| run.glyphs.first())
        .unwrap()
        .point
        .y;
    assert!((red_baseline - blue_baseline).abs() < f32::EPSILON);

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

#[test]
fn retained_document_reuses_complete_frames_and_font_allocations() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="label">retained text</div></body></html>"#,
    );
    let mut document = LiveryDocument::new(
        document,
        StyleSet::cambium(&[".label { color: navy; font-size: 18px; width: 120px; }"]),
        Device::screen(320.0, 240.0),
    );

    let first = document.frame(320, 240).unwrap();
    let first_generation = document.generation();
    let first_shape_count = document.text_system().shape_count();
    let first_font = first.fonts().first().unwrap().data.clone();

    let cached = document.frame(320, 240).unwrap();
    assert_eq!(document.generation(), first_generation);
    assert_eq!(document.text_system().shape_count(), first_shape_count);
    assert!(Arc::ptr_eq(
        &first_font,
        &cached.fonts().first().unwrap().data
    ));

    let resized = document.frame(480, 240).unwrap();
    assert!(document.generation() > first_generation);
    assert!(document.text_system().shape_count() > first_shape_count);
    assert!(Arc::ptr_eq(
        &first_font,
        &resized.fonts().first().unwrap().data
    ));
    assert_eq!(document.text_system().retained_font_count(), 1);
}

#[test]
fn shaped_text_height_moves_the_following_block() {
    fn following_block_y(label_width: u32) -> f32 {
        let document = StaticDocument::parse(
            r#"<html><body><div class="label">one two three four five six seven eight</div><div class="after"></div></body></html>"#,
        );
        let css = format!(
            ".label {{ width: {label_width}px; font-size: 16px; line-height: 20px; }} \
             .after {{ width: 10px; height: 10px; background-color: lime; }}"
        );
        let mut document = LiveryDocument::new(
            document,
            StyleSet::cambium(&[&css]),
            Device::screen(320.0, 240.0),
        );
        document
            .frame(320, 240)
            .unwrap()
            .commands()
            .iter()
            .find_map(|command| match command {
                PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0) => {
                    Some(rect.placement.bounds.min.y)
                },
                _ => None,
            })
            .expect("following block paints")
    }

    let wide = following_block_y(240);
    let narrow = following_block_y(48);

    assert!(
        narrow >= wide + 40.0,
        "wrapped Parley lines must increase Taffy's parent height: wide={wide}, narrow={narrow}"
    );
}

#[test]
fn shared_inline_group_height_matches_its_painted_lines() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="label"><span class="all">one <em>two three</em><span class="badge"></span> four five six</span></div><div class="after"></div></body></html>"#,
    );
    let mut document = LiveryDocument::new(
        document,
        StyleSet::cambium(&[
            ".label { width: 72px; font-size: 16px; line-height: 20px; } \
             .all { background-color: lime; } em { color: blue; } \
             .badge { display: inline-block; width: 18px; height: 26px; } \
             .after { width: 10px; height: 10px; background-color: #ff00ff; }",
        ]),
        Device::screen(320.0, 240.0),
    );
    let frame = document.frame(320, 240).unwrap();
    let inline_bottom = frame
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0) => {
                Some(rect.placement.bounds.max.y)
            },
            _ => None,
        })
        .reduce(f32::max)
        .expect("the shared inline owner paints its Parley fragments");
    let following_top = frame
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 1.0, 1.0) => {
                Some(rect.placement.bounds.min.y)
            },
            _ => None,
        })
        .expect("the following block paints");

    assert!(
        (following_top - inline_bottom).abs() <= 0.5,
        "Taffy block flow must consume exactly the shared Parley group height: inline_bottom={inline_bottom}, following_top={following_top}"
    );
}

#[test]
fn collapsed_whitespace_crosses_inline_element_boundaries() {
    fn blue_origin(html: &str) -> f32 {
        render(
            html,
            ".label { color: red; font-size: 16px; width: 120px; } span { color: blue; }",
            1,
        )
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0) => {
                run.glyphs.first().map(|glyph| glyph.point.x)
            },
            _ => None,
        })
        .unwrap()
    }

    let joined =
        blue_origin(r#"<html><body><div class="label">a<span>b</span></div></body></html>"#);
    let spaced =
        blue_origin(r#"<html><body><div class="label">a <span>b</span></div></body></html>"#);

    assert!(spaced > joined);
}

#[test]
fn bidi_runs_paint_in_parley_visual_order() {
    let list = render(
        r#"<html><body><div class="label"><span>אב</span><em>גד</em></div></body></html>"#,
        ".label { width: 200px; font-size: 18px; } \
         span { color: red; background-color: lime; } \
         em { color: blue; background-color: #ffff00; }",
        1,
    );
    let runs = list
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawText(run)
                if run.color == ColorF::new(1.0, 0.0, 0.0, 1.0)
                    || run.color == ColorF::new(0.0, 0.0, 1.0, 1.0) =>
            {
                Some((run.color, run.glyphs.first().unwrap().point.x))
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].0, ColorF::new(0.0, 0.0, 1.0, 1.0));
    assert_eq!(runs[1].0, ColorF::new(1.0, 0.0, 0.0, 1.0));
    assert!(runs[0].1 < runs[1].1);

    let last_background = list
        .commands()
        .iter()
        .enumerate()
        .filter_map(|(index, command)| matches!(command, PaintCmd::DrawRect(_)).then_some(index))
        .max()
        .expect("both inline backgrounds paint");
    let first_text = list
        .commands()
        .iter()
        .position(|command| matches!(command, PaintCmd::DrawText(_)))
        .expect("visual text stream paints");
    assert!(last_background < first_text);
}

#[test]
fn inline_background_uses_the_shaped_line_fragment() {
    let list = render(
        r#"<html><body><div class="label">before <span>inside</span> after</div></body></html>"#,
        ".label { width: 240px; font-size: 18px; } span { color: blue; background-color: lime; margin-left: 10px; }",
        1,
    );
    let background = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0) => {
                Some(rect.placement.bounds)
            },
            _ => None,
        })
        .expect("the inline span paints its shaped fragment");
    let glyph = list
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0) => {
                run.glyphs.first()
            },
            _ => None,
        })
        .expect("the inline span emits its text");
    let background_index = list
        .commands()
        .iter()
        .position(|command| {
            matches!(
                command,
                PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0)
            )
        })
        .unwrap();
    let text_index = list
        .commands()
        .iter()
        .position(|command| {
            matches!(
                command,
                PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0)
            )
        })
        .unwrap();

    assert!(background.min.x <= glyph.point.x && glyph.point.x <= background.max.x);
    assert!(background.min.y <= glyph.point.y && glyph.point.y <= background.max.y);
    assert!(
        glyph.point.x - background.min.x <= 1.0,
        "inline margins consume advance without painting into the background: background={background:?} glyph={:?}",
        glyph.point.x
    );
    assert!(background_index < text_index);
}

#[test]
fn inline_horizontal_edges_occupy_text_advance() {
    fn trailing_origin(edges: &str) -> f32 {
        render(
            r#"<html><body><div class="label">a<span>b</span><em>c</em></div></body></html>"#,
            &format!(
                ".label {{ width: 200px; font-size: 16px; }} \
                 span {{ {edges} }} em {{ color: blue; }}"
            ),
            1,
        )
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0) => {
                run.glyphs.first().map(|glyph| glyph.point.x)
            },
            _ => None,
        })
        .expect("trailing inline text paints")
    }

    let plain = trailing_origin("");
    let decorated =
        trailing_origin("padding-left: 10px; padding-right: 10px; border: 2px solid lime;");
    let margined = trailing_origin("margin-left: 10px; margin-right: 10px;");

    assert!(
        decorated - plain >= 23.5,
        "inline padding and borders must consume advance: plain={plain}, decorated={decorated}"
    );
    assert!(
        margined - plain >= 19.5,
        "inline margins must consume advance: plain={plain}, margined={margined}"
    );
}

#[test]
fn wrapped_inline_borders_use_slice_edges() {
    let list = render(
        r#"<html><body><div class="label"><span>one two three four five six seven</span></div></body></html>"#,
        ".label { width: 52px; font-size: 16px; line-height: 20px; } \
         span { padding: 14px 3px; border: 2px solid lime; }",
        1,
    );
    let borders = list
        .commands()
        .iter()
        .filter_map(|command| match command {
            PaintCmd::DrawBorder(border) => Some(border.widths),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        borders.len() >= 3,
        "the inline must wrap across three fragments"
    );
    assert_eq!(borders.first().unwrap().left, 2.0);
    assert_eq!(borders.first().unwrap().right, 0.0);
    assert_eq!(borders.last().unwrap().left, 0.0);
    assert_eq!(borders.last().unwrap().right, 2.0);
    for border in &borders {
        assert_eq!((border.top, border.bottom), (2.0, 2.0));
    }
    for border in &borders[1..borders.len() - 1] {
        assert_eq!((border.left, border.right), (0.0, 0.0));
    }
}

#[test]
fn vertical_inline_edges_paint_outside_the_line_box() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="label"><span>text</span></div><div class="after"></div></body></html>"#,
    );
    let mut document = LiveryDocument::new(
        document,
        StyleSet::cambium(&[
            ".label { width: 120px; font-size: 16px; line-height: 20px; } \
             span { padding-top: 4px; padding-bottom: 6px; \
                    border: 2px solid lime; background-color: lime; } \
             .after { width: 10px; height: 10px; background-color: #ff00ff; }",
        ]),
        Device::screen(320.0, 240.0),
    );
    let frame = document.frame(320, 240).unwrap();
    let decoration = frame
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0) => {
                Some(rect.placement.bounds)
            },
            _ => None,
        })
        .expect("inline decoration paints");
    let following_top = frame
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 1.0, 1.0) => {
                Some(rect.placement.bounds.min.y)
            },
            _ => None,
        })
        .expect("following block paints");

    assert!(decoration.height() >= 33.5);
    assert!(
        following_top < decoration.max.y,
        "vertical inline edges are paint overflow, not line-height input"
    );
}

#[test]
fn wrapped_inline_spans_paint_one_fragment_per_line() {
    let list = render(
        r#"<html><body><div class="label"><span>one two three four five</span></div></body></html>"#,
        ".label { width: 48px; font-size: 16px; } span { background-color: lime; }",
        1,
    );
    let fragments = list
        .commands()
        .iter()
        .filter(|command| {
            matches!(
                command,
                PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 1.0, 0.0, 1.0)
            )
        })
        .count();

    assert!(
        fragments >= 2,
        "wrapped span should paint multiple line boxes"
    );
}

#[test]
fn inline_blocks_occupy_atomic_space_in_the_text_line() {
    fn trailing_text_origin(badge_width: u32) -> f32 {
        let css = format!(
            ".label {{ width: 200px; font-size: 16px; }} \
             .badge {{ display: inline-block; width: {badge_width}px; height: 10px; \
             background-color: lime; }} em {{ color: blue; }}"
        );
        render(
            r#"<html><body><div class="label">a<span class="badge"></span><em>b</em></div></body></html>"#,
            &css,
            1,
        )
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0) => {
                run.glyphs.first().map(|glyph| glyph.point.x)
            },
            _ => None,
        })
        .expect("trailing inline text is painted")
    }

    let without_badge = trailing_text_origin(0);
    let with_badge = trailing_text_origin(30);

    assert!(with_badge - without_badge > 29.0);
}
