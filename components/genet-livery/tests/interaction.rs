use genet_livery::{
    ClickOutcome, InteractionStates, LiveryDocument, StyleSet, hit_test, layout, resolve_styles,
};
use genet_static_dom::StaticDocument;
use layout_dom_api::LayoutDom;
use livery::media::Device;
use paint_list_api::{PaintCmd, PaintList};

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
