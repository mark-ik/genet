//! Cambium's executable component acceptance surface.
//!
//! The view is a catalog a visual host can render. Running or testing the
//! example also exercises the same tree headlessly: semantic attributes,
//! keyboard and pointer interaction, action routing, grid virtualization, and
//! Sprigging's retained paint path all have assertions here.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    ActionItem, ActionListEvent, ActionListState, AnyView, DomHandle, GenetAppRunner, GenetCtx,
    GenetElement, GraphCanvasEdge, GraphCanvasNode, GraphCanvasSubgraph, GraphCanvasSwatch,
    GridColumn, GridSpec, GridView, HoverEvent, HoverPhase, Key, KeyEvent, NamedKey, PointerClick,
    RadioGroup, SelectState, Slider, StyleRange, TextInput, action_list, button, button_with,
    checkbox, custom_leaf, data_grid, el, graph_canvas_swatch, lens, map_action, menu, on_hover,
    radio_group, select, slider, styled_textarea, text_field_typed, textarea_typed, toggle,
};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{LayoutDom, LocalName, Namespace};
use sprigging::{
    ColorF, GraphGlyph, GraphGlyphNode, Knob, LeafRegistry, Meter, RenderedLeaves, Size, Swatch,
};

const THEME: &str = include_str!("component_catalog.css");

const SWATCH_KEY: u64 = 101;
const GRAPH_KEY: u64 = 102;
const METER_KEY: u64 = 103;
const KNOB_KEY: u64 = 104;
const GRAPH_SWATCH_KEY: u64 = 105;

type CatalogView = Box<dyn AnyView<CatalogState, (), GenetCtx, GenetElement>>;
type CatalogLogic = fn(&CatalogState) -> CatalogView;
type CatalogRunner = GenetAppRunner<CatalogState, CatalogLogic, CatalogView, ()>;

struct CatalogState {
    checked: bool,
    toggled: bool,
    radio: RadioGroup,
    select: SelectState,
    slider: Slider,
    text: TextInput,
    multiline: TextInput,
    styled: TextInput,
    actions: ActionListState,
    last_action: String,
    menu_selected: usize,
    grid_scroll: f32,
    grid_sort: usize,
    grid_descending: bool,
    presses: usize,
    graph_presses: usize,
    hovered: bool,
    hover_moves: usize,
    graph_swatch_selected: u8,
    graph_swatch_hovered: Option<u8>,
    graph_swatch_expanded: bool,
}

impl Default for CatalogState {
    fn default() -> Self {
        Self {
            checked: false,
            toggled: false,
            radio: RadioGroup::new(0).with_label("Detail density"),
            select: SelectState::new(1).with_label("Rendering mode"),
            slider: Slider::new(0.35).with_steps(0.05, 0.2).with_label("Zoom"),
            text: TextInput::new("merecat"),
            multiline: TextInput::new("First line\nSecond line"),
            styled: TextInput::new("let answer = 42;"),
            actions: ActionListState::default()
                .with_label("Catalog actions")
                .with_id("catalog-actions"),
            last_action: "none".into(),
            menu_selected: 0,
            grid_scroll: 0.0,
            grid_sort: 0,
            grid_descending: false,
            presses: 0,
            graph_presses: 0,
            hovered: false,
            hover_moves: 0,
            graph_swatch_selected: 1,
            graph_swatch_hovered: None,
            graph_swatch_expanded: false,
        }
    }
}

fn graph_swatch(selected: u8, hovered: Option<u8>) -> GraphCanvasSwatch<u8, &'static str> {
    let mut swatch = GraphCanvasSwatch::new(
        GRAPH_SWATCH_KEY,
        GraphCanvasSubgraph {
            nodes: vec![
                GraphCanvasNode {
                    id: 1,
                    kind: "document",
                    position: (0.12, 0.52),
                    label: "Selected document".into(),
                },
                GraphCanvasNode {
                    id: 2,
                    kind: "person",
                    position: (0.53, 0.18),
                    label: "Related person".into(),
                },
                GraphCanvasNode {
                    id: 3,
                    kind: "place",
                    position: (0.88, 0.70),
                    label: "Related place".into(),
                },
            ],
            edges: vec![
                GraphCanvasEdge { from: 1, to: 2 },
                GraphCanvasEdge { from: 2, to: 3 },
                GraphCanvasEdge { from: 1, to: 3 },
            ],
        },
    )
    .with_size(260, 128)
    .with_label("Related nodes");
    swatch.selected = Some(selected);
    swatch.focus = Some(selected);
    swatch.hovered = hovered;
    swatch
}

fn graph_kind_color(kind: &&str) -> ColorF {
    match *kind {
        "document" => color(0.22, 0.41, 0.72),
        "person" => color(0.65, 0.35, 0.72),
        _ => color(0.25, 0.65, 0.45),
    }
}

fn action_items() -> Vec<ActionItem> {
    vec![
        ActionItem::new("Open graph").with_shortcut("Ctrl+O"),
        ActionItem::new("Unavailable action").disabled(true),
        ActionItem::new("Close tab").with_shortcut("Ctrl+W"),
    ]
}

fn grid_spec() -> GridSpec {
    GridSpec {
        columns: vec![
            GridColumn::new("Name", 150.0),
            GridColumn::new("Kind", 110.0),
            GridColumn::new("Status", 100.0),
        ],
        row_height: 24.0,
        header_height: 28.0,
        overscan: 2,
    }
}

fn grid(state: &CatalogState) -> GridView<CatalogState, ()> {
    let descending = state.grid_descending;
    data_grid(
        &grid_spec(),
        10_000,
        144.0,
        state.grid_scroll,
        move |row, column| {
            let visible_row = if descending { 9_999 - row } else { row };
            let value = match column {
                0 => format!("Node {visible_row}"),
                1 => "document".to_string(),
                _ => {
                    if visible_row % 2 == 0 {
                        "ready".to_string()
                    } else {
                        "syncing".to_string()
                    }
                }
            };
            Box::new(el::<_, CatalogState, ()>("span", value)) as GridView<CatalogState, ()>
        },
        |state: &mut CatalogState, column| {
            if state.grid_sort == column {
                state.grid_descending = !state.grid_descending;
            } else {
                state.grid_sort = column;
                state.grid_descending = false;
            }
        },
        |row| (row == 0).then(|| "grid-row-selected".to_string()),
    )
}

fn catalog(state: &CatalogState) -> CatalogView {
    let choices = ["Quiet", "Balanced", "Detailed"];

    let controls = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Controls").attr("class", "catalog-label"),
            el(
                "div",
                lens(
                    |value: &mut bool| checkbox(*value).attr("id", "catalog-checkbox"),
                    |state: &mut CatalogState| &mut state.checked,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |value: &mut bool| toggle(*value).attr("id", "catalog-switch"),
                    |state: &mut CatalogState| &mut state.toggled,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    move |value: &mut RadioGroup| radio_group(value, &choices),
                    |state: &mut CatalogState| &mut state.radio,
                ),
            )
            .attr("id", "catalog-radio")
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    move |value: &mut SelectState| select(value, &choices),
                    |state: &mut CatalogState| &mut state.select,
                ),
            )
            .attr("id", "catalog-select")
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |value: &mut Slider| slider(value),
                    |state: &mut CatalogState| &mut state.slider,
                ),
            )
            .attr("id", "catalog-slider")
            .attr("class", "catalog-row"),
            button("Apply", |state: &mut CatalogState, _: PointerClick| {
                state.presses += 1;
            })
            .attr("id", "catalog-apply")
            .attr("class", "catalog-button"),
            on_hover(
                el::<_, CatalogState, ()>("div", "Hover target")
                    .attr("id", "catalog-hover")
                    .attr(
                        "class",
                        if state.hovered {
                            "catalog-hover active"
                        } else {
                            "catalog-hover"
                        },
                    ),
                |state: &mut CatalogState, event: HoverEvent| match event.phase {
                    HoverPhase::Enter => state.hovered = true,
                    HoverPhase::Leave => state.hovered = false,
                    HoverPhase::Move => state.hover_moves += 1,
                },
            ),
        ),
    )
    .attr("id", "controls-section")
    .attr("class", "catalog-section");

    let editors = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Editors").attr("class", "catalog-label"),
            el(
                "label",
                (
                    "Single line",
                    lens(
                        |input: &mut TextInput| text_field_typed(input),
                        |state: &mut CatalogState| &mut state.text,
                    ),
                ),
            )
            .attr("id", "catalog-text")
            .attr("class", "catalog-row"),
            el(
                "label",
                (
                    "Multiline",
                    lens(
                        |input: &mut TextInput| textarea_typed(input),
                        |state: &mut CatalogState| &mut state.multiline,
                    ),
                ),
            )
            .attr("id", "catalog-textarea")
            .attr("class", "catalog-row"),
            el(
                "label",
                (
                    "Styled editor",
                    lens(
                        |input: &mut TextInput| {
                            styled_textarea(
                                input,
                                &[
                                    StyleRange {
                                        range: 0..3,
                                        class: "syntax-keyword".into(),
                                    },
                                    StyleRange {
                                        range: 13..15,
                                        class: "syntax-number".into(),
                                    },
                                ],
                            )
                        },
                        |state: &mut CatalogState| &mut state.styled,
                    ),
                ),
            )
            .attr("id", "catalog-styled-editor")
            .attr("class", "catalog-row"),
        ),
    )
    .attr("id", "editors-section")
    .attr("class", "catalog-section");

    let actions = map_action(
        lens(
            |action_state: &mut ActionListState| action_list(action_state, &action_items()),
            |state: &mut CatalogState| &mut state.actions,
        ),
        |state: &mut CatalogState, event: ActionListEvent| {
            state.last_action = match event {
                ActionListEvent::Activate(index) => format!("activate:{index}"),
                ActionListEvent::Dismiss => "dismiss".into(),
            };
        },
    );
    let navigation = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Actions and overlays").attr("class", "catalog-label"),
            el("div", actions)
                .attr("id", "catalog-action-list")
                .attr("class", "catalog-row"),
            menu(
                12.0,
                12.0,
                ["Inspect".into(), "Duplicate".into(), "Close".into()],
                state.menu_selected,
                |state: &mut CatalogState, index| state.menu_selected = index,
            ),
        ),
    )
    .attr("id", "navigation-section")
    .attr("class", "catalog-section catalog-overlay-stage");

    let data = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Virtualized data grid").attr("class", "catalog-label"),
            grid(state),
        ),
    )
    .attr("id", "data-section")
    .attr("class", "catalog-section");

    let leaves = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Sprigging leaves").attr("class", "catalog-label"),
            el(
                "div",
                custom_leaf::<CatalogState, ()>(SWATCH_KEY, 32, 32)
                    .attr("aria-label", "Color swatch"),
            )
            .attr("class", "catalog-leaf-card"),
            button_with(
                custom_leaf::<CatalogState, ()>(GRAPH_KEY, 48, 32),
                |state: &mut CatalogState, _: PointerClick| state.graph_presses += 1,
            )
            .attr("id", "catalog-graph-button")
            .attr("class", "catalog-leaf-card")
            .attr("aria-label", "Open graph"),
            el(
                "div",
                graph_canvas_swatch(
                    &graph_swatch(state.graph_swatch_selected, state.graph_swatch_hovered),
                    |state: &mut CatalogState, id| state.graph_swatch_selected = id,
                    |state: &mut CatalogState, id| state.graph_swatch_hovered = id,
                    |state: &mut CatalogState| state.graph_swatch_expanded = true,
                ),
            )
            .attr("id", "catalog-graph-swatch")
            .attr("class", "catalog-graph-swatch-card"),
            el(
                "div",
                custom_leaf::<CatalogState, ()>(METER_KEY, 96, 18).attr("aria-label", "Level"),
            )
            .attr("class", "catalog-leaf-card"),
            el(
                "div",
                custom_leaf::<CatalogState, ()>(KNOB_KEY, 48, 48).attr("aria-label", "Gain"),
            )
            .attr("class", "catalog-leaf-card"),
        ),
    )
    .attr("id", "leaves-section")
    .attr("class", "catalog-section catalog-leaf-grid");

    Box::new(
        el(
            "main",
            (
                el::<_, CatalogState, ()>("h1", "Cambium component catalog")
                    .attr("class", "catalog-title"),
                el::<_, CatalogState, ()>(
                    "p",
                    "Executable coverage for controls, editors, actions, data, and custom paint.",
                )
                .attr("class", "catalog-intro"),
                controls,
                editors,
                navigation,
                data,
                leaves,
            ),
        )
        .attr("class", "component-catalog"),
    )
}

fn color(r: f32, g: f32, b: f32) -> ColorF {
    ColorF { r, g, b, a: 1.0 }
}

fn catalog_leaves() -> LeafRegistry<u64> {
    let mut registry = LeafRegistry::new();
    registry.insert(
        SWATCH_KEY,
        Box::new(Swatch::new(
            color(0.22, 0.41, 0.72),
            Size {
                width: 32.0,
                height: 32.0,
            },
        )),
    );
    registry.insert(
        GRAPH_KEY,
        Box::new(GraphGlyph::new(
            vec![
                GraphGlyphNode {
                    x: 0.1,
                    y: 0.5,
                    color: color(0.22, 0.41, 0.72),
                },
                GraphGlyphNode {
                    x: 0.55,
                    y: 0.15,
                    color: color(0.65, 0.35, 0.72),
                },
                GraphGlyphNode {
                    x: 0.9,
                    y: 0.7,
                    color: color(0.25, 0.65, 0.45),
                },
            ],
            vec![(0, 1), (1, 2), (0, 2)],
            Size {
                width: 48.0,
                height: 32.0,
            },
        )),
    );
    registry.insert(
        GRAPH_SWATCH_KEY,
        Box::new(graph_swatch(1, None).paint_leaf(graph_kind_color)),
    );
    let mut meter = Meter::new(
        false,
        Size {
            width: 96.0,
            height: 18.0,
        },
    );
    meter.set_level(0.68, Some(0.82));
    registry.insert(METER_KEY, Box::new(meter));
    let mut knob = Knob::new(Size {
        width: 48.0,
        height: 48.0,
    });
    knob.set_value(0.62);
    registry.insert(KNOB_KEY, Box::new(knob));
    registry
}

fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
    dom.attribute(node, &Namespace::from(""), &LocalName::from(name))
}

fn has_class(dom: &ScriptedDom, node: NodeId, class: &str) -> bool {
    attr(dom, node, "class").is_some_and(|classes| {
        classes
            .split_whitespace()
            .any(|candidate| candidate == class)
    })
}

fn find_where(
    dom: &ScriptedDom,
    node: NodeId,
    predicate: &impl Fn(&ScriptedDom, NodeId) -> bool,
) -> Option<NodeId> {
    if predicate(dom, node) {
        return Some(node);
    }
    dom.dom_children(node)
        .find_map(|child| find_where(dom, child, predicate))
}

fn find_id(dom: &ScriptedDom, root: NodeId, id: &str) -> NodeId {
    find_where(dom, root, &|dom, node| attr(dom, node, "id") == Some(id))
        .unwrap_or_else(|| panic!("catalog node #{id} is missing"))
}

fn find_class(dom: &ScriptedDom, root: NodeId, class: &str) -> NodeId {
    find_where(dom, root, &|dom, node| has_class(dom, node, class))
        .unwrap_or_else(|| panic!("catalog class .{class} is missing"))
}

fn collect_class(dom: &ScriptedDom, node: NodeId, class: &str, out: &mut Vec<NodeId>) {
    if has_class(dom, node, class) {
        out.push(node);
    }
    for child in dom.dom_children(node) {
        collect_class(dom, child, class, out);
    }
}

fn collect_named(dom: &ScriptedDom, node: NodeId, name: &str, out: &mut Vec<NodeId>) {
    if dom
        .element_name(node)
        .is_some_and(|qualified| qualified.local.as_ref() == name)
    {
        out.push(node);
    }
    for child in dom.dom_children(node) {
        collect_named(dom, child, name, out);
    }
}

fn assert_attr(dom: &ScriptedDom, node: NodeId, name: &str, expected: &str) {
    assert_eq!(attr(dom, node, name), Some(expected), "attribute {name}");
}

fn assert_initial_surface(dom: &ScriptedDom, root: NodeId) {
    assert_eq!(
        dom.element_name(root).map(|name| name.local.to_string()),
        Some("main".to_string())
    );
    for section in [
        "controls-section",
        "editors-section",
        "navigation-section",
        "data-section",
        "leaves-section",
    ] {
        find_id(dom, root, section);
    }

    let checkbox = find_id(dom, root, "catalog-checkbox");
    assert_attr(dom, checkbox, "role", "checkbox");
    assert_attr(dom, checkbox, "aria-checked", "false");
    let switch = find_id(dom, root, "catalog-switch");
    assert_attr(dom, switch, "role", "switch");
    assert_attr(dom, switch, "aria-checked", "false");
    assert!(has_class(
        dom,
        find_id(dom, root, "catalog-hover"),
        "catalog-hover"
    ));

    let radio_root = find_id(dom, root, "catalog-radio");
    let radio_group = find_where(dom, radio_root, &|dom, node| {
        attr(dom, node, "role") == Some("radiogroup")
    })
    .expect("radio group semantics");
    assert_attr(dom, radio_group, "aria-label", "Detail density");

    let select_root = find_id(dom, root, "catalog-select");
    let select = find_where(dom, select_root, &|dom, node| {
        attr(dom, node, "role") == Some("combobox")
    })
    .expect("select semantics");
    assert_attr(dom, select, "aria-expanded", "false");

    let slider_root = find_id(dom, root, "catalog-slider");
    let slider = find_where(dom, slider_root, &|dom, node| {
        attr(dom, node, "role") == Some("slider")
    })
    .expect("slider semantics");
    assert_attr(dom, slider, "aria-label", "Zoom");
    assert_attr(dom, slider, "aria-valuenow", "0.35");

    let text_root = find_id(dom, root, "catalog-text");
    assert!(
        find_where(dom, text_root, &|dom, node| {
            dom.element_name(node)
                .is_some_and(|name| name.local.as_ref() == "input")
        })
        .is_some()
    );
    let textarea_root = find_id(dom, root, "catalog-textarea");
    assert!(
        find_where(dom, textarea_root, &|dom, node| {
            dom.element_name(node)
                .is_some_and(|name| name.local.as_ref() == "textarea")
        })
        .is_some()
    );
    find_class(
        dom,
        find_id(dom, root, "catalog-styled-editor"),
        "syntax-keyword",
    );

    let action_root = find_id(dom, root, "catalog-action-list");
    let action_combobox = find_where(dom, action_root, &|dom, node| {
        attr(dom, node, "role") == Some("combobox")
    })
    .expect("action list semantics");
    assert_attr(dom, action_combobox, "aria-autocomplete", "list");
    assert_attr(
        dom,
        action_combobox,
        "aria-controls",
        "catalog-actions-options",
    );

    let mut rows = Vec::new();
    let data_root = find_id(dom, root, "data-section");
    let grid = find_class(dom, data_root, "grid");
    assert_attr(dom, grid, "role", "grid");
    assert_attr(dom, grid, "aria-rowcount", "10001");
    assert_attr(dom, grid, "aria-colcount", "3");
    let header = find_class(dom, grid, "grid-header-cell");
    assert_attr(dom, header, "role", "columnheader");
    assert_attr(dom, header, "tabindex", "0");
    collect_class(dom, data_root, "grid-row", &mut rows);
    assert!(!rows.is_empty());
    assert!(rows.len() <= 12, "the 10k-row grid must stay virtualized");
    assert!(
        rows.iter()
            .all(|row| attr(dom, *row, "role") == Some("row"))
    );
    let cell = find_class(dom, grid, "grid-cell");
    assert_attr(dom, cell, "role", "gridcell");

    let mut leaves = Vec::new();
    collect_named(
        dom,
        find_id(dom, root, "leaves-section"),
        "custom-leaf",
        &mut leaves,
    );
    assert_eq!(leaves.len(), 5);
    assert!(THEME.contains("--catalog-accent"));
    assert!(THEME.contains(":focus-visible"));
}

fn run_interactions(runner: &mut CatalogRunner) {
    let root = runner.root();

    let checkbox = find_id(&runner.dom().borrow(), root, "catalog-checkbox");
    runner.dispatch_click(checkbox, PointerClick::at((4.0, 4.0)));
    assert!(runner.state().checked);

    let switch = find_id(&runner.dom().borrow(), root, "catalog-switch");
    runner.set_focus(Some(switch));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    assert!(runner.state().toggled);

    let selected_radio = find_where(&runner.dom().borrow(), root, &|dom, node| {
        attr(dom, node, "role") == Some("radio") && attr(dom, node, "aria-checked") == Some("true")
    })
    .expect("selected radio");
    runner.set_focus(Some(selected_radio));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert_eq!(runner.state().radio.selected, 1);

    let select_root = find_id(&runner.dom().borrow(), root, "catalog-select");
    let select = find_where(&runner.dom().borrow(), select_root, &|dom, node| {
        attr(dom, node, "role") == Some("combobox")
    })
    .expect("select");
    runner.set_focus(Some(select));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    assert_eq!(runner.state().select.selected, 2);
    assert!(runner.state().select.open);
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
    assert!(!runner.state().select.open);

    let slider_root = find_id(&runner.dom().borrow(), root, "catalog-slider");
    let slider = find_where(&runner.dom().borrow(), slider_root, &|dom, node| {
        attr(dom, node, "role") == Some("slider")
    })
    .expect("slider");
    runner.set_focus(Some(slider));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::PageUp)));
    assert!((runner.state().slider.value - 0.55).abs() < f32::EPSILON);

    let text_root = find_id(&runner.dom().borrow(), root, "catalog-text");
    let text = find_where(&runner.dom().borrow(), text_root, &|dom, node| {
        dom.element_name(node)
            .is_some_and(|name| name.local.as_ref() == "input")
    })
    .expect("single-line input");
    runner.set_focus(Some(text));
    runner.dispatch_key(KeyEvent::new(Key::Character("!".into())));
    assert_eq!(runner.state().text.text(), "merecat!");

    let actions_root = find_id(&runner.dom().borrow(), root, "catalog-action-list");
    let actions = find_where(&runner.dom().borrow(), actions_root, &|dom, node| {
        attr(dom, node, "role") == Some("combobox")
    })
    .expect("action list");
    runner.set_focus(Some(actions));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
    assert_eq!(runner.state().last_action, "activate:2");

    let apply = find_id(&runner.dom().borrow(), root, "catalog-apply");
    runner.dispatch_click(apply, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().presses, 1);

    let menu_row = find_class(&runner.dom().borrow(), root, "menu-row");
    runner.dispatch_click(menu_row, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().menu_selected, 1);

    let header = find_class(&runner.dom().borrow(), root, "grid-header-cell");
    runner.set_focus(Some(header));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
    assert!(runner.state().grid_descending);

    let graph_button = find_id(&runner.dom().borrow(), root, "catalog-graph-button");
    runner.dispatch_click(graph_button, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().graph_presses, 1);

    let graph_swatch = find_id(&runner.dom().borrow(), root, "catalog-graph-swatch");
    let related_person = find_where(&runner.dom().borrow(), graph_swatch, &|dom, node| {
        attr(dom, node, "aria-label") == Some("Related person")
    })
    .expect("interactive graph node");
    runner.dispatch_hover(
        related_person,
        HoverEvent::new(HoverPhase::Enter, (4.0, 4.0), (20.0, 20.0)),
    );
    assert_eq!(runner.state().graph_swatch_hovered, Some(2));
    runner.dispatch_click(related_person, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().graph_swatch_selected, 2);
    let expand = find_where(&runner.dom().borrow(), graph_swatch, &|dom, node| {
        attr(dom, node, "aria-label") == Some("Expand graph")
    })
    .expect("graph expand affordance");
    runner.dispatch_click(expand, PointerClick::at((4.0, 4.0)));
    assert!(runner.state().graph_swatch_expanded);

    let hover = find_id(&runner.dom().borrow(), root, "catalog-hover");
    runner.dispatch_hover(
        hover,
        HoverEvent::new(HoverPhase::Enter, (8.0, 8.0), (120.0, 32.0)),
    );
    assert!(runner.state().hovered);
    runner.dispatch_hover(
        hover,
        HoverEvent::new(HoverPhase::Move, (12.0, 8.0), (120.0, 32.0)),
    );
    assert_eq!(runner.state().hover_moves, 1);
    runner.dispatch_hover(
        hover,
        HoverEvent::new(HoverPhase::Leave, (122.0, 8.0), (120.0, 32.0)),
    );
    assert!(!runner.state().hovered);

    let dom = runner.dom();
    let dom = dom.borrow();
    assert_attr(
        &dom,
        find_id(&dom, root, "catalog-checkbox"),
        "aria-checked",
        "true",
    );
    assert_attr(
        &dom,
        find_id(&dom, root, "catalog-switch"),
        "aria-checked",
        "true",
    );
}

fn assert_leaf_pipeline() {
    fn size(key: u64) -> Option<Size> {
        match key {
            SWATCH_KEY => Some(Size {
                width: 32.0,
                height: 32.0,
            }),
            GRAPH_KEY => Some(Size {
                width: 48.0,
                height: 32.0,
            }),
            METER_KEY => Some(Size {
                width: 96.0,
                height: 18.0,
            }),
            KNOB_KEY => Some(Size {
                width: 48.0,
                height: 48.0,
            }),
            GRAPH_SWATCH_KEY => Some(Size {
                width: 260.0,
                height: 128.0,
            }),
            _ => None,
        }
    }

    let mut registry = catalog_leaves();
    for key in [SWATCH_KEY, GRAPH_KEY, METER_KEY, KNOB_KEY, GRAPH_SWATCH_KEY] {
        assert!(registry.contains(&key));
    }
    let mut rendered = RenderedLeaves::new();
    let painted = registry.render_into(size, &mut rendered);
    assert_eq!(painted, 5);
    assert_eq!(rendered.len(), 5);
    for key in [SWATCH_KEY, GRAPH_KEY, METER_KEY, KNOB_KEY, GRAPH_SWATCH_KEY] {
        assert!(
            rendered
                .get(key)
                .is_some_and(|commands| !commands.is_empty())
        );
    }
    assert_eq!(registry.render_into(size, &mut rendered), 0);
}

fn run_acceptance() {
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let mut runner = CatalogRunner::new(
        dom.clone(),
        catalog as CatalogLogic,
        CatalogState::default(),
    );
    assert_initial_surface(&dom.borrow(), runner.root());
    run_interactions(&mut runner);
    assert_leaf_pipeline();
}

fn main() {
    run_acceptance();
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_is_the_component_acceptance_surface() {
        super::run_acceptance();
    }
}
