# Servo canvas backend selection

Canvas2D backend selection in this branch is GPU-first at runtime.

- `dom_canvas_backend = ""` or `"auto"` prefers the GPU-backed `vello` backend when Servo is built with the `vello` feature.
- `dom_canvas_backend = "vello"` explicitly requests the GPU backend.
- `dom_canvas_backend = "vello_cpu"` forces the CPU backend.

If Servo is built without the `vello` feature, `auto` falls back to `vello_cpu`, and an explicit `vello` request also falls back to `vello_cpu` with a warning.

`ports/servoshell` now enables the `vello` feature by default, so ordinary shell builds follow that GPU-first policy unless the runtime pref explicitly selects `vello_cpu`.
