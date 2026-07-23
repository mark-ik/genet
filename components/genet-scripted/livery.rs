/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Livery's scripted CSSOM adapter.
//!
//! The JS runtime owns the live mutable DOM. Livery owns retained author rule
//! objects beside it, and resolves a style plane on demand for
//! `getComputedStyle`. This is intentionally a style/session bridge rather than
//! a second DOM copy: script mutations are visible to the next read immediately.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use genet_livery::{
    Device, IncrementalStyle, InteractionStates, RestyleStats, RuleMutationError, StyleSet,
    canonicalize_specified_value, layout,
};
use genet_scripted_dom::NodeId;
use script_engine_api::ScriptEngine;
use script_runtime_api::{
    ComputedStyleHandler, HostState, InlineStyleHandler, InlineStyleValueResult, Runtime,
    StyleSheetHandler, StyleSheetMutationError,
};

struct LiveryState {
    styles: StyleSet,
    device: Device,
    interactions: InteractionStates<NodeId>,
    session: IncrementalStyle<NodeId>,
    mutation_cursor: u64,
}

/// A retained Livery stylesheet session installed on one scripted runtime.
///
/// Keep this handle when the host needs to update the media device. The runtime
/// itself retains the handler state for JS reads and mutations.
#[derive(Clone)]
pub struct LiveryCssom {
    state: Rc<RefCell<LiveryState>>,
}

impl LiveryCssom {
    /// Install Livery as the runtime's `document.styleSheets` and
    /// `getComputedStyle` provider. `author_sheets` are ordered exactly as they
    /// appear in the document.
    pub fn install<E: ScriptEngine>(
        runtime: &mut Runtime<E>,
        author_sheets: &[&str],
        device: Device,
    ) -> Self {
        let state = Rc::new(RefCell::new(LiveryState {
            styles: StyleSet::cambium(author_sheets),
            device,
            interactions: InteractionStates::default(),
            session: IncrementalStyle::new(),
            mutation_cursor: 0,
        }));
        let host = Rc::downgrade(runtime.host());
        runtime.set_computed_style_handler(Box::new(LiveryComputedStyle {
            host,
            state: state.clone(),
        }));
        runtime.set_inline_style_handler(Box::new(LiveryInlineStyle));
        runtime.set_stylesheet_handler(Box::new(LiveryStyleSheets {
            state: state.clone(),
        }));
        Self { state }
    }

    /// Update the device used by media queries and computed-value resolution.
    pub fn set_viewport_size(&self, width: f32, height: f32) {
        let mut state = self.state.borrow_mut();
        state.device.viewport_width = width;
        state.device.viewport_height = height;
    }

    /// The retained generation stamp for one author sheet.
    pub fn generation(&self, sheet: usize) -> Option<u64> {
        self.state
            .borrow()
            .styles
            .author_sheets()
            .get(sheet)
            .map(|sheet| sheet.generation())
    }

    /// Work performed by the latest scripted computed-style read.
    pub fn last_restyle_stats(&self) -> RestyleStats {
        self.state.borrow().session.last_stats()
    }
}

struct LiveryInlineStyle;

impl InlineStyleHandler for LiveryInlineStyle {
    fn canonicalize(&self, property: &str, value: &str) -> InlineStyleValueResult {
        canonicalize_specified_value(property, value).map_or(
            InlineStyleValueResult::PassThrough,
            InlineStyleValueResult::Canonical,
        )
    }
}

struct LiveryComputedStyle {
    host: Weak<RefCell<HostState>>,
    state: Rc<RefCell<LiveryState>>,
}

impl ComputedStyleHandler for LiveryComputedStyle {
    fn computed_value(&self, node: u64, property: &str) -> Option<String> {
        let host = self.host.upgrade()?;
        let host = host.borrow();
        let (base, pending) = host.dom.pending_mutations();
        let end = base.saturating_add(pending.len() as u64);
        let mut state = self.state.borrow_mut();
        if state.mutation_cursor < base || state.mutation_cursor > end {
            state.session.invalidate();
            state.mutation_cursor = base;
        }
        let start = state.mutation_cursor.saturating_sub(base) as usize;
        let LiveryState {
            styles,
            device,
            interactions,
            session,
            ..
        } = &mut *state;
        session.update(&host.dom, styles, device, interactions, &pending[start..]);
        let node = NodeId::from_raw(node as usize);
        let used_size = (property.eq_ignore_ascii_case("width")
            || property.eq_ignore_ascii_case("height"))
        .then(|| {
            layout(
                &host.dom,
                session.styles(),
                device.viewport_width,
                device.viewport_height,
            )
            .ok()
            .and_then(|fragments| {
                fragments
                    .get(node)
                    .map(|fragment| (fragment.width, fragment.height))
            })
        })
        .flatten();
        let value = session
            .styles()
            .computed_style_with_used_size(node, property, used_size);
        state.mutation_cursor = end;
        value
    }
}

struct LiveryStyleSheets {
    state: Rc<RefCell<LiveryState>>,
}

impl StyleSheetHandler for LiveryStyleSheets {
    fn sheet_count(&self) -> usize {
        self.state.borrow().styles.author_sheets().len()
    }

    fn rule_count(&self, sheet: usize) -> Option<usize> {
        self.state
            .borrow()
            .styles
            .author_sheets()
            .get(sheet)
            .map(|sheet| sheet.items().len())
    }

    fn insert_rule(
        &self,
        sheet: usize,
        rule: &str,
        index: usize,
    ) -> Result<usize, StyleSheetMutationError> {
        self.state
            .borrow_mut()
            .styles
            .insert_author_rule(sheet, rule, index)
            .map_err(mutation_error)
    }

    fn delete_rule(&self, sheet: usize, index: usize) -> Result<(), StyleSheetMutationError> {
        self.state
            .borrow_mut()
            .styles
            .delete_author_rule(sheet, index)
            .map_err(mutation_error)
    }
}

fn mutation_error(error: RuleMutationError) -> StyleSheetMutationError {
    match error {
        RuleMutationError::IndexSize => StyleSheetMutationError::IndexSize,
        RuleMutationError::Syntax(message) => StyleSheetMutationError::Syntax(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genet_static_dom::StaticDocument;
    use layout_dom_api::LayoutDomMut;
    use script_engine_boa::BoaEngine;

    #[test]
    fn boa_reaches_livery_stylesheets_mutation_and_computed_values() {
        let mut runtime = Runtime::<BoaEngine>::new().expect("runtime");
        runtime.load_dom(&StaticDocument::parse(
            "<html><body><div id='card' class='card'></div></body></html>",
        ));
        let cssom = LiveryCssom::install(
            &mut runtime,
            &[".card { --accent: #ff0000; color: var(--accent); }"],
            Device::screen(800.0, 600.0),
        );
        let initial_generation = cssom.generation(0).expect("author sheet");

        runtime
            .eval(
                "var card = document.getElementById('card');\
                 var sheet = document.styleSheets[0];\
                 console.log(document.styleSheets.length + '|' + sheet.cssRules.length + '|' +\
                   getComputedStyle(card).color + '|' +\
                   getComputedStyle(card).getPropertyValue('--accent'));\
                 console.log(sheet.insertRule('.card { --accent: #0000ff; }', 1));\
                 console.log(sheet.cssRules.length + '|' + getComputedStyle(card).color + '|' +\
                   getComputedStyle(card).getPropertyValue('--accent'));\
                 try { sheet.insertRule('.bad {}', 9); } catch (e) { console.log(e.name); }\
                 try { sheet.insertRule('not a rule', 2); } catch (e) { console.log(e.name); }\
                 console.log(sheet.cssRules.length + '|' + getComputedStyle(card).color);\
                 sheet.deleteRule(1);\
                 console.log(sheet.cssRules.length + '|' + getComputedStyle(card).color);",
            )
            .expect("Livery CSSOM script");

        assert_eq!(
            runtime.host().borrow().console,
            vec![
                "1|1|#ff0000|#ff0000",
                "1",
                "2|#0000ff|#0000ff",
                "IndexSizeError",
                "SyntaxError",
                "2|#0000ff",
                "1|#ff0000",
            ],
        );
        assert_eq!(cssom.generation(0), Some(initial_generation + 2));
    }

    #[test]
    fn boa_canonicalizes_nested_calc_through_livery_inline_cssom() {
        let mut runtime = Runtime::<BoaEngine>::new().expect("runtime");
        runtime.load_dom(&StaticDocument::parse(
            "<html><body><div id='target'></div></body></html>",
        ));
        LiveryCssom::install(&mut runtime, &[], Device::screen(800.0, 600.0));

        runtime
            .eval(
                "var s = document.getElementById('target').style;\
                 var values = [\
                   'calc(20px + calc(80px))',\
                   'calc(calc(100px))',\
                   'calc(calc(2) * calc(50px))',\
                   'calc(calc(150px*2/3))',\
                   'calc(calc(2 * calc(calc(3)) + 4) * 10px)',\
                   'calc(50px + calc(40%))'\
                 ];\
                 for (var i = 0; i < values.length; i++) {\
                   s.left = values[i]; console.log(s.left);\
                 }\
                 s.border = 'calc(calc(10px)) solid pink';\
                 console.log(s.border);",
            )
            .expect("Livery inline CSSOM script");

        assert_eq!(
            runtime.host().borrow().console,
            vec![
                "calc(100px)",
                "calc(100px)",
                "calc(100px)",
                "calc(100px)",
                "calc(100px)",
                "calc(40% + 50px)",
                "calc(10px) solid pink",
            ]
        );
    }

    #[test]
    fn boa_resolves_nested_calc_widths_through_livery_layout() {
        let mut runtime = Runtime::<BoaEngine>::new().expect("runtime");
        runtime.load_dom(&StaticDocument::parse(
            "<html><body><div id='parent'>\
             <div id='div1'></div><div id='div2'></div>\
             <div id='div3'></div><div id='div4'></div>\
             </div></body></html>",
        ));
        LiveryCssom::install(
            &mut runtime,
            &["#parent { width: 200px; }\
                 #div1 { width: calc(calc(50px)); }\
                 #div2 { width: calc(calc(60%) - 20px); }\
                 #div3 { width: calc(calc(3 * 25%)); }\
                 #div4 { --width: calc(10% + 30px); width: calc(2 * var(--width)); }"],
            Device::screen(800.0, 600.0),
        );

        runtime
            .eval(
                "console.log(getComputedStyle(div1).width);\
                 console.log(getComputedStyle(div2).width);\
                 console.log(getComputedStyle(div3).width);\
                 console.log(getComputedStyle(div4).width);",
            )
            .expect("Livery used width script");

        assert_eq!(
            runtime.host().borrow().console,
            vec!["50px", "100px", "150px", "100px"]
        );
    }

    #[test]
    fn scripted_attribute_change_restyles_only_its_livery_subtree() {
        let mut runtime = Runtime::<BoaEngine>::new().expect("runtime");
        runtime.load_dom(&StaticDocument::parse(
            "<html><body>\
             <main id='branch' class='branch'><span id='leaf' class='leaf'></span></main>\
             <aside><span class='unrelated'></span></aside>\
             </body></html>",
        ));
        let cssom = LiveryCssom::install(
            &mut runtime,
            &[".branch.on .leaf { color: #0000ff; }"],
            Device::screen(800.0, 600.0),
        );

        runtime
            .eval(
                "var leaf = document.getElementById('leaf');\
                 console.log(getComputedStyle(leaf).color);\
                 document.getElementById('branch').className = 'branch on';\
                 console.log(getComputedStyle(leaf).color);",
            )
            .expect("scoped Livery restyle");

        assert_eq!(
            runtime.host().borrow().console,
            vec!["CanvasText", "#0000ff"]
        );
        let stats = cssom.last_restyle_stats();
        assert_eq!(stats.snapshots, 1);
        assert_eq!(stats.hints, 1);
        assert_eq!(stats.restyled_elements, 2);
        assert!(stats.restyled_elements < stats.total_elements);
        assert!(!stats.full_document);
    }

    #[test]
    fn scripted_style_read_recovers_when_layout_drained_an_unseen_batch() {
        let mut runtime = Runtime::<BoaEngine>::new().expect("runtime");
        runtime.load_dom(&StaticDocument::parse(
            "<html><body><main id='branch'><span id='leaf'></span></main></body></html>",
        ));
        let cssom = LiveryCssom::install(
            &mut runtime,
            &[".on span { color: #0000ff; }"],
            Device::screen(800.0, 600.0),
        );

        runtime
            .eval("console.log(getComputedStyle(document.getElementById('leaf')).color);")
            .expect("initial style read");
        runtime
            .eval("document.getElementById('branch').className = 'on';")
            .expect("mutation");
        let mut drained = Vec::new();
        runtime
            .host()
            .borrow_mut()
            .dom
            .drain_mutations(&mut drained);
        assert!(!drained.is_empty());
        runtime
            .eval("console.log(getComputedStyle(document.getElementById('leaf')).color);")
            .expect("style read after layout drain");

        assert_eq!(
            runtime.host().borrow().console,
            vec!["CanvasText", "#0000ff"]
        );
        assert!(cssom.last_restyle_stats().full_document);
    }
}
