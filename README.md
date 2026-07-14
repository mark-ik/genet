# Cambium

Cambium is a Genet-native reactive GUI toolkit. It combines Meristem's
reactive view core with a Genet DOM backend and Sprigging custom leaves.

This repository was extracted from Genet's former Serval tree. Its public
backend vocabulary is now the `Genet*` family. Deprecated `Serval*` aliases
remain for source compatibility during consumer migration.

## Crates

- `meristem`: renderer-independent reactive diff and message core
- `cambium`: Genet backend, application runner, controls, and composition
- `cambium-winit`: winit keyboard translation for Cambium applications
- `cambium-nematic`: reactive views and themes over Errand's smolweb ASTs
- `sprigging`: engine-neutral custom leaves and arrangement geometry

The crates use their own appropriate licenses: Cambium is MPL-2.0, Meristem is
Apache-2.0, and Sprigging is MIT OR Apache-2.0.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the ownership rule and
[docs/upstream-xilem.md](docs/upstream-xilem.md) for provenance. The mixed
inherited license layout is recorded in [LICENSES.md](LICENSES.md), and the
claimed package names in [docs/namespace-claims.md](docs/namespace-claims.md).
Standalone and sibling-checkout development are described in
[docs/local-genet-development.md](docs/local-genet-development.md).
