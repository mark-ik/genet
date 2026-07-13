# Cambium

Cambium is a Genet-native reactive GUI toolkit. It combines Meristem's
reactive view core with a Genet DOM backend and Sprigging custom leaves.

This repository was extracted locally from the engine repository still named
`serval`. The engine product is migrating to **Genet**; existing `serval-*`
crate and Rust identifiers remain compatibility names until that source rename
lands. Meristem, the Cambium backend, and Sprigging now live here; consumer and
reverse-dependency migration remain staged work.

## Crates

- `meristem`: renderer-independent reactive diff and message core
- `cambium`: Genet backend, application runner, controls, and composition
- `sprigging`: engine-neutral custom leaves and arrangement geometry

The crates use their own appropriate licenses: Cambium is MPL-2.0, Meristem is
Apache-2.0, and Sprigging is MIT OR Apache-2.0.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the ownership rule and
[docs/upstream-xilem.md](docs/upstream-xilem.md) for provenance. The mixed
inherited license layout is recorded in [LICENSES.md](LICENSES.md), and the
claimed package names in [docs/namespace-claims.md](docs/namespace-claims.md).
