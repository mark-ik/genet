# Pelt port boundary

**Date:** 2026-07-24
**Status:** landed

Pelt is Genet's reference application, analogous to Servoshell. Its complete
product family lives under `ports/pelt`:

```text
ports/pelt/
  Cargo.toml       pelt library and executable
  desktop/         Pelt-specific desktop integration and acceptance runners
```

The former `pelt-core` package mixed product identity with contracts consumed
by other hosts. It is now `components/genet-host-api`. That component owns:

- engine profiles and capability reporting;
- the host-supplied resource-fetch seam;
- the tile-tree presentation contract.

Pelt, Mere, and Merecat consume `genet-host-api`. Mere and Merecat do not
depend on Pelt. The package rename is deliberate: the contract describes how
a host embeds Genet, while Pelt is one implementation of that host boundary.

The CI dependency-cone witness enforces the direction:

- components may not depend on packages below `ports/`;
- `genet-host-api` must remain below `components/`;
- `pelt` and `pelt-desktop` must remain below `ports/pelt/`.

Historical documents keep their contemporary `pelt-core` and
`ports/pelt-desktop` names where they describe already-executed work.
