use genet_livery::{
    ClickOutcome, InteractionStates, LiveryDocument, StyleSet, hit_test, layout, resolve_styles,
};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use livery::media::Device;
use paint_list_api::{BorderDetails, BorderStyle as PaintBorderStyle, ColorF, PaintCmd, PaintList};

#[test]
fn hit_test_skips_pointer_events_none_overlays() {
    let document = StaticDocument::parse(
        r#"<html><body><div class="stage"><a class="under" href="/under">under</a><div class="overlay"></div></div></body></html>"#,
    );
    let styles = StyleSet::cambium(&[r#"
        .stage { position: relative; width: 100px; height: 40px; }
        .under, .overlay { position: absolute; inset: 0; width: 100px; height: 40px; }
        .under { z-index: 1; }
        .overlay { z-index: 2; pointer-events: none; }
    "#]);
    let plane = resolve_styles(
        &document,
        &styles,
        &Device::screen(200.0, 100.0),
        &InteractionStates::default(),
    );
    let fragments = layout(&document, &plane, 200.0, 100.0).unwrap();
    let under = document
        .first_with_class(document.document(), "under")
        .unwrap();

    assert_eq!(
        hit_test(&document, &plane, &fragments, 10.0, 10.0),
        Some(under)
    );
}

#[test]
fn retained_document_routes_scroll_fragment_and_links() {
    let document = StaticDocument::parse(
        r##"<html><body>
            <a id="top" href="#target" class="link">top</a>
            <div class="spacer"></div>
            <div id="target" class="target">target</div>
        </body></html>"##,
    );
    let styles = StyleSet::cambium(&[r#"
        html, body { margin: 0; padding: 0; }
        .link, .target { display: block; width: 100px; height: 20px; }
        .spacer { height: 500px; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));

    retained.frame(200, 100).unwrap();
    assert!(retained.content_height(100) > 100);
    let links = retained.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].url, "#target");
    assert!(links[0].rect[1] >= 0.0);

    let [x, y, width, height] = links[0].rect;
    assert_eq!(
        retained.click_at(x + width / 2.0, y + height / 2.0),
        ClickOutcome::Scrolled
    );
    assert!(retained.scroll().1 > 0.0);
    retained.frame(200, 100).unwrap();
    assert!(retained.links()[0].rect[1] < 0.0);
}

#[test]
fn retained_document_focuses_controls() {
    let document =
        StaticDocument::parse(r#"<html><body><button class="focus">focus</button></body></html>"#);
    let button = document.first_tag(document.document(), "button").unwrap();
    let styles = StyleSet::cambium(&[".focus { display: block; width: 100px; height: 20px; }"]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();
    let y = (0..100)
        .map(|y| y as f32 + 0.5)
        .find(|y| retained.hit_test(20.0, *y) == Some(button))
        .expect("button has a retained hit rectangle");
    assert_eq!(retained.click_at(20.0, y), ClickOutcome::Focused);
}

#[test]
fn retained_opacity_clock_paints_intermediate_frames() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="fade">fade</div></body></html>"#);
    let fade = document
        .first_with_class(document.document(), "fade")
        .unwrap();
    let styles = StyleSet::cambium(&[".fade { display: block; width: 100px; height: 20px; }"]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();

    assert!(retained.animate_opacity(fade, 0.0, 1.0, 0.0, 100.0));
    assert!(!retained.settled());
    let initial = retained.frame(200, 100).unwrap();
    assert!(
        initial
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(layer) if layer.opacity == 0.0))
    );

    assert!(retained.pump(50.0));
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("mid-transition frame keeps a compositing layer");
    assert!((opacity - 0.5).abs() < 0.01);

    assert!(retained.pump(100.0));
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert!(
        !final_frame
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(_)))
    );
}

#[test]
fn css_transition_opacity_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="fade">fade</div></body></html>"#);
    let fade = document
        .first_with_class(document.document(), "fade")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .fade { display: block; width: 100px; height: 20px; opacity: 0;
                transition: opacity 100ms; }
        .fade:hover { opacity: 1; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();

    retained
        .interactions_mut()
        .set(fade, livery::selector::StatePseudoClass::Hover, true);
    let initial = retained.frame(200, 100).unwrap();
    assert!(!retained.settled());
    assert!(
        initial
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(layer) if layer.opacity == 0.0))
    );

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("CSS transition keeps a mid-frame layer");
    assert!((opacity - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert!(
        !final_frame
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(_)))
    );
}

#[test]
fn css_transition_all_animates_opacity_and_background_color() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="fade">fade</div></body></html>"#);
    let fade = document
        .first_with_class(document.document(), "fade")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .fade { display: block; width: 100px; height: 20px; opacity: 0;
                background-color: red; transition: all 100ms; }
        .fade:hover { opacity: 1; background-color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();

    retained
        .interactions_mut()
        .set(fade, livery::selector::StatePseudoClass::Hover, true);
    let initial = retained.frame(200, 100).unwrap();
    assert!(!retained.settled());
    assert!(
        initial
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(layer) if layer.opacity == 0.0))
    );
    let initial_color = initial.commands().iter().find_map(|command| match command {
        PaintCmd::DrawRect(rect) if rect.color == ColorF::new(1.0, 0.0, 0.0, 1.0) => {
            Some(rect.color)
        },
        _ => None,
    });
    assert_eq!(initial_color, Some(ColorF::new(1.0, 0.0, 0.0, 1.0)));

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("all transition keeps a mid-frame compositing layer");
    assert!((opacity - 0.5).abs() < 0.01);
    let middle_color = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect)
                if rect.color.r > 0.4
                    && rect.color.r < 0.6
                    && rect.color.b > 0.4
                    && rect.color.b < 0.6 =>
            {
                Some(rect.color)
            },
            _ => None,
        })
        .expect("all transition paints an interpolated background");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert!(
        !final_frame
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(_)))
    );
    assert!(final_frame.commands().iter().any(|command| {
        matches!(command, PaintCmd::DrawRect(rect) if rect.color == ColorF::new(0.0, 0.0, 1.0, 1.0))
    }));
}

#[test]
fn css_transition_explicit_pair_animates_opacity_and_background_color() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="fade">fade</div></body></html>"#);
    let fade = document
        .first_with_class(document.document(), "fade")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .fade { display: block; width: 100px; height: 20px; opacity: 0;
                background-color: red;
                transition: opacity 100ms, background-color 100ms; }
        .fade:hover { opacity: 1; background-color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();

    retained
        .interactions_mut()
        .set(fade, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("explicit pair keeps a mid-frame compositing layer");
    assert!((opacity - 0.5).abs() < 0.01);
    let middle_color = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::DrawRect(rect)
                if rect.color.r > 0.4
                    && rect.color.r < 0.6
                    && rect.color.b > 0.4
                    && rect.color.b < 0.6 =>
            {
                Some(rect.color)
            },
            _ => None,
        })
        .expect("explicit pair paints an interpolated background");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);
}

#[test]
fn css_transition_color_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="label">label</div></body></html>"#);
    let label = document
        .first_with_class(document.document(), "label")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .label { display: block; width: 100px; height: 20px; color: red;
                 transition: color 100ms; }
        .label:hover { color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let initial = retained.frame(200, 100).unwrap();
    assert!(initial.commands().iter().any(|command| {
        matches!(command, PaintCmd::DrawText(run) if run.color == ColorF::new(1.0, 0.0, 0.0, 1.0))
    }));

    retained
        .interactions_mut()
        .set(label, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_color = middle.commands().iter().find_map(|command| match command {
        PaintCmd::DrawText(run) => Some(run.color),
        _ => None,
    });
    let middle_color = middle_color.expect("color transition keeps a text run");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert!(final_frame.commands().iter().any(|command| {
        matches!(command, PaintCmd::DrawText(run) if run.color == ColorF::new(0.0, 0.0, 1.0, 1.0))
    }));
}

#[test]
fn css_transition_three_property_list_shares_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="label">label</div></body></html>"#);
    let label = document
        .first_with_class(document.document(), "label")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .label { display: block; width: 100px; height: 20px; opacity: 0;
                 background-color: red; color: red;
                 transition: opacity 100ms, background-color 100ms, color 100ms; }
        .label:hover { opacity: 1; background-color: blue; color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    retained.frame(200, 100).unwrap();

    retained
        .interactions_mut()
        .set(label, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("three-property transition keeps a compositing layer");
    assert!((opacity - 0.5).abs() < 0.01);
    let middle_color = middle.commands().iter().find_map(|command| match command {
        PaintCmd::DrawRect(rect)
            if rect.color.r > 0.4
                && rect.color.r < 0.6
                && rect.color.b > 0.4
                && rect.color.b < 0.6 =>
        {
            Some(rect.color)
        },
        _ => None,
    });
    let middle_color = middle_color.expect("three-property transition paints an interpolated fill");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);
    let middle_text = middle.commands().iter().find_map(|command| match command {
        PaintCmd::DrawText(run) => Some(run.color),
        _ => None,
    });
    let middle_text = middle_text.expect("three-property transition keeps a text run");
    assert!((middle_text.r - 0.5).abs() < 0.01);
    assert!((middle_text.b - 0.5).abs() < 0.01);
}

#[test]
fn css_transition_border_top_color_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px; border-top-width: 4px;
                border-top-style: solid; border-top-color: red;
                transition: border-top-color 100ms; }
        .card:hover { border-top-color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let initial = retained.frame(200, 100).unwrap();
    let border_color = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some(border.top.color),
                _ => None,
            },
            _ => None,
        })
    };
    assert_eq!(
        border_color(&initial),
        Some(ColorF::new(1.0, 0.0, 0.0, 1.0))
    );

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_color = border_color(&middle).expect("border transition keeps a border primitive");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(
        border_color(&final_frame),
        Some(ColorF::new(0.0, 0.0, 1.0, 1.0))
    );
}

#[test]
fn css_transition_border_bottom_color_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px; border-bottom-width: 4px;
                border-bottom-style: solid; border-bottom-color: red;
                transition: border-bottom-color 100ms; }
        .card:hover { border-bottom-color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let initial = retained.frame(200, 100).unwrap();
    let border_color = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some(border.bottom.color),
                _ => None,
            },
            _ => None,
        })
    };
    assert_eq!(
        border_color(&initial),
        Some(ColorF::new(1.0, 0.0, 0.0, 1.0))
    );

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_color = border_color(&middle).expect("border transition keeps a border primitive");
    assert!((middle_color.r - 0.5).abs() < 0.01);
    assert!((middle_color.b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(
        border_color(&final_frame),
        Some(ColorF::new(0.0, 0.0, 1.0, 1.0))
    );
}

#[test]
fn css_transition_border_left_and_right_colors_use_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px;
                border-left-width: 4px; border-left-style: solid; border-left-color: red;
                border-right-width: 4px; border-right-style: solid; border-right-color: red;
                transition: all 100ms; }
        .card:hover { border-left-color: blue; border-right-color: blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let initial = retained.frame(200, 100).unwrap();
    let border_colors = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some((border.left.color, border.right.color)),
                _ => None,
            },
            _ => None,
        })
    };
    let red = ColorF::new(1.0, 0.0, 0.0, 1.0);
    let blue = ColorF::new(0.0, 0.0, 1.0, 1.0);
    assert_eq!(border_colors(&initial), Some((red, red)));

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let (left, right) = border_colors(&middle).expect("side border transition keeps a border");
    for color in [left, right] {
        assert!((color.r - 0.5).abs() < 0.01);
        assert!((color.b - 0.5).abs() < 0.01);
    }

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(border_colors(&final_frame), Some((blue, blue)));
}

#[test]
fn css_transition_border_radius_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px;
                border: 4px solid red; border-radius: 0;
                transition: border-radius 100ms; }
        .card:hover { border-radius: 20px; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let radius = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some(border.radius.top_left.width),
                _ => None,
            },
            _ => None,
        })
    };
    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(radius(&initial), Some(0.0));

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_radius = radius(&middle).expect("radius transition keeps a border primitive");
    assert!((middle_radius - 10.0).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(radius(&final_frame), Some(20.0));
}

#[test]
fn css_transition_transform_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px;
                transform: translate(0px, 0px);
                transition: transform 100ms; }
        .card:hover { transform: translate(20px, 4px); }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let translation = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::PushTransform(spec) => Some((
                spec.origin.x + spec.transform.m41,
                spec.origin.y + spec.transform.m42,
            )),
            _ => None,
        })
    };
    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(translation(&initial), Some((0.0, 0.0)));

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_translation = translation(&middle).expect("transform transition opens a context");
    assert!((middle_translation.0 - 10.0).abs() < 0.01);
    assert!((middle_translation.1 - 2.0).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(translation(&final_frame), Some((20.0, 4.0)));
}

#[test]
fn css_transition_background_position_uses_the_retained_clock() {
    use base64::Engine as _;

    let blue = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
    let mut png = Vec::new();
    blue.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode test PNG");
    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    let document = StaticDocument::parse(r#"<html><body><div class="card"></div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[&format!(
        r#"
        body {{ margin: 0; }}
        .card {{ display: block; width: 80px; height: 40px;
                  background-repeat: no-repeat; background-position: left top;
                  background-image: url({data_uri});
                  transition: background-position 100ms; }}
        .card:hover {{ background-position: right bottom; }}
        "#
    )]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let image_position = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawImage(image) => Some(image.placement.bounds.min),
            _ => None,
        })
    };

    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(
        image_position(&initial),
        Some(paint_list_api::LayoutPoint::new(0.0, 0.0))
    );

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle_position = image_position(&middle).expect("position transition keeps an image");
    assert!((middle_position.x - 39.0).abs() < 0.01);
    assert!((middle_position.y - 18.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(
        image_position(&final_frame),
        Some(paint_list_api::LayoutPoint::new(78.0, 37.0))
    );
}

#[test]
fn css_transition_background_image_interpolates_gradient_stops() {
    let document = StaticDocument::parse(r#"<html><body><div class="card"></div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        body { margin: 0; }
        .card { display: block; width: 80px; height: 40px;
                background-image: linear-gradient(red, blue);
                transition: background-image 100ms; }
        .card:hover { background-image: linear-gradient(white, black); }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let gradient_stops = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawLinearGradient(item) => Some(
                item.gradient
                    .stops
                    .iter()
                    .map(|stop| stop.color)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
    };

    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(
        gradient_stops(&initial),
        Some(vec![
            ColorF::new(1.0, 0.0, 0.0, 1.0),
            ColorF::new(0.0, 0.0, 1.0, 1.0),
        ])
    );

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle = gradient_stops(&middle).expect("image transition keeps a gradient");
    assert!((middle[0].r - 1.0).abs() < 0.01);
    assert!((middle[0].g - 0.5).abs() < 0.01);
    assert!((middle[0].b - 0.5).abs() < 0.01);
    assert!((middle[1].r - 0.0).abs() < 0.01);
    assert!((middle[1].g - 0.0).abs() < 0.01);
    assert!((middle[1].b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(
        gradient_stops(&final_frame),
        Some(vec![
            ColorF::new(1.0, 1.0, 1.0, 1.0),
            ColorF::new(0.0, 0.0, 0.0, 1.0),
        ])
    );
}

#[test]
fn css_transition_box_shadow_uses_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px; background-color: white;
                box-shadow: 0 0 0 red; transition: box-shadow 100ms; }
        .card:hover { box-shadow: 20px 4px 10px blue; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let shadow = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawShadow(shadow) => Some((
                shadow.offset.x,
                shadow.offset.y,
                shadow.blur_radius,
                shadow.color,
            )),
            _ => None,
        })
    };

    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(
        shadow(&initial),
        Some((0.0, 0.0, 0.0, ColorF::new(1.0, 0.0, 0.0, 1.0)))
    );

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let (offset_x, offset_y, blur, color) = shadow(&middle).expect("shadow transition paints");
    assert!((offset_x - 10.0).abs() < 0.01);
    assert!((offset_y - 2.0).abs() < 0.01);
    assert!((blur - 5.0).abs() < 0.01);
    assert!((color.r - 0.5).abs() < 0.01);
    assert!((color.b - 0.5).abs() < 0.01);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(
        shadow(&final_frame),
        Some((20.0, 4.0, 10.0, ColorF::new(0.0, 0.0, 1.0, 1.0)))
    );
}

#[test]
fn css_transition_border_widths_use_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px; border: 2px solid red;
                transition: all 100ms; }
        .card:hover { border-top-width: 10px; border-right-width: 10px;
                      border-bottom-width: 10px; border-left-width: 10px; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let widths = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => Some(border.widths),
            _ => None,
        })
    };

    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(widths(&initial).map(|widths| widths.top), Some(2.0));
    assert_eq!(widths(&initial).map(|widths| widths.right), Some(2.0));
    assert_eq!(widths(&initial).map(|widths| widths.bottom), Some(2.0));
    assert_eq!(widths(&initial).map(|widths| widths.left), Some(2.0));

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let middle = widths(&middle).expect("width transition keeps a border primitive");
    assert_eq!(middle.top, 6.0);
    assert_eq!(middle.right, 6.0);
    assert_eq!(middle.bottom, 6.0);
    assert_eq!(middle.left, 6.0);

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    let final_widths = widths(&final_frame).expect("settled width transition paints");
    assert_eq!(final_widths.top, 10.0);
    assert_eq!(final_widths.right, 10.0);
    assert_eq!(final_widths.bottom, 10.0);
    assert_eq!(final_widths.left, 10.0);
}

#[test]
fn css_transition_border_styles_switches_at_the_retained_midpoint() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="card">card</div></body></html>"#);
    let card = document
        .first_with_class(document.document(), "card")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        .card { display: block; width: 100px; height: 20px; border: 4px solid red;
                transition: all 100ms; }
        .card:hover { border-top-style: dashed; border-right-style: dotted;
                      border-bottom-style: double; border-left-style: groove; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));
    let border_styles = |frame: &genet_livery::LiveryPaintList| {
        frame.commands().iter().find_map(|command| match command {
            PaintCmd::DrawBorder(border) => match &border.details {
                BorderDetails::Normal(border) => Some((
                    border.top.style,
                    border.right.style,
                    border.bottom.style,
                    border.left.style,
                )),
                _ => None,
            },
            _ => None,
        })
    };
    let solid = (
        PaintBorderStyle::Solid,
        PaintBorderStyle::Solid,
        PaintBorderStyle::Solid,
        PaintBorderStyle::Solid,
    );
    let target = (
        PaintBorderStyle::Dashed,
        PaintBorderStyle::Dotted,
        PaintBorderStyle::Double,
        PaintBorderStyle::Groove,
    );

    let initial = retained.frame(200, 100).unwrap();
    assert_eq!(border_styles(&initial), Some(solid));

    retained
        .interactions_mut()
        .set(card, livery::selector::StatePseudoClass::Hover, true);
    retained.frame(200, 100).unwrap();
    assert!(!retained.settled());

    retained.pump(25.0);
    let early = retained.frame(200, 100).unwrap();
    assert_eq!(border_styles(&early), Some(solid));

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    assert_eq!(border_styles(&middle), Some(target));

    retained.pump(100.0);
    assert!(retained.settled());
    let final_frame = retained.frame(200, 100).unwrap();
    assert_eq!(border_styles(&final_frame), Some(target));
}

#[test]
fn css_keyframes_opacity_use_the_retained_clock() {
    let document =
        StaticDocument::parse(r#"<html><body><div class="fade">fade</div></body></html>"#);
    let styles = StyleSet::cambium(&[r#"
        @keyframes fade-in {
            from { opacity: 0; }
            to { opacity: 1; }
        }
        .fade { display: block; width: 100px; height: 20px;
                animation: fade-in 100ms ease-in; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 100.0));

    let initial = retained.frame(200, 100).unwrap();
    assert!(!retained.settled());
    assert!(
        initial
            .commands()
            .iter()
            .any(|command| matches!(command, PaintCmd::PushLayer(layer) if layer.opacity == 0.0))
    );

    retained.pump(50.0);
    let middle = retained.frame(200, 100).unwrap();
    let opacity = middle
        .commands()
        .iter()
        .find_map(|command| match command {
            PaintCmd::PushLayer(layer) => Some(layer.opacity),
            _ => None,
        })
        .expect("mid-keyframe frame keeps a compositing layer");
    assert!(
        opacity > 0.25 && opacity < 0.4,
        "ease-in opacity: {opacity}"
    );

    retained.pump(100.0);
    assert!(retained.settled());
}

#[test]
fn nested_scroll_chains_into_paint_and_then_the_viewport() {
    let document = StaticDocument::parse(
        r#"<html><body>
            <div class="scroller"><div class="top">top</div><div class="bottom">bottom</div></div>
            <div class="tail">tail</div>
        </body></html>"#,
    );
    let scroller = document
        .first_with_class(document.document(), "scroller")
        .unwrap();
    let top = document
        .first_with_class(document.document(), "top")
        .unwrap();
    let bottom = document
        .first_with_class(document.document(), "bottom")
        .unwrap();
    let styles = StyleSet::cambium(&[r#"
        html, body { display: block; margin: 0; padding: 0; }
        .scroller { display: block; width: 100px; height: 100px;
                    overflow-x: scroll; overflow-y: scroll; }
        .top, .bottom { display: block; width: 100px; height: 250px; }
        .tail { display: block; width: 100px; height: 500px; }
    "#]);
    let mut retained = LiveryDocument::new(document, styles, Device::screen(200.0, 200.0));
    retained.frame(200, 200).unwrap();
    assert_eq!(retained.hit_test(50.0, 50.0), Some(top));

    assert!(retained.scroll_at(50.0, 50.0, 0.0, 300.0));
    assert_eq!(
        retained.element_scroll().get(&scroller),
        Some(&(0.0, 300.0))
    );
    assert_eq!(retained.scroll(), (0.0, 0.0));
    assert_eq!(retained.hit_test(50.0, 50.0), Some(bottom));
    let frame = retained.frame(200, 200).unwrap();
    assert!(frame.commands().iter().any(|command| {
        matches!(command, PaintCmd::PushTransform(spec)
            if spec.origin.x == 0.0
                && spec.origin.y == 0.0
                && (spec.transform.m41 + 0.0).abs() < 0.01
                && (spec.transform.m42 + 300.0).abs() < 0.01)
    }));

    assert!(retained.scroll_at(50.0, 50.0, 0.0, 1_000.0));
    assert!(
        retained
            .element_scroll()
            .get(&scroller)
            .is_some_and(|offset| offset.1 > 399.0)
    );
    assert!(retained.scroll_at(50.0, 50.0, 0.0, 300.0));
    assert!(retained.scroll().1 > 0.0);
}
