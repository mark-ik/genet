//! Cambium's executable component acceptance surface.
//!
//! The view is a catalog a visual host can render. Running or testing the
//! example also exercises the same tree headlessly: semantic attributes,
//! keyboard and pointer interaction, action routing, grid virtualization, and
//! Sprigging's retained paint path all have assertions here.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AccordionConfig, AccordionItem, AccordionState, AnyView, CommandEvent, CommandItem,
    CommandState, DetailPopoverMode, DetailPopoverState, DisclosureState, DomHandle,
    GenetAppRunner, GenetCtx, GenetElement, GraphCanvasEdge, GraphCanvasNode, GraphCanvasSubgraph,
    GraphCanvasSwatch, GridColumn, GridSpec, GridView, HoverEvent, HoverPhase, Key, KeyEvent,
    NamedKey, OverlayDismiss, OverlayRole, OverlaySurface, Placement, PointerClick, PointerEvent,
    PointerPhase, RadioGroup, ReorderItem, ReorderMove, ReorderState, SelectState, SelectionItem,
    SelectionState, Slider, StyleRange, SummaryBody, TabActivation, TextInput, TreeItem, TreeState,
    accordion_with, button, button_with, checkbox, command_menu, command_palette, command_picker,
    custom_leaf, data_grid, detail_popover, disclosure, el, filter_chips, graph_canvas_swatch,
    graph_canvas_swatch_with_focus, lens, map_action, on_hover, on_pointer, overlay_surface,
    radio_group, reorderable_list, segmented_control, select, slider, styled_textarea,
    summary_body, tab_bar, text_field_typed, textarea_typed, toggle, tree_view,
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
const GRAPH_EMPTY_KEY: u64 = 106;
const GRAPH_SINGLE_KEY: u64 = 107;
const GRAPH_CROWDED_KEY: u64 = 108;
const LEAF_KEYS: [u64; 8] = [
    SWATCH_KEY,
    GRAPH_KEY,
    METER_KEY,
    KNOB_KEY,
    GRAPH_SWATCH_KEY,
    GRAPH_EMPTY_KEY,
    GRAPH_SINGLE_KEY,
    GRAPH_CROWDED_KEY,
];

type CatalogView = Box<dyn AnyView<CatalogState, (), GenetCtx, GenetElement>>;
type CatalogLogic = fn(&CatalogState) -> CatalogView;
type CatalogRunner = GenetAppRunner<CatalogState, CatalogLogic, CatalogView, ()>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogWidth {
    Narrow,
    Regular,
}

impl CatalogWidth {
    const fn class(self) -> &'static str {
        match self {
            Self::Narrow => "component-catalog catalog-width-narrow",
            Self::Regular => "component-catalog catalog-width-regular",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Regular => "regular",
        }
    }

    const fn viewport(self) -> (u32, u32) {
        match self {
            Self::Narrow => (420, 5200),
            Self::Regular => (900, 3600),
        }
    }
}

struct CatalogState {
    width: CatalogWidth,
    checked: bool,
    toggled: bool,
    radio: RadioGroup,
    tabs: SelectionState,
    segments: SelectionState,
    chips: SelectionState,
    reorder: ReorderState<&'static str>,
    reorder_order: Vec<&'static str>,
    last_reorder: Option<ReorderMove<&'static str>>,
    disclosure: DisclosureState,
    accordion: AccordionState<&'static str>,
    tree: TreeState<&'static str>,
    select: SelectState,
    slider: Slider,
    text: TextInput,
    multiline: TextInput,
    styled: TextInput,
    actions: CommandState,
    last_action: String,
    picker_commands: CommandState,
    last_picker: String,
    menu_commands: CommandState,
    last_menu: String,
    grid_scroll: f32,
    grid_sort: usize,
    grid_descending: bool,
    presses: usize,
    graph_presses: usize,
    hovered: bool,
    hover_moves: usize,
    graph_swatch_selected: u8,
    graph_swatch_hovered: Option<u8>,
    graph_swatch_focused: Option<u8>,
    graph_swatch_expanded: bool,
    knob_value: f32,
    overlay_open: bool,
    overlay_inside_presses: usize,
    overlay_last_dismiss: Option<OverlayDismiss>,
    detail_popover: DetailPopoverState,
    detail_uses: usize,
}

impl Default for CatalogState {
    fn default() -> Self {
        Self {
            width: CatalogWidth::Regular,
            checked: false,
            toggled: false,
            radio: RadioGroup::new(0).with_label("Detail density"),
            tabs: SelectionState::single(0)
                .with_label("Related panel")
                .with_id("catalog-tabs"),
            segments: SelectionState::single(0)
                .with_label("Card density")
                .with_id("catalog-segments"),
            chips: SelectionState::multiple([0])
                .with_label("Visible node kinds")
                .with_id("catalog-chips"),
            reorder: ReorderState::new()
                .with_label("Related panel order")
                .with_id("catalog-reorder"),
            reorder_order: vec!["notes", "people", "places"],
            last_reorder: None,
            disclosure: DisclosureState::new("catalog-disclosure", "Node details"),
            accordion: AccordionState::new()
                .with_id("catalog-accordion")
                .with_label("Node record sections")
                .single(true)
                .with_expanded(["identity"]),
            tree: TreeState::new()
                .with_id("catalog-tree")
                .with_label("Workspace outline")
                .with_expanded(["workspace"])
                .with_selected("workspace"),
            select: SelectState::new(1).with_label("Rendering mode"),
            slider: Slider::new(0.35).with_steps(0.05, 0.2).with_label("Zoom"),
            text: TextInput::new("merecat"),
            multiline: TextInput::new("First line\nSecond line"),
            styled: TextInput::new("let answer = 42;"),
            actions: CommandState::default()
                .with_label("Catalog actions")
                .with_id("catalog-actions"),
            last_action: "none".into(),
            picker_commands: CommandState::default()
                .with_label("Open mode")
                .with_id("catalog-command-picker"),
            last_picker: "none".into(),
            menu_commands: CommandState::default()
                .with_label("Canvas commands")
                .with_id("catalog-command-menu"),
            last_menu: "none".into(),
            grid_scroll: 0.0,
            grid_sort: 0,
            grid_descending: false,
            presses: 0,
            graph_presses: 0,
            hovered: false,
            hover_moves: 0,
            graph_swatch_selected: 1,
            graph_swatch_hovered: None,
            graph_swatch_focused: None,
            graph_swatch_expanded: false,
            knob_value: 0.62,
            overlay_open: true,
            overlay_inside_presses: 0,
            overlay_last_dismiss: None,
            detail_popover: DetailPopoverState::default(),
            detail_uses: 0,
        }
    }
}

impl CatalogState {
    fn at_width(width: CatalogWidth) -> Self {
        Self {
            width,
            ..Self::default()
        }
    }
}

fn graph_swatch(
    selected: u8,
    hovered: Option<u8>,
    focused: Option<u8>,
) -> GraphCanvasSwatch<u8, &'static str> {
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
    swatch.focus = focused;
    swatch.hovered = hovered;
    swatch
}

fn empty_graph_swatch() -> GraphCanvasSwatch<u8, &'static str> {
    GraphCanvasSwatch::new(
        GRAPH_EMPTY_KEY,
        GraphCanvasSubgraph {
            nodes: Vec::new(),
            edges: Vec::new(),
        },
    )
    .with_size(220, 104)
    .with_label("Empty related graph")
}

fn single_graph_swatch() -> GraphCanvasSwatch<u8, &'static str> {
    GraphCanvasSwatch::new(
        GRAPH_SINGLE_KEY,
        GraphCanvasSubgraph {
            nodes: vec![GraphCanvasNode {
                id: 10,
                kind: "document",
                position: (0.5, 0.5),
                label: "Only node".into(),
            }],
            edges: Vec::new(),
        },
    )
    .with_size(220, 104)
    .with_label("Single-node related graph")
}

fn crowded_graph_swatch() -> GraphCanvasSwatch<u8, &'static str> {
    let nodes = (0..12)
        .map(|index| GraphCanvasNode {
            id: 20 + index,
            kind: match index % 3 {
                0 => "document",
                1 => "person",
                _ => "place",
            },
            position: (
                0.08 + (index % 4) as f32 * 0.28,
                0.12 + (index / 4) as f32 * 0.36,
            ),
            label: format!("Crowded node {}", index + 1),
        })
        .collect();
    let edges = (0..11)
        .map(|index| GraphCanvasEdge {
            from: 20 + index,
            to: 21 + index,
        })
        .chain((0..8).map(|index| GraphCanvasEdge {
            from: 20 + index,
            to: 24 + index,
        }))
        .collect();
    GraphCanvasSwatch::new(GRAPH_CROWDED_KEY, GraphCanvasSubgraph { nodes, edges })
        .with_size(300, 144)
        .with_label("Crowded related graph")
}

fn graph_kind_color(kind: &&str) -> ColorF {
    match *kind {
        "document" => color(0.22, 0.41, 0.72),
        "person" => color(0.65, 0.35, 0.72),
        _ => color(0.25, 0.65, 0.45),
    }
}

fn command_items() -> Vec<CommandItem> {
    vec![
        CommandItem::new("Open graph")
            .with_id("open-graph")
            .with_shortcut("Ctrl+O"),
        CommandItem::new("Unavailable action")
            .with_id("unavailable")
            .disabled_because("Connect a writable graph first"),
        CommandItem::new("Close tab")
            .with_id("close-tab")
            .with_shortcut("Ctrl+W"),
        CommandItem::new("Export").with_id("export").with_children([
            CommandItem::new("Plain text"),
            CommandItem::new("PDF").disabled_because("PDF export is not installed"),
            CommandItem::new("JSON"),
        ]),
    ]
}

fn tab_items() -> Vec<SelectionItem> {
    vec![
        SelectionItem::new("Overview")
            .with_id("overview")
            .controls("catalog-panel-overview"),
        SelectionItem::new("History")
            .with_id("history")
            .controls("catalog-panel-history")
            .disabled_because("History is still loading"),
        SelectionItem::new("Links")
            .with_id("links")
            .controls("catalog-panel-links"),
    ]
}

fn segment_items() -> Vec<SelectionItem> {
    vec![
        SelectionItem::new("Compact"),
        SelectionItem::new("Balanced"),
        SelectionItem::new("Wide").disabled_because("Wide needs a larger window"),
    ]
}

fn chip_items() -> Vec<SelectionItem> {
    vec![
        SelectionItem::new("Documents"),
        SelectionItem::new("People").disabled_because("People index is unavailable"),
        SelectionItem::new("Tags"),
    ]
}

fn reorder_items(order: &[&'static str]) -> Vec<ReorderItem<&'static str>> {
    order
        .iter()
        .map(|id| {
            let label = match *id {
                "notes" => "Notes",
                "people" => "People",
                _ => "Places",
            };
            ReorderItem::new(*id, label)
        })
        .collect()
}

fn apply_reorder(state: &mut CatalogState, movement: ReorderMove<&'static str>) {
    let from = state
        .reorder_order
        .iter()
        .position(|id| *id == movement.id)
        .expect("reorder identity remains in the application collection");
    let item = state.reorder_order.remove(from);
    let destination = movement.to.min(state.reorder_order.len());
    state.reorder_order.insert(destination, item);
    state.last_reorder = Some(movement);
}

fn disclosure_accordion_items() -> Vec<AccordionItem<&'static str>> {
    vec![
        AccordionItem::new("identity", "Identity", "Identity details"),
        AccordionItem::new("activity", "Activity", "Recent activity"),
    ]
}

fn catalog_tree_items() -> Vec<TreeItem<&'static str>> {
    vec![
        TreeItem::new("workspace", "Workspace").with_children([
            TreeItem::new("notes", "Notes"),
            TreeItem::new("people", "People").with_children([
                TreeItem::new("collaborators", "Collaborators"),
                TreeItem::new("authors", "Authors"),
            ]),
        ]),
        TreeItem::new("archive", "Archive"),
    ]
}

fn catalog_summary(id: impl Into<String>, title: impl Into<String>) -> SummaryBody {
    SummaryBody::new(id, title)
        .with_eyebrow("Document")
        .with_description("A quiet reusable body for the selected graph record.")
        .with_fact("Links", "12")
        .with_fact("Updated", "Today")
}

fn command_result(event: CommandEvent) -> String {
    match event {
        CommandEvent::Activate(path) => format!(
            "activate:{}",
            path.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("/")
        ),
        CommandEvent::Dismiss => "dismiss".into(),
    }
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

    let selected_tab = state.tabs.selected.first().copied().unwrap_or(0);
    let mut overview_panel = el::<_, CatalogState, ()>("div", "Overview of the selected node")
        .attr("id", "catalog-panel-overview")
        .attr("class", "catalog-tab-panel")
        .attr("role", "tabpanel")
        .attr("aria-labelledby", "catalog-tabs-item-overview");
    if selected_tab != 0 {
        overview_panel = overview_panel.attr("hidden", "true");
    }
    let mut history_panel = el::<_, CatalogState, ()>("div", "History for the selected node")
        .attr("id", "catalog-panel-history")
        .attr("class", "catalog-tab-panel")
        .attr("role", "tabpanel")
        .attr("aria-labelledby", "catalog-tabs-item-history");
    if selected_tab != 1 {
        history_panel = history_panel.attr("hidden", "true");
    }
    let mut links_panel = el::<_, CatalogState, ()>("div", "Links for the selected node")
        .attr("id", "catalog-panel-links")
        .attr("class", "catalog-tab-panel")
        .attr("role", "tabpanel")
        .attr("aria-labelledby", "catalog-tabs-item-links");
    if selected_tab != 2 {
        links_panel = links_panel.attr("hidden", "true");
    }
    let selection = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Selection bars").attr("class", "catalog-label"),
            el(
                "div",
                lens(
                    |state: &mut SelectionState| {
                        tab_bar(state, &tab_items(), TabActivation::Automatic)
                    },
                    |state: &mut CatalogState| &mut state.tabs,
                ),
            )
            .attr("class", "catalog-row"),
            (overview_panel, history_panel, links_panel),
            el(
                "div",
                lens(
                    |state: &mut SelectionState| segmented_control(state, &segment_items()),
                    |state: &mut CatalogState| &mut state.segments,
                ),
            )
            .attr("class", "catalog-row"),
            el(
                "div",
                lens(
                    |state: &mut SelectionState| filter_chips(state, &chip_items()),
                    |state: &mut CatalogState| &mut state.chips,
                ),
            )
            .attr("class", "catalog-row"),
        ),
    )
    .attr("id", "selection-section")
    .attr("class", "catalog-section");

    let current_reorder_items = reorder_items(&state.reorder_order);
    let reorder = map_action(
        lens(
            move |reorder: &mut ReorderState<&'static str>| {
                reorderable_list(reorder, &current_reorder_items)
            },
            |state: &mut CatalogState| &mut state.reorder,
        ),
        apply_reorder,
    );
    let reorder_section = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Reorderable list").attr("class", "catalog-label"),
            el::<_, CatalogState, ()>(
                "p",
                "Drag a row, or pick it up with Space and move it with the arrow keys.",
            )
            .attr("class", "catalog-note"),
            reorder,
        ),
    )
    .attr("id", "reorder-section")
    .attr("class", "catalog-section");

    let accordion_items = disclosure_accordion_items();
    let accordion = lens(
        move |accordion: &mut AccordionState<&'static str>| {
            accordion_with(
                accordion,
                &accordion_items,
                AccordionConfig::default().with_heading_level(3),
                |item| {
                    let summary = catalog_summary(
                        format!("catalog-accordion-summary-{}", item.dom_id),
                        item.label.clone(),
                    );
                    summary_body::<AccordionState<&'static str>, ()>(&summary)
                },
            )
        },
        |state: &mut CatalogState| &mut state.accordion,
    );
    let tree = lens(
        |tree: &mut TreeState<&'static str>| tree_view(tree, &catalog_tree_items()),
        |state: &mut CatalogState| &mut state.tree,
    );
    let card_summary = catalog_summary("catalog-summary-card-body", "Field notes");
    let row_summary = catalog_summary("catalog-summary-row-body", "Field notes");
    let disclosure_section = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Disclosure and summaries")
                .attr("class", "catalog-label"),
            el(
                "div",
                lens(
                    |disclosure_state: &mut DisclosureState| {
                        disclosure(disclosure_state, "Controlled detail content")
                    },
                    |state: &mut CatalogState| &mut state.disclosure,
                ),
            )
            .attr("class", "catalog-row"),
            accordion,
            tree,
            el("article", summary_body::<CatalogState, ()>(&card_summary))
                .attr("id", "catalog-summary-card")
                .attr("class", "catalog-summary-card"),
            el("div", summary_body::<CatalogState, ()>(&row_summary))
                .attr("id", "catalog-summary-row")
                .attr("class", "catalog-summary-row"),
        ),
    )
    .attr("id", "disclosure-section")
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
            |command_state: &mut CommandState| command_palette(command_state, &command_items()),
            |state: &mut CatalogState| &mut state.actions,
        ),
        |state: &mut CatalogState, event: CommandEvent| {
            state.last_action = command_result(event);
        },
    );
    let picker = map_action(
        lens(
            |command_state: &mut CommandState| command_picker(command_state, &command_items()),
            |state: &mut CatalogState| &mut state.picker_commands,
        ),
        |state: &mut CatalogState, event: CommandEvent| {
            state.last_picker = command_result(event);
        },
    );
    let context_menu = map_action(
        lens(
            |command_state: &mut CommandState| {
                command_menu(command_state, &command_items(), 12.0, 128.0)
            },
            |state: &mut CatalogState| &mut state.menu_commands,
        ),
        |state: &mut CatalogState, event: CommandEvent| {
            state.last_menu = command_result(event);
        },
    );
    let navigation = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "Actions and overlays").attr("class", "catalog-label"),
            el("div", actions)
                .attr("id", "catalog-action-list")
                .attr("class", "catalog-row"),
            el("div", picker).attr("class", "catalog-row"),
            context_menu,
            button("Open detail surface", |state: &mut CatalogState, _| {
                state.overlay_open = true;
                state.overlay_last_dismiss = None;
            })
            .attr("id", "catalog-overlay-open")
            .attr("class", "catalog-button"),
            state.overlay_open.then(|| {
                overlay_surface(
                    &OverlaySurface::new(
                        (260.0, 34.0, 120.0, 32.0),
                        (220.0, 104.0),
                        (0.0, 0.0, 640.0, 280.0),
                    )
                    .with_placement(Placement::Below)
                    .with_role(OverlayRole::Dialog)
                    .with_label("Catalog detail"),
                    el(
                        "div",
                        (
                            el::<_, CatalogState, ()>("strong", "Anchored detail"),
                            button("Use detail", |state: &mut CatalogState, _| {
                                state.overlay_inside_presses += 1;
                            })
                            .attr("id", "catalog-overlay-inside")
                            .attr("class", "catalog-button"),
                        ),
                    )
                    .attr("class", "catalog-overlay-content"),
                    |state: &mut CatalogState, reason| {
                        state.overlay_open = false;
                        state.overlay_last_dismiss = Some(reason);
                    },
                )
            }),
            detail_popover(
                state.detail_popover,
                &OverlaySurface::new(
                    (404.0, 246.0, 128.0, 32.0),
                    (208.0, 96.0),
                    (0.0, 0.0, 640.0, 280.0),
                )
                .with_placement(Placement::Below)
                .with_label("Marker detail"),
                "Marker details",
                el::<_, CatalogState, ()>(
                    "span",
                    "A short preview. Activate to pin the interactive detail.",
                ),
                (
                    el::<_, CatalogState, ()>("strong", "Pinned marker detail"),
                    button("Use marker", |state: &mut CatalogState, _| {
                        state.detail_uses += 1;
                    })
                    .attr("id", "catalog-detail-use")
                    .attr("class", "catalog-button"),
                ),
                |state: &mut CatalogState, event| state.detail_popover.apply(event),
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

    let overflow_rows: Vec<_> = (1..=8)
        .map(|index| {
            el::<_, CatalogState, ()>(
                "li",
                format!("Overflow row {index}: a deliberately long record label for clipping"),
            )
        })
        .collect();
    let dense_tokens: Vec<_> = ["Notes", "People", "Places", "Tags", "Sessions", "Archive"]
        .into_iter()
        .map(|label| el::<_, CatalogState, ()>("span", label).attr("class", "catalog-dense-token"))
        .collect();
    let states = el::<_, CatalogState, ()>(
        "section",
        (
            el::<_, CatalogState, ()>("h2", "State specimens").attr("class", "catalog-label"),
            el("div", "Unavailable while the graph is read-only")
                .attr("class", "catalog-state catalog-state-disabled")
                .attr("data-state", "disabled")
                .attr("aria-disabled", "true"),
            el("div", "No related records")
                .attr("class", "catalog-state catalog-state-empty")
                .attr("data-state", "empty")
                .attr("role", "status"),
            el("div", "The remote record could not be loaded")
                .attr("class", "catalog-state catalog-state-error")
                .attr("data-state", "error")
                .attr("role", "alert"),
            el("ul", overflow_rows)
                .attr("class", "catalog-state catalog-state-overflow")
                .attr("data-state", "overflow"),
            el("div", dense_tokens)
                .attr("class", "catalog-state catalog-state-dense")
                .attr("data-state", "dense"),
            el::<_, CatalogState, ()>(
                "button",
                "Open the related record with a label long enough to wrap across narrow cards",
            )
            .attr("class", "catalog-state catalog-state-long-label")
            .attr("data-state", "long-label")
            .attr("type", "button"),
        ),
    )
    .attr("id", "states-section")
    .attr("class", "catalog-section catalog-state-grid");

    let primary_graph = graph_swatch(
        state.graph_swatch_selected,
        state.graph_swatch_hovered,
        state.graph_swatch_focused,
    );
    let empty_graph = empty_graph_swatch();
    let single_graph = single_graph_swatch();
    let crowded_graph = crowded_graph_swatch();

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
                graph_canvas_swatch_with_focus(
                    &primary_graph,
                    |state: &mut CatalogState, id| state.graph_swatch_selected = id,
                    |state: &mut CatalogState, id| state.graph_swatch_hovered = id,
                    |state: &mut CatalogState, id| state.graph_swatch_focused = id,
                    |state: &mut CatalogState| state.graph_swatch_expanded = true,
                ),
            )
            .attr("id", "catalog-graph-swatch")
            .attr("class", "catalog-graph-swatch-card"),
            el(
                "div",
                graph_canvas_swatch(
                    &empty_graph,
                    |_: &mut CatalogState, _: u8| {},
                    |_: &mut CatalogState, _: Option<u8>| {},
                    |_: &mut CatalogState| {},
                ),
            )
            .attr("id", "catalog-graph-empty")
            .attr("class", "catalog-graph-variant"),
            el(
                "div",
                graph_canvas_swatch(
                    &single_graph,
                    |_: &mut CatalogState, _: u8| {},
                    |_: &mut CatalogState, _: Option<u8>| {},
                    |_: &mut CatalogState| {},
                ),
            )
            .attr("id", "catalog-graph-single")
            .attr("class", "catalog-graph-variant"),
            el(
                "div",
                graph_canvas_swatch(
                    &crowded_graph,
                    |_: &mut CatalogState, _: u8| {},
                    |_: &mut CatalogState, _: Option<u8>| {},
                    |_: &mut CatalogState| {},
                ),
            )
            .attr("id", "catalog-graph-crowded")
            .attr(
                "class",
                "catalog-graph-variant catalog-graph-variant-crowded",
            ),
            el(
                "div",
                custom_leaf::<CatalogState, ()>(METER_KEY, 96, 18).attr("aria-label", "Level"),
            )
            .attr("class", "catalog-leaf-card"),
            el(
                "div",
                (
                    on_pointer(
                        custom_leaf::<CatalogState, ()>(KNOB_KEY, 48, 48)
                            .attr("id", "catalog-knob")
                            .attr("aria-label", "Gain"),
                        |state: &mut CatalogState, event: PointerEvent| {
                            if event.phase != PointerPhase::Up && event.size.0 > 0.0 {
                                state.knob_value = (event.local.0 / event.size.0).clamp(0.0, 1.0);
                            }
                        },
                    ),
                    el::<_, CatalogState, ()>(
                        "output",
                        format!("{:.0}%", state.knob_value * 100.0),
                    )
                    .attr("id", "catalog-knob-value")
                    .attr("for", "catalog-knob"),
                ),
            )
            .attr("class", "catalog-leaf-card catalog-knob-card"),
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
                selection,
                reorder_section,
                disclosure_section,
                editors,
                navigation,
                data,
                states,
                leaves,
            ),
        )
        .attr("class", state.width.class())
        .attr("data-specimen-width", state.width.name()),
    )
}

fn color(r: f32, g: f32, b: f32) -> ColorF {
    ColorF { r, g, b, a: 1.0 }
}

fn catalog_leaves(state: &CatalogState) -> LeafRegistry<u64> {
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
        Box::new(
            graph_swatch(
                state.graph_swatch_selected,
                state.graph_swatch_hovered,
                state.graph_swatch_focused,
            )
            .paint_leaf(graph_kind_color),
        ),
    );
    registry.insert(
        GRAPH_EMPTY_KEY,
        Box::new(empty_graph_swatch().paint_leaf(graph_kind_color)),
    );
    registry.insert(
        GRAPH_SINGLE_KEY,
        Box::new(single_graph_swatch().paint_leaf(graph_kind_color)),
    );
    registry.insert(
        GRAPH_CROWDED_KEY,
        Box::new(crowded_graph_swatch().paint_leaf(graph_kind_color)),
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
    knob.set_value(state.knob_value);
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

fn node_text(dom: &ScriptedDom, node: NodeId) -> String {
    dom.dom_children(node)
        .filter_map(|child| dom.text(child))
        .collect()
}

fn assert_initial_surface(dom: &ScriptedDom, root: NodeId, width: CatalogWidth) {
    assert_eq!(
        dom.element_name(root).map(|name| name.local.to_string()),
        Some("main".to_string())
    );
    for section in [
        "controls-section",
        "selection-section",
        "reorder-section",
        "disclosure-section",
        "editors-section",
        "navigation-section",
        "data-section",
        "states-section",
        "leaves-section",
    ] {
        find_id(dom, root, section);
    }
    assert_attr(dom, root, "data-specimen-width", width.name());
    assert!(has_class(
        dom,
        root,
        match width {
            CatalogWidth::Narrow => "catalog-width-narrow",
            CatalogWidth::Regular => "catalog-width-regular",
        }
    ));

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

    let tabs = find_id(dom, root, "catalog-tabs");
    assert_attr(dom, tabs, "role", "tablist");
    let selected_tab = find_where(dom, tabs, &|dom, node| {
        attr(dom, node, "role") == Some("tab") && attr(dom, node, "aria-selected") == Some("true")
    })
    .expect("selected tab");
    assert_attr(dom, selected_tab, "aria-controls", "catalog-panel-overview");
    let panel = find_id(dom, root, "catalog-panel-overview");
    assert_attr(dom, panel, "role", "tabpanel");
    assert_eq!(attr(dom, panel, "hidden"), None);
    assert_attr(
        dom,
        find_id(dom, root, "catalog-panel-history"),
        "hidden",
        "true",
    );
    assert_attr(
        dom,
        find_id(dom, root, "catalog-panel-links"),
        "hidden",
        "true",
    );

    let segments = find_id(dom, root, "catalog-segments");
    assert_attr(dom, segments, "role", "radiogroup");
    let chips = find_id(dom, root, "catalog-chips");
    assert_attr(dom, chips, "role", "toolbar");

    let reorder = find_id(dom, root, "catalog-reorder");
    assert_attr(dom, reorder, "role", "group");
    assert_attr(dom, reorder, "aria-label", "Related panel order");
    let first_reorder = find_id(dom, reorder, "catalog-reorder-item-notes");
    assert_attr(dom, first_reorder, "role", "listitem");
    assert_attr(dom, first_reorder, "tabindex", "0");
    let reorder_status = find_where(dom, reorder, &|dom, node| {
        attr(dom, node, "role") == Some("status")
    })
    .expect("reorder status announcement");
    assert_attr(dom, reorder_status, "aria-live", "polite");

    let disclosure_trigger = find_id(dom, root, "catalog-disclosure-trigger");
    assert_attr(dom, disclosure_trigger, "aria-expanded", "false");
    assert_attr(dom, disclosure_trigger, "data-disclosure-control", "true");
    assert_attr(
        dom,
        disclosure_trigger,
        "aria-controls",
        "catalog-disclosure-panel",
    );
    assert_attr(
        dom,
        find_id(dom, root, "catalog-disclosure-panel"),
        "hidden",
        "true",
    );

    let accordion = find_id(dom, root, "catalog-accordion");
    assert_attr(dom, accordion, "role", "group");
    let identity_trigger = find_id(dom, accordion, "catalog-accordion-item-identity-trigger");
    assert_attr(dom, identity_trigger, "aria-expanded", "true");
    assert_attr(dom, identity_trigger, "data-disclosure-control", "true");
    let accordion_heading = dom.parent(identity_trigger).expect("accordion heading");
    assert_attr(dom, accordion_heading, "role", "heading");
    assert_attr(dom, accordion_heading, "aria-level", "3");
    assert_attr(
        dom,
        find_id(dom, accordion, "catalog-accordion-item-identity-panel"),
        "role",
        "region",
    );

    let tree = find_id(dom, root, "catalog-tree");
    assert_attr(dom, tree, "role", "tree");
    let workspace = find_id(dom, tree, "catalog-tree-item-workspace");
    assert_attr(dom, workspace, "role", "treeitem");
    assert_attr(dom, workspace, "aria-expanded", "true");
    assert_attr(dom, workspace, "aria-selected", "true");
    assert_attr(dom, workspace, "data-disclosure-control", "true");
    let notes = find_id(dom, tree, "catalog-tree-item-notes");
    assert_eq!(attr(dom, notes, "aria-expanded"), None);
    let people = find_id(dom, tree, "catalog-tree-item-people");
    assert_attr(dom, people, "aria-expanded", "false");

    for surface in ["catalog-summary-card", "catalog-summary-row"] {
        let surface = find_id(dom, root, surface);
        find_where(dom, surface, &|dom, node| {
            has_class(dom, node, "summary-body")
        })
        .expect("shared summary body in each surface");
    }
    let mut summaries = Vec::new();
    collect_class(dom, root, "summary-body", &mut summaries);
    assert!(
        summaries.len() >= 4,
        "summary bodies include accordion panels"
    );

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
    assert_attr(
        dom,
        find_id(dom, root, "catalog-command-picker"),
        "role",
        "listbox",
    );
    assert_attr(
        dom,
        find_id(dom, root, "catalog-command-menu"),
        "role",
        "menu",
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

    for state in [
        "disabled",
        "empty",
        "error",
        "overflow",
        "dense",
        "long-label",
    ] {
        find_where(dom, find_id(dom, root, "states-section"), &|dom, node| {
            attr(dom, node, "data-state") == Some(state)
        })
        .unwrap_or_else(|| panic!("state specimen {state} is missing"));
    }
    let error = find_where(dom, root, &|dom, node| {
        attr(dom, node, "data-state") == Some("error")
    })
    .expect("error specimen");
    assert_attr(dom, error, "role", "alert");

    for graph in [
        ("catalog-graph-empty", "Empty related graph", 0),
        ("catalog-graph-single", "Single-node related graph", 1),
        ("catalog-graph-crowded", "Crowded related graph", 12),
    ] {
        let graph_root = find_id(dom, root, graph.0);
        let group = find_where(dom, graph_root, &|dom, node| {
            attr(dom, node, "aria-label") == Some(graph.1)
        })
        .expect("graph variant group");
        let mut targets = Vec::new();
        collect_class(dom, group, "graph-canvas-swatch-node", &mut targets);
        assert_eq!(targets.len(), graph.2, "{} node targets", graph.0);
    }

    let mut leaves = Vec::new();
    collect_named(
        dom,
        find_id(dom, root, "leaves-section"),
        "custom-leaf",
        &mut leaves,
    );
    assert_eq!(leaves.len(), 8);
    assert_eq!(
        node_text(dom, find_id(dom, root, "catalog-knob-value")),
        "62%"
    );
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

    let selected_tab = find_where(&runner.dom().borrow(), root, &|dom, node| {
        attr(dom, node, "role") == Some("tab") && attr(dom, node, "aria-selected") == Some("true")
    })
    .expect("selected tab");
    runner.set_focus(Some(selected_tab));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert_eq!(runner.state().tabs.active, 2);
    assert_eq!(runner.state().tabs.selected, [2]);
    let links_panel = find_id(&runner.dom().borrow(), root, "catalog-panel-links");
    assert_eq!(attr(&runner.dom().borrow(), links_panel, "hidden"), None);
    let overview_panel = find_id(&runner.dom().borrow(), root, "catalog-panel-overview");
    assert_eq!(
        attr(&runner.dom().borrow(), overview_panel, "hidden"),
        Some("true")
    );
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
    assert_eq!(runner.state().tabs.selected, [0]);

    let selected_segment = find_where(&runner.dom().borrow(), root, &|dom, node| {
        attr(dom, node, "role") == Some("radio")
            && attr(dom, node, "aria-checked") == Some("true")
            && has_class(dom, node, "selection-item")
    })
    .expect("selected segment");
    runner.set_focus(Some(selected_segment));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    assert_eq!(
        runner.state().segments.selected,
        [1],
        "disabled end is skipped"
    );
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
    assert_eq!(runner.state().segments.selected, [0]);
    let balanced_segment = find_id(
        &runner.dom().borrow(),
        root,
        "catalog-segments-item-Balanced",
    );
    runner.dispatch_click(balanced_segment, PointerClick::at((4.0, 4.0)));
    assert_eq!(
        runner.state().segments.selected,
        [1],
        "pointer activation uses the shared selection path"
    );

    let first_chip = find_id(&runner.dom().borrow(), root, "catalog-chips-item-Documents");
    runner.set_focus(Some(first_chip));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    assert_eq!(runner.state().chips.active, 2);
    assert_eq!(runner.state().chips.selected, [0]);
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    assert_eq!(runner.state().chips.selected, [0, 2]);
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
    assert_eq!(runner.state().chips.active, 0);

    let notes = find_id(&runner.dom().borrow(), root, "catalog-reorder-item-notes");
    runner.set_focus(Some(notes));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    assert_eq!(runner.state().reorder.destination(), Some(2));
    assert!(
        find_where(&runner.dom().borrow(), root, &|dom, node| {
            has_class(dom, node, "reorder-drop-indicator")
        })
        .is_some()
    );
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
    assert_eq!(runner.state().reorder_order, ["notes", "people", "places"]);
    assert_eq!(runner.state().reorder.destination(), None);

    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    assert_eq!(runner.state().reorder_order, ["people", "notes", "places"]);
    assert_eq!(
        runner.state().last_reorder,
        Some(ReorderMove {
            id: "notes",
            from: 0,
            to: 1,
        })
    );
    assert_eq!(runner.focus(), Some(notes), "focus follows the keyed row");

    let people = find_id(&runner.dom().borrow(), root, "catalog-reorder-item-people");
    runner.dispatch_pointer_down(
        people,
        PointerEvent::new(PointerPhase::Down, (8.0, 10.0), (180.0, 20.0)),
    );
    assert_eq!(runner.pointer_capture(), Some(people));
    runner.dispatch_pointer_move(PointerEvent::new(
        PointerPhase::Move,
        (8.0, 50.0),
        (180.0, 20.0),
    ));
    runner.dispatch_pointer_up(PointerEvent::new(
        PointerPhase::Up,
        (8.0, 50.0),
        (180.0, 20.0),
    ));
    assert_eq!(runner.pointer_capture(), None);
    assert_eq!(runner.state().reorder_order, ["notes", "places", "people"]);
    assert_eq!(
        runner.state().last_reorder,
        Some(ReorderMove {
            id: "people",
            from: 0,
            to: 2,
        })
    );

    let disclosure_trigger = find_id(&runner.dom().borrow(), root, "catalog-disclosure-trigger");
    runner.dispatch_click(disclosure_trigger, PointerClick::at((4.0, 4.0)));
    assert!(runner.state().disclosure.expanded);
    runner.set_focus(Some(disclosure_trigger));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
    assert!(!runner.state().disclosure.expanded);

    let activity_trigger = find_id(
        &runner.dom().borrow(),
        root,
        "catalog-accordion-item-activity-trigger",
    );
    runner.dispatch_click(activity_trigger, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().accordion.expanded, ["activity"]);
    assert_attr(
        &runner.dom().borrow(),
        find_id(
            &runner.dom().borrow(),
            root,
            "catalog-accordion-item-identity-panel",
        ),
        "hidden",
        "true",
    );

    let workspace = find_id(&runner.dom().borrow(), root, "catalog-tree-item-workspace");
    runner.set_focus(Some(workspace));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert_eq!(runner.state().tree.active(), Some(&"notes"));
    let notes = find_id(&runner.dom().borrow(), root, "catalog-tree-item-notes");
    assert_eq!(runner.focus(), Some(notes));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
    assert_eq!(runner.state().tree.active(), Some(&"people"));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert!(runner.state().tree.is_expanded(&"people"));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert_eq!(runner.state().tree.active(), Some(&"collaborators"));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowLeft)));
    assert_eq!(runner.state().tree.active(), Some(&"people"));

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

    let disabled_command = find_where(&runner.dom().borrow(), root, &|dom, node| {
        attr(dom, node, "aria-description") == Some("Connect a writable graph first")
    })
    .expect("disabled command reason");
    assert_eq!(
        attr(&runner.dom().borrow(), disabled_command, "aria-disabled"),
        Some("true")
    );

    let picker = find_id(&runner.dom().borrow(), root, "catalog-command-picker");
    assert_eq!(
        attr(&runner.dom().borrow(), picker, "role"),
        Some("listbox")
    );
    runner.set_focus(Some(picker));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
    assert_eq!(runner.state().last_picker, "activate:3");

    let command_menu = find_id(&runner.dom().borrow(), root, "catalog-command-menu");
    assert_eq!(
        attr(&runner.dom().borrow(), command_menu, "role"),
        Some("menu")
    );
    runner.set_focus(Some(command_menu));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
    assert_eq!(runner.state().menu_commands.submenu, Some(3));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
    assert_eq!(runner.state().last_menu, "activate:3/2");
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowLeft)));
    assert_eq!(runner.state().menu_commands.submenu, None);
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
    assert_eq!(runner.state().last_menu, "dismiss");

    let inside_overlay = find_id(&runner.dom().borrow(), root, "catalog-overlay-inside");
    runner.dispatch_click(inside_overlay, PointerClick::at((2.0, 2.0)));
    assert!(runner.state().overlay_open);
    assert_eq!(runner.state().overlay_inside_presses, 1);
    let dismiss_layer = find_class(
        &runner.dom().borrow(),
        root,
        "overlay-surface-dismiss-layer",
    );
    runner.dispatch_click(dismiss_layer, PointerClick::at((2.0, 2.0)));
    assert!(!runner.state().overlay_open);
    assert_eq!(
        runner.state().overlay_last_dismiss,
        Some(OverlayDismiss::OutsideClick)
    );
    let open_overlay = find_id(&runner.dom().borrow(), root, "catalog-overlay-open");
    runner.dispatch_click(open_overlay, PointerClick::at((2.0, 2.0)));
    let inside_overlay = find_id(&runner.dom().borrow(), root, "catalog-overlay-inside");
    runner.set_focus(Some(inside_overlay));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
    assert!(!runner.state().overlay_open);
    assert_eq!(
        runner.state().overlay_last_dismiss,
        Some(OverlayDismiss::Escape)
    );

    let detail_trigger = find_class(&runner.dom().borrow(), root, "detail-popover-trigger");
    runner.dispatch_hover(
        detail_trigger,
        HoverEvent::new(HoverPhase::Enter, (4.0, 4.0), (128.0, 32.0)),
    );
    assert_eq!(runner.state().detail_popover.mode, DetailPopoverMode::Peek);
    let tooltip = find_where(&runner.dom().borrow(), root, &|dom, node| {
        attr(dom, node, "role") == Some("tooltip")
    })
    .expect("detail tooltip");
    assert!(
        attr(&runner.dom().borrow(), tooltip, "style")
            .is_some_and(|style| style.contains("top: 150px")),
        "below placement flips above inside the catalog bounds"
    );
    runner.dispatch_click(detail_trigger, PointerClick::at((2.0, 2.0)));
    assert_eq!(
        runner.state().detail_popover.mode,
        DetailPopoverMode::Pinned
    );
    let detail_use = find_id(&runner.dom().borrow(), root, "catalog-detail-use");
    runner.dispatch_click(detail_use, PointerClick::at((2.0, 2.0)));
    assert_eq!(runner.state().detail_uses, 1);
    runner.set_focus(Some(detail_trigger));
    runner.focus_traverse(true);
    assert_eq!(runner.focus(), Some(detail_use));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
    assert_eq!(
        runner.state().detail_popover.mode,
        DetailPopoverMode::Hidden
    );
    assert_eq!(runner.focus(), Some(detail_trigger));

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
    let selected_document = find_where(&runner.dom().borrow(), graph_swatch, &|dom, node| {
        attr(dom, node, "aria-label") == Some("Selected document")
    })
    .expect("selected graph node");
    runner.set_focus(Some(selected_document));
    assert_eq!(runner.state().graph_swatch_focused, Some(1));
    runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Tab)));
    assert_eq!(runner.focus(), Some(related_person));
    assert_eq!(runner.state().graph_swatch_focused, Some(2));
    runner.dispatch_hover(
        related_person,
        HoverEvent::new(HoverPhase::Enter, (4.0, 4.0), (20.0, 20.0)),
    );
    assert_eq!(runner.state().graph_swatch_hovered, Some(2));
    runner.dispatch_click(related_person, PointerClick::at((4.0, 4.0)));
    assert_eq!(runner.state().graph_swatch_selected, 2);
    assert_eq!(runner.focus(), Some(related_person));
    assert_eq!(runner.state().graph_swatch_focused, Some(2));
    assert!(has_class(&runner.dom().borrow(), related_person, "focused"));
    let expand = find_where(&runner.dom().borrow(), graph_swatch, &|dom, node| {
        attr(dom, node, "aria-label") == Some("Expand graph")
    })
    .expect("graph expand affordance");
    runner.dispatch_click(expand, PointerClick::at((4.0, 4.0)));
    assert!(runner.state().graph_swatch_expanded);

    let knob = find_id(&runner.dom().borrow(), root, "catalog-knob");
    runner.dispatch_pointer_down(
        knob,
        PointerEvent::new(PointerPhase::Down, (12.0, 24.0), (48.0, 48.0)),
    );
    runner.dispatch_pointer_move(PointerEvent::new(
        PointerPhase::Move,
        (36.0, 24.0),
        (48.0, 48.0),
    ));
    runner.dispatch_pointer_up(PointerEvent::new(
        PointerPhase::Up,
        (36.0, 24.0),
        (48.0, 48.0),
    ));
    assert!((runner.state().knob_value - 0.75).abs() < f32::EPSILON);
    assert_eq!(
        node_text(
            &runner.dom().borrow(),
            find_id(&runner.dom().borrow(), root, "catalog-knob-value")
        ),
        "75%"
    );

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

fn assert_leaf_pipeline(state: &CatalogState) {
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
            GRAPH_EMPTY_KEY | GRAPH_SINGLE_KEY => Some(Size {
                width: 220.0,
                height: 104.0,
            }),
            GRAPH_CROWDED_KEY => Some(Size {
                width: 300.0,
                height: 144.0,
            }),
            _ => None,
        }
    }

    let mut registry = catalog_leaves(state);
    for key in LEAF_KEYS {
        assert!(registry.contains(&key));
    }
    let mut rendered = RenderedLeaves::new();
    let painted = registry.render_into(size, &mut rendered);
    assert_eq!(painted, LEAF_KEYS.len());
    assert_eq!(rendered.len(), LEAF_KEYS.len());
    for key in LEAF_KEYS.into_iter().filter(|key| *key != GRAPH_EMPTY_KEY) {
        assert!(
            rendered
                .get(key)
                .is_some_and(|commands| !commands.is_empty())
        );
    }
    assert!(rendered.get(GRAPH_EMPTY_KEY).is_some());
    assert_eq!(registry.render_into(size, &mut rendered), 0);
}

fn assert_retained_lifecycle_wall() {
    #[derive(Default)]
    struct LifecycleState {
        replaced: bool,
    }

    type LifecycleView = Box<dyn AnyView<LifecycleState, (), GenetCtx, GenetElement>>;

    fn focus_view(state: &LifecycleState) -> LifecycleView {
        if state.replaced {
            Box::new(el::<_, LifecycleState, ()>("span", ("replacement", "!")))
        } else {
            Box::new(button(
                "replace",
                |state: &mut LifecycleState, _: PointerClick| state.replaced = true,
            ))
        }
    }

    fn capture_view(state: &LifecycleState) -> LifecycleView {
        if state.replaced {
            Box::new(el::<_, LifecycleState, ()>("span", ("replacement", "!")))
        } else {
            Box::new(on_pointer(
                el::<_, LifecycleState, ()>("div", "drag target"),
                |state: &mut LifecycleState, event: PointerEvent| {
                    if event.phase == PointerPhase::Down {
                        state.replaced = true;
                    }
                },
            ))
        }
    }

    let focus_dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let mut focus_runner = GenetAppRunner::<_, _, _, ()>::new(
        focus_dom.clone(),
        focus_view,
        LifecycleState::default(),
    );
    let retired_focus = focus_runner.root();
    focus_runner.set_focus(Some(retired_focus));
    focus_runner.dispatch_click(retired_focus, PointerClick::at((2.0, 2.0)));
    assert!(!focus_dom.borrow().is_live(retired_focus));
    assert_eq!(focus_runner.focus(), None);
    assert!(
        focus_runner
            .dispatch_click(retired_focus, PointerClick::at((2.0, 2.0)))
            .is_empty()
    );

    let capture_dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let mut capture_runner = GenetAppRunner::<_, _, _, ()>::new(
        capture_dom.clone(),
        capture_view,
        LifecycleState::default(),
    );
    let retired_capture = capture_runner.root();
    capture_runner.dispatch_pointer_down(
        retired_capture,
        PointerEvent::new(PointerPhase::Down, (4.0, 4.0), (40.0, 20.0)),
    );
    assert!(!capture_dom.borrow().is_live(retired_capture));
    assert_eq!(capture_runner.pointer_capture(), None);
}

fn catalog_runner(width: CatalogWidth) -> CatalogRunner {
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    CatalogRunner::new(dom, catalog as CatalogLogic, CatalogState::at_width(width))
}

fn receipt_html(width: CatalogWidth) -> String {
    let runner = catalog_runner(width);
    let markup = runner.dom().borrow().outer_html(runner.root());
    let (viewport_width, _) = width.viewport();
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Cambium component catalog: {name}</title>\n<style>\nhtml {{ background: #dfe4ec; }}\nbody {{ margin: 0; min-width: {viewport_width}px; }}\n{THEME}\n</style>\n{markup}\n",
        name = width.name(),
    )
}

fn write_receipts() -> std::io::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("cambium crate lives under workspace/crates");
    let directory = root.join("docs").join("receipts");
    std::fs::create_dir_all(&directory)?;
    for width in [CatalogWidth::Narrow, CatalogWidth::Regular] {
        std::fs::write(
            directory.join(format!("component_catalog_{}.html", width.name())),
            receipt_html(width),
        )?;
    }
    Ok(())
}

fn run_acceptance() {
    let mut runner = catalog_runner(CatalogWidth::Regular);
    assert_initial_surface(&runner.dom().borrow(), runner.root(), CatalogWidth::Regular);
    run_interactions(&mut runner);
    assert_leaf_pipeline(runner.state());

    let narrow = catalog_runner(CatalogWidth::Narrow);
    assert_initial_surface(&narrow.dom().borrow(), narrow.root(), CatalogWidth::Narrow);
    assert_retained_lifecycle_wall();
}

fn main() {
    run_acceptance();
    if std::env::args().any(|arg| arg == "--write-receipts") {
        write_receipts().expect("write catalog HTML receipts");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_is_the_component_acceptance_surface() {
        super::run_acceptance();
    }

    #[test]
    fn committed_receipts_match_the_live_catalog() {
        assert_eq!(
            super::receipt_html(super::CatalogWidth::Narrow),
            include_str!("../../../docs/receipts/component_catalog_narrow.html")
        );
        assert_eq!(
            super::receipt_html(super::CatalogWidth::Regular),
            include_str!("../../../docs/receipts/component_catalog_regular.html")
        );
    }
}
