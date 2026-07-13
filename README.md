# Cambium

Cambium is a Serval-native reactive GUI toolkit. It combines Meristem's
reactive view core with a Serval DOM backend and Chisel custom leaves.

This repository is being extracted locally from Serval. The first landed slice
is `meristem`; the backend and Chisel remain in Serval until their dependency
boundary is ready to move.

## Crates

- `meristem`: renderer-independent reactive diff and message core

See [ARCHITECTURE.md](ARCHITECTURE.md) for the ownership rule and
[docs/upstream-xilem.md](docs/upstream-xilem.md) for provenance.

