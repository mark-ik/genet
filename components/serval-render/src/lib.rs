/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Genet's host render driver.
//!
//! This crate turns a [`layout_dom_api::LayoutDom`] into a
//! [`netrender::Scene`] and exposes spatial and accessibility queries against
//! the same layout. Reactive view construction and toolkit-specific custom
//! leaf assembly belong to consumers such as Cambium and Pelt.

pub mod a11y;
pub mod inspect;
pub mod render;

pub use a11y::accesskit_tree;
pub use inspect::{ContentReport, OutlineEntry, content_report};
pub use render::{
    TextCursor, caret_byte_at, caret_screen_rect, fragments_from_scripted_dom, hit_test_node,
    paint_list_from_scripted_dom, paint_list_from_session, scene_from_layout_dom,
    scene_from_scripted_dom, scene_from_session, scene_from_session_dom, soft_wrap_caret_byte,
};

#[cfg(test)]
mod tests {
    use layout_dom_api::{LayoutDom, LayoutDomMut, QualName};
    use serval_scripted_dom::ScriptedDom;

    use crate::{fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

    const SHEET: &[&str] = &["div { display: block; }"];

    fn html(local: &str) -> QualName {
        QualName::new(None, layout_dom_api::Namespace::from("http://www.w3.org/1999/xhtml"), local.into())
    }

    fn counter_dom(value: u32) -> (ScriptedDom, serval_scripted_dom::NodeId) {
        let mut dom = ScriptedDom::new();
        let document = dom.document();
        let counter = dom.create_element(html("div"));
        let text = dom.create_text(&value.to_string());
        dom.append_child(counter, text);
        dom.append_child(document, counter);
        (dom, counter)
    }

    fn render_ops_debug(html: &str, width: u32, height: u32) -> String {
        const BLOCK_SHEET: &[&str] = &[
            "html, body, div, p, h1 { display: block; }",
            "body { padding: 16px; }",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        dom.set_inner_html(root, html);
        let scene = scene_from_scripted_dom(
            &dom,
            BLOCK_SHEET,
            width,
            height,
            None,
            &Default::default(),
        );
        format!("{:?}", scene.ops)
    }

    #[test]
    fn cascade_is_deterministic_off_thread_and_concurrent() {
        const HTML: &str = "<style>p { color: rgb(20, 20, 20); } h1 { font-size: 30px; }</style>\
             <h1>Heading</h1><p>One paragraph of text.</p><p>Another paragraph here.</p>";

        let baseline = render_ops_debug(HTML, 420, 360);
        assert!(baseline.contains("GlyphRun"));

        let off_thread = std::thread::spawn(|| render_ops_debug(HTML, 420, 360))
            .join()
            .expect("off-thread cascade panicked");
        assert_eq!(off_thread, baseline);

        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| render_ops_debug(HTML, 420, 360)))
            .collect();
        for handle in handles {
            assert_eq!(handle.join().expect("concurrent cascade panicked"), baseline);
        }
    }

    #[test]
    fn scripted_dom_counter_renders_end_to_end() {
        let (mut dom, counter) = counter_dom(0);
        let text = dom.dom_children(counter).next().expect("counter text");

        for expected in 0..=3u32 {
            dom.set_text(text, &expected.to_string());
            let fragments = fragments_from_scripted_dom(&dom, SHEET, 800, 600);
            assert!(fragments.rect_of(counter).is_some());

            let scene = scene_from_scripted_dom(
                &dom,
                SHEET,
                800,
                600,
                None,
                &Default::default(),
            );
            assert_eq!(scene.viewport_width, 800);
            assert_eq!(scene.viewport_height, 600);
            assert!(!scene.ops.is_empty());
        }
    }

    #[test]
    fn hit_test_recovers_live_node_in_subtree() {
        let (dom, counter) = counter_dom(0);
        let hit = hit_test_node(&dom, SHEET, 800, 600, 5.0, 5.0, &Default::default())
            .expect("point inside counter should hit");
        assert!(hit == counter || dom.parent(hit) == Some(counter));
        assert!(
            hit_test_node(
                &dom,
                SHEET,
                800,
                600,
                10_000.0,
                10_000.0,
                &Default::default(),
            )
            .is_none()
        );
    }
}
