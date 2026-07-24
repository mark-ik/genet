# Meristem

Meristem is the renderer-independent reactive diff and message core of
Cambium. Backends implement its view and element contracts; Cambium's primary
backend targets Genet.

The crate supports `no_std` with `alloc` and contains no dependency on Genet,
Chisel, winit, or a renderer.

Meristem is derived from Linebender's Apache-2.0 `xilem_core`. Existing Xilem
copyright and SPDX headers remain in inherited source files. See
[`docs/upstream-xilem.md`](../../docs/upstream-xilem.md) for the recorded bases
and Cambium's semantic patch ledger.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
