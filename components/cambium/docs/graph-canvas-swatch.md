# Graph-canvas swatch

`graph_canvas_swatch` is a bounded instance of Sprigging's `GraphCanvas`, for a
Related panel, preview card, or section-line summary. It is content-sized at
260 by 128 pixels by default and remains configurable through
`GraphCanvasSwatch`.

The app-facing contract is:

- `GraphCanvasSubgraph`: nodes carrying id, kind, normalized position, and an
  accessible label; edges carrying endpoint ids;
- selected, focused, and hovered node ids;
- one stable leaf key and a pane-local `GraphViewport`;
- callbacks for node click, hover transition, and expansion.

Node kind stays opaque to Cambium. The consumer supplies the kind-to-color
mapping when it calls `GraphCanvasSwatch::paint_leaf`, so the same product
palette drives a swatch and a full canvas.

The view is a relative card containing one `custom_leaf`, a native button at
each projected node coordinate, and a small Expand button. Node buttons use
`on_click` and `on_hover`; keyboard focus remains native. The leaf and hit
targets both call `GraphViewport::project`, which is the alignment contract.

The host owns its `LeafRegistry`. After a graph, viewport, selection, focus, or
hover change, rebuild the registered leaf with `paint_leaf` before the next
paint pass. App navigation and staging remain callback policy rather than
component state.

`GRAPH_CANVAS_SWATCH_CSS` provides a quiet baseline. Hosts may use their own
palette against the emitted structural classes.
