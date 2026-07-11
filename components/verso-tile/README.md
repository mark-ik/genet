# verso-tile

The engine-flip machinery for the serval engine family: carry a live page's
state (cookies, scroll, form values) across an engine swap — the dynamic
counterpart of inker's engine multiplexer.

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/verso-tile`. Consolidated 2026-07-10 from the four mere-side
> verso crates (verso, verso-api, verso-scry, verso-serval) into one
> feature-layered component under the family's crates.io name.

- `api` — the portable view-state + donor/back/receiver contracts
  (dependency-free, so a black-box secondary can implement it).
- `flip` — the orchestrator: pairs a `FlipDonor` with a `FlipReceiver`,
  masks the carry to the layers both support (degrade, never block).
- `scry` — the black-box receiver over a thin `ScrySurface` seam (a system
  WebView the host drives).
- `serval` (behind the `serval-donor` feature) — the glass-box donor reading
  serval's scripted DOM.

Default features carry no engine dependencies; only the donor feature pulls
serval's DOM crates.
