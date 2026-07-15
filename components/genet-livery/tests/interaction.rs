use genet_livery::{
    ClickOutcome, InteractionStates, LiveryDocument, StyleSet, hit_test, layout, resolve_styles,
};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use livery::media::Device;
use paint_list_api::{ColorF, PaintCmd, PaintList};

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
