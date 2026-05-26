/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Scripted tier — the reflector bridge (engine↔DOM, JS→DOM direction).
//!
//! A JS script, handed a **reflector** (a value carrying a `NodeId`), can mutate
//! the corresponding `serval-scripted-dom` node through a native callback: the
//! callback recovers the `NodeId` from the reflector and calls [`LayoutDomMut`].
//! This closes the JS→DOM half of the live-scripting loop (the DOM→layout half is
//! the next pass: draining `DomMutation` into serval-layout).
//!
//! Built on the engine-neutral `script-engine-api` contract (`NativeFn` +
//! `CallCx` + host data), implemented by `script-engine-nova`. The host DOM
//! reaches the callback through Nova host-defined data, not a `thread_local`. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.
//!
//! Native-only: Nova is 64-bit-bound, and JS is native-only by design (wasm ships
//! no JS). On wasm32 the scripted tier carries no engine.

#![cfg_attr(target_arch = "wasm32", allow(unused_crate_dependencies))]

use layout_dom_api::LayoutDomMut;
use serval_layout::{render, FragmentPlane};
use serval_scripted_dom::{NodeId, ScriptedDom};

/// The live incremental layout engine. Re-exported as the scripted
/// tier's relayout-on-mutation entry: a persistent cascade + layout
/// session that restyles attribute changes through Stylo invalidation
/// (skipping layout for paint-only changes) and splices structural
/// changes — one engine for both, superseding the earlier stateless
/// `relayout_incremental` splice. See `serval_layout::IncrementalLayout`
/// and `docs/2026-05-25_fine_grained_restyle_plan.md`.
pub use serval_layout::{Applied, IncrementalLayout};

/// Coarse relayout-on-mutation — the **correctness oracle**. Drain the DOM's
/// pending [`DomMutation`](layout_dom_api::DomMutation)s; if anything changed, re-run
/// the *whole* layout pipeline and return the fresh fragment plane. Correct by
/// construction (a full recompute can't be stale), so it is the ground truth the
/// incremental engine ([`IncrementalLayout`]) is diff-tested against. The live path
/// uses `IncrementalLayout`; this stays as the oracle. Engine-agnostic (DOM + layout
/// only), so it lives at the crate root, not the Nova module.
pub fn relayout_if_dirty(
    dom: &mut ScriptedDom,
    stylesheets: &[&str],
    width: f32,
    height: f32,
) -> Option<FragmentPlane<NodeId>> {
    let mut mutations = Vec::new();
    dom.drain_mutations(&mut mutations);
    if mutations.is_empty() {
        return None;
    }
    Some(render(dom, stylesheets, width, height))
}

// Incremental relayout is now `serval_layout::IncrementalLayout` (re-exported
// above) — a persistent cascade+layout session that handles both attribute
// restyle (via Stylo invalidation, skipping layout for paint-only changes) and
// structural splice, superseding the earlier stateless `relayout_incremental`
// here. `relayout_if_dirty` stays as the coarse oracle it's diff-tested against.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::LayoutDomMut;
    use script_engine_api::{CallCx, NativeFn, ScriptEngine, ScriptEngineLive};
    use script_engine_nova::NovaEngine;
    use serval_scripted_dom::{NodeId, ScriptedDom};

    /// The host DOM stashed in engine host data, recovered inside the callback.
    type HostDom = RefCell<ScriptedDom>;

    /// `setText(node, text)` — recover the `NodeId` off the reflector argument, read
    /// the text, and set it on the host DOM. Host state arrives through host-defined
    /// data (`CallCx::host_data`), not a `thread_local`.
    struct SetText;

    impl NativeFn<NovaEngine> for SetText {
        fn call(
            cx: &mut <NovaEngine as ScriptEngine>::CallCx<'_>,
        ) -> Result<<NovaEngine as ScriptEngine>::Value, <NovaEngine as ScriptEngine>::Error>
        {
            let node = cx.arg(0);
            let text = cx.arg(1);
            let Some(id) = cx.reflector_data(&node) else {
                return Ok(cx.undefined());
            };
            let text = cx.value_to_string(&text)?;
            if let Some(data) = cx.host_data() {
                if let Some(dom) = data.downcast_ref::<HostDom>() {
                    dom.borrow_mut().set_text(NodeId::from_raw(id as usize), &text);
                }
            }
            Ok(cx.undefined())
        }
    }

    /// Run `source` against an engine wired so JS can mutate `dom` through the `node`
    /// reflector (which reflects `reflect`).
    pub fn run_script(dom: Rc<RefCell<ScriptedDom>>, reflect: NodeId, source: &str) {
        let mut engine = NovaEngine::new().expect("NovaEngine");
        engine.set_host_data(dom);
        engine.set_function::<SetText>("setText", 2).expect("install setText");

        let reflector = engine.make_reflector(reflect.raw() as u64).expect("reflector");
        engine.set_global("node", &reflector).expect("install node");

        let _ = engine.eval(source);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use layout_dom_api::{DomMutation, LayoutDom, LocalName, Namespace, QualName};

        #[test]
        fn js_mutates_dom_through_reflector() {
            let dom = Rc::new(RefCell::new(ScriptedDom::new()));
            let div = {
                let mut d = dom.borrow_mut();
                let root = d.document();
                let div = d.create_element(QualName::new(
                    None,
                    Namespace::from(""),
                    LocalName::from("div"),
                ));
                d.append_child(root, div);
                let mut drained = Vec::new();
                d.drain_mutations(&mut drained); // clear the append
                div
            };

            // JS reaches the host DOM node via its reflector and mutates it.
            run_script(dom.clone(), div, "setText(node, 'hello from JS')");

            let mut d = dom.borrow_mut();
            assert_eq!(d.text(div), Some("hello from JS"));
            let mut muts = Vec::new();
            d.drain_mutations(&mut muts);
            assert!(matches!(
                muts.as_slice(),
                [DomMutation::CharacterDataChanged { .. }]
            ));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::run_script;

#[cfg(test)]
mod relayout_tests {
    use super::*;
    use layout_dom_api::{LayoutDom, LocalName, Namespace, QualName};

    fn html_el(local: &str) -> QualName {
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/1999/xhtml"),
            LocalName::from(local),
        )
    }

    /// The #2(a) oracle: mutating the DOM and re-running the full pipeline yields an
    /// updated layout — three stacked paragraphs are taller than one.
    #[test]
    fn coarse_relayout_reflects_mutation() {
        const SHEET: &[&str] = &["html, body, p { display: block; }"];

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let html = dom.create_element(html_el("html"));
        dom.append_child(root, html);
        let body = dom.create_element(html_el("body"));
        dom.append_child(html, body);
        let p = dom.create_element(html_el("p"));
        dom.append_child(body, p);
        let text = dom.create_text("Hi");
        dom.append_child(p, text);

        // Initial layout (drains the build mutations) — the single <p> is laid out.
        let frags1 = relayout_if_dirty(&mut dom, SHEET, 800.0, 600.0).expect("initial layout");
        assert!(frags1.rect_of(p).is_some(), "initial <p> laid out");

        // Mutate via innerHTML, then relayout: the three new paragraphs must stack
        // vertically — a deterministic signal that the relayout reflects the mutation.
        dom.set_inner_html(body, "<p>one</p><p>two</p><p>three</p>");
        let frags2 =
            relayout_if_dirty(&mut dom, SHEET, 800.0, 600.0).expect("relayout after mutation");

        let kids: Vec<_> = dom.dom_children(body).collect();
        assert_eq!(kids.len(), 3, "innerHTML produced three paragraphs");
        let ys: Vec<f32> = kids
            .iter()
            .map(|&k| frags2.rect_of(k).expect("paragraph laid out").location.y)
            .collect();
        assert!(
            ys[0] < ys[1] && ys[1] < ys[2],
            "paragraphs should stack vertically after relayout: {ys:?}",
        );

        // Gating: no mutation since the last relayout → None.
        assert!(relayout_if_dirty(&mut dom, SHEET, 800.0, 600.0).is_none());
    }

    /// #2(b) first scoped-execution check: laying out only `body`'s subtree (via the
    /// re-rooted `SubtreeView`) must reproduce the *relative interior* layout that the
    /// coarse full-document pass produces. This is the diff-test guarding scoped
    /// recompute against the coarse oracle (for the inheritance-neutral case).
    #[test]
    fn scoped_relayout_matches_coarse_interior() {
        const SHEET: &[&str] = &["html, body, p { display: block; }"];

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let html = dom.create_element(html_el("html"));
        dom.append_child(root, html);
        let body = dom.create_element(html_el("body"));
        dom.append_child(html, body);
        dom.set_inner_html(body, "<p>one</p><p>two</p><p>three</p>");

        let coarse = serval_layout::render(&dom, SHEET, 800.0, 600.0);
        let scoped = serval_layout::render_subtree(&dom, body, SHEET, 800.0, 600.0);

        let kids: Vec<_> = dom.dom_children(body).collect();
        assert_eq!(kids.len(), 3);
        let coarse_y: Vec<f32> = kids
            .iter()
            .map(|&k| coarse.rect_of(k).expect("coarse paragraph").location.y)
            .collect();
        let scoped_y: Vec<f32> = kids
            .iter()
            .map(|&k| scoped.rect_of(k).expect("scoped paragraph").location.y)
            .collect();

        // Relative stacking within the subtree must match (absolute origin differs:
        // scoped lays the subtree out at its own root).
        for i in 1..3 {
            let coarse_rel = coarse_y[i] - coarse_y[0];
            let scoped_rel = scoped_y[i] - scoped_y[0];
            assert!(
                (coarse_rel - scoped_rel).abs() < 0.5,
                "paragraph {i} relative offset: coarse={coarse_rel} scoped={scoped_rel}",
            );
        }
    }

    // The splice/absolute-position correctness check moved with the engine:
    // `serval_layout::incremental::tests::inner_html_replace_splices_matching_full`
    // (IncrementalLayout over the persistent StylePlane). `relayout_if_dirty`
    // (the coarse oracle) and the SubtreeView relative-geometry check above
    // stay here.
}
