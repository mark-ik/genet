/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Scripted tier — the reflector bridge (engine↔DOM, JS→DOM direction).
//!
//! A JS script, handed a **reflector** (an `EmbedderObject` carrying a `NodeId`),
//! can mutate the corresponding `serval-scripted-dom` node through native
//! callbacks: the callback recovers the `NodeId` from the reflector and calls
//! [`LayoutDomMut`]. This closes the JS→DOM half of the live-scripting loop
//! (the DOM→layout half is the next pass: draining `DomMutation` into serval-layout).
//!
//! Native-only: the reflector/host-callback binding is engine-specific (Appendix A
//! in the script-engine plan), and JS is native-only by design (wasm ships no JS).
//!
//! Probe-grade caveat: the host DOM is reached through a `thread_local` (the
//! rakers pattern) rather than Nova host-defined data, and bindings are installed
//! at realm-init via a plain `fn` (hence the thread_locals). Cleaning this up — a
//! proper `script-runtime-api` host layer over Nova host-data — is follow-up work.

#![cfg_attr(target_arch = "wasm32", allow(unused_crate_dependencies))]

use std::hash::Hash;

use layout_dom_api::{LayoutDom, LayoutDomMut};
use serval_layout::{classify, coalesce, render, render_subtree, FragmentPlane};
use serval_scripted_dom::{NodeId, ScriptedDom};

/// Coarse relayout-on-mutation — the **#2(a) correctness oracle**. Drain the DOM's
/// pending [`DomMutation`](layout_dom_api::DomMutation)s; if anything changed, re-run
/// the *whole* layout pipeline and return the fresh fragment plane. Correct by
/// construction (a full recompute can't be stale), so it is the ground truth that
/// incremental invalidation (#2(b)) is diff-tested against. Engine-agnostic
/// (DOM + layout only), so it lives at the crate root, not the Nova module.
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

/// Incremental relayout (#2(b)): drain the DOM's mutations, plan the minimal
/// recompute roots (classify → coalesce), and for each root lay out only its
/// subtree, splicing the result into `prior` at the root's real document position.
///
/// Falls back to a correct full recompute when a subtree's outer size changes (its
/// ancestors would reflow) or the root wasn't previously laid out. Two known
/// boundaries (both deferred, both safe — they only affect *removed*/inherited
/// cases): (1) stale fragments for removed nodes linger until a full pass (the
/// mutation stream doesn't carry the old children of a `SubtreeReplaced`);
/// (2) the scoped cascade uses the default inherited context, not the root's real
/// ancestors' (the `SubtreeView` boundary). Diff-tested against the coarse oracle.
pub fn relayout_incremental<D>(
    dom: &mut D,
    prior: &FragmentPlane<D::NodeId>,
    stylesheets: &[&str],
    width: f32,
    height: f32,
) -> FragmentPlane<D::NodeId>
where
    D: LayoutDom + LayoutDomMut,
    D::NodeId: Copy + Eq + Hash + 'static,
{
    let mut mutations = Vec::new();
    dom.drain_mutations(&mut mutations);
    if mutations.is_empty() {
        return prior.clone();
    }
    let invalidations: Vec<_> = mutations.iter().map(classify).collect();
    let roots = coalesce(&invalidations, |id| dom.parent(id));

    let mut result = prior.clone();
    for inv in &roots {
        let root = inv.node();
        let Some(prior_root) = prior.rect_of(root).copied() else {
            return render(dom, stylesheets, width, height);
        };
        let scoped = render_subtree(dom, root, stylesheets, width, height);
        let Some(scoped_root) = scoped.rect_of(root).copied() else {
            return render(dom, stylesheets, width, height);
        };
        // Outer size change → ancestors would reflow → defer to the correct full pass.
        if (scoped_root.size.width - prior_root.size.width).abs() >= 0.5
            || (scoped_root.size.height - prior_root.size.height).abs() >= 0.5
        {
            return render(dom, stylesheets, width, height);
        }
        // Splice: translate the scoped subtree to the root's real document position.
        let dx = prior_root.location.x - scoped_root.location.x;
        let dy = prior_root.location.y - scoped_root.location.y;
        let mut subtree = Vec::new();
        collect_subtree(dom, root, &mut subtree);
        for node in subtree {
            if let Some(layout) = scoped.rect_of(node) {
                let mut translated = *layout;
                translated.location.x += dx;
                translated.location.y += dy;
                result.insert(node, translated);
            }
        }
    }
    result
}

fn collect_subtree<D: LayoutDom>(dom: &D, root: D::NodeId, out: &mut Vec<D::NodeId>) {
    out.push(root);
    for child in dom.dom_children(root) {
        collect_subtree(dom, child, out);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use layout_dom_api::LayoutDomMut;
    use nova_vm::{
        ecmascript::{
            Agent, AgentOptions, ArgumentsList, Behaviour, BuiltinFunctionArgs, DefaultHostHooks,
            EmbedderObject, GcAgent, InternalMethods, JsResult, Object, PropertyDescriptor,
            PropertyKey, String as JsString, Value, create_builtin_function, parse_script,
            script_evaluation,
        },
        engine::{Bindable, GcScope},
    };
    use serval_scripted_dom::{NodeId, ScriptedDom};

    thread_local! {
        // The host DOM the native callbacks mutate. Set for the duration of a script run.
        static HOST_DOM: RefCell<Option<Rc<RefCell<ScriptedDom>>>> = const { RefCell::new(None) };
        // The NodeId to expose to JS as the `node` reflector global.
        static REFLECT_NODE: Cell<u64> = const { Cell::new(0) };
    }

    /// `setText(reflector, text)` — recover the `NodeId` from the reflector and set
    /// the node's text on the host DOM.
    fn dom_set_text<'gc>(
        agent: &mut Agent,
        _this: Value,
        args: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let Value::EmbedderObject(reflector) = args[0] else {
            return Ok(Value::Undefined);
        };
        let node = NodeId::from_raw(reflector.embedder_data(agent) as usize);
        let text = args[1]
            .to_string(agent, gc)?
            .to_string_lossy(agent)
            .into_owned();
        HOST_DOM.with(|dom| {
            if let Some(dom) = dom.borrow().as_ref() {
                dom.borrow_mut().set_text(node, &text);
            }
        });
        Ok(Value::Undefined)
    }

    /// Realm-init: install the `setText` native function and a `node` reflector for
    /// the thread-local target node.
    fn install_dom_bindings(agent: &mut Agent, global: Object, mut gc: GcScope) {
        let set_text = create_builtin_function(
            agent,
            Behaviour::Regular(dom_set_text),
            BuiltinFunctionArgs::new(2, "setText"),
            gc.nogc(),
        );
        let set_text_key = PropertyKey::from_static_str(agent, "setText", gc.nogc());
        global
            .internal_define_own_property(
                agent,
                set_text_key.unbind(),
                PropertyDescriptor {
                    value: Some(set_text.unbind().into()),
                    ..Default::default()
                },
                gc.reborrow(),
            )
            .unwrap();

        let reflector = EmbedderObject::create_with_data(agent, REFLECT_NODE.with(Cell::get));
        let node_key = PropertyKey::from_static_str(agent, "node", gc.nogc());
        global
            .internal_define_own_property(
                agent,
                node_key.unbind(),
                PropertyDescriptor {
                    value: Some(Value::EmbedderObject(reflector).unbind()),
                    ..Default::default()
                },
                gc,
            )
            .unwrap();
    }

    /// Run `source` against a realm wired so JS can mutate `dom` through the `node`
    /// reflector (which reflects `reflect`).
    pub fn run_script(dom: Rc<RefCell<ScriptedDom>>, reflect: NodeId, source: &str) {
        REFLECT_NODE.with(|n| n.set(reflect.raw() as u64));
        HOST_DOM.with(|d| *d.borrow_mut() = Some(dom));

        let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
        let create_global_object: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> =
            None;
        let create_global_this_value: Option<
            for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>,
        > = None;
        let realm = agent.create_realm(
            create_global_object,
            create_global_this_value,
            Some(install_dom_bindings),
        );

        let src = source.to_string();
        agent.run_in_realm(&realm, |agent, mut gc| {
            let realm = agent.current_realm(gc.nogc());
            let source_text = JsString::from_string(agent, src, gc.nogc());
            let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
            let _ = script_evaluation(agent, script.unbind(), gc.reborrow());
        });

        HOST_DOM.with(|d| *d.borrow_mut() = None);
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

    /// #2(b) completion check: `relayout_incremental` (splice path) must reproduce
    /// the coarse full-recompute at ABSOLUTE positions for a size-stable mutation.
    #[test]
    fn incremental_relayout_matches_coarse_absolute() {
        const SHEET: &[&str] = &["html, body, p { display: block; }"];

        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let html = dom.create_element(html_el("html"));
        dom.append_child(root, html);
        let body = dom.create_element(html_el("body"));
        dom.append_child(html, body);
        let p0 = dom.create_element(html_el("p"));
        dom.append_child(body, p0);
        let hi = dom.create_text("Hi");
        dom.append_child(p0, hi);

        // Prior full layout, then clear the build mutations so the incremental pass
        // sees only the upcoming edit.
        let prior = serval_layout::render(&dom, SHEET, 800.0, 600.0);
        let mut cleared = Vec::new();
        dom.drain_mutations(&mut cleared);

        // Edit: replace body's content (body fills the viewport → outer size stable).
        dom.set_inner_html(body, "<p>one</p><p>two</p><p>three</p>");

        let incremental = relayout_incremental(&mut dom, &prior, SHEET, 800.0, 600.0);
        let coarse = serval_layout::render(&dom, SHEET, 800.0, 600.0); // oracle, post-edit

        // body's position unchanged, and the three new paragraphs match coarse at
        // absolute positions (the splice placed them at body's real origin).
        let cb = coarse.rect_of(body).expect("coarse body");
        let ib = incremental.rect_of(body).expect("incremental body");
        assert!((cb.location.y - ib.location.y).abs() < 0.5, "body y drifted");

        let kids: Vec<_> = dom.dom_children(body).collect();
        assert_eq!(kids.len(), 3);
        for &p in &kids {
            let c = coarse.rect_of(p).expect("coarse paragraph");
            let i = incremental.rect_of(p).expect("incremental paragraph");
            assert!(
                (c.location.x - i.location.x).abs() < 0.5
                    && (c.location.y - i.location.y).abs() < 0.5,
                "paragraph abs pos: coarse=({},{}) incremental=({},{})",
                c.location.x,
                c.location.y,
                i.location.x,
                i.location.y,
            );
        }
    }
}
