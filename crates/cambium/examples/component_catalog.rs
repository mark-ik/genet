//! A small, headless catalog of Cambium's reusable controls.
//!
//! The companion stylesheet is the first real application-owned theme corpus
//! audited by Genet's native CSS engine plan. This example proves the themed
//! classes belong to live Cambium views rather than a detached CSS sample.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    DomHandle, GenetAppRunner, GenetCtx, GenetElement, PointerClick, RadioGroup, SelectState,
    Slider, TextInput, View, button, checkbox, el, lens, radio_group, select, slider, text_field,
    toggle,
};
use genet_scripted_dom::ScriptedDom;
use layout_dom_api::LayoutDom;

const THEME: &str = include_str!("component_catalog.css");

#[derive(Default)]
struct CatalogState {
    checked: bool,
    toggled: bool,
    radio: RadioGroup,
    select: SelectState,
    slider: Slider,
    text: TextInput,
    presses: usize,
}

fn catalog(
    _state: &CatalogState,
) -> impl View<CatalogState, (), GenetCtx, Element = GenetElement> + use<> {
    let choices = ["Quiet", "Balanced", "Detailed"];
    let controls = el::<_, CatalogState, ()>(
        "section",
        (
            el(
                "div",
                lens(
                    |value: &mut bool| checkbox(*value),
                    |s: &mut CatalogState| &mut s.checked,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |value: &mut bool| toggle(*value),
                    |s: &mut CatalogState| &mut s.toggled,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    move |value: &mut RadioGroup| radio_group(value, &choices),
                    |s: &mut CatalogState| &mut s.radio,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    move |value: &mut SelectState| select(value, &choices),
                    |s: &mut CatalogState| &mut s.select,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |value: &mut Slider| slider(value),
                    |s: &mut CatalogState| &mut s.slider,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |value: &mut TextInput| text_field(value),
                    |s: &mut CatalogState| &mut s.text,
                ),
            )
            .attr("class", "catalog-row"),
            button("Apply", |state: &mut CatalogState, _: PointerClick| {
                state.presses += 1
            })
            .attr("class", "catalog-button"),
        ),
    )
    .attr("class", "catalog-section");

    el(
        "main",
        (
            el::<_, CatalogState, ()>("h1", "Cambium component catalog")
                .attr("class", "catalog-title"),
            el::<_, CatalogState, ()>("div", "Controls").attr("class", "catalog-label"),
            controls,
        ),
    )
    .attr("class", "component-catalog")
}

fn main() {
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), catalog, CatalogState::default());
    let root = runner.root();
    let dom = dom.borrow();

    assert_eq!(
        dom.element_name(root).map(|name| name.local.to_string()),
        Some("main".to_string())
    );
    assert_eq!(dom.dom_children(root).count(), 3);
    assert!(THEME.contains(".component-catalog"));
    assert!(THEME.contains(".slider-thumb"));
}
