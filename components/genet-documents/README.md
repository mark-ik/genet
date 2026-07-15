# genet-documents

Genet's retained document sessions: the static, scripted, and smolweb
content lanes as inker **session engines** (the third engine kind — spawn a
session, take paint frames, scroll, click, settle).

> **Home:** [`mark-ik/genet`](https://github.com/mark-ik/genet), at
> `components/genet-documents`. Born 2026-07-10 in the session-engines
> plan: these types began as pelt's convenience lanes and were promoted to
> an engine-grade component; pelt is now one consumer among hosts
> (merecat's mere, meerkat).

- `LoadedDocument` / `StaticSessionEngine` (`genet.web`): fetched HTML laid
  out by genet's cascade, no scripts.
- `LiveryDocument` / `LiverySessionEngine` (`livery` feature,
  `genet.livery`): the opt-in clean-room static implementation. It retains
  style/layout/text paint state and lowers the neutral PaintList into the same
  scene contract. It also routes bounded viewport scrolling, retained link
  rectangles, pointer hit testing, fragment navigation, and focus state. The
  session also exposes the retained animation clock for host-driven opacity
  frames, bounded CSS opacity transitions, and opacity-only `@keyframes` with
  named timing functions. Nested scroll chaining is routed through the retained
  session and chains at its boundary. Resource-backed images and multi-property
  transitions remain open. Livery's image gate covers two-stop gradients,
  raster `data:` URLs, host-resolved local bytes, and bounded intrinsic
  position/repeat modes; remote URL resources and replaced-element layout
  remain open.
- `ScriptedDocument` sessions / `ScriptedSessionEngine<E, _>` (`scripted`
  feature): a live page whose JS runs on Boa (or Nova on the nova rung),
  with the tick + quiescence seam (`pump` / `settled`).
- `SmolwebDocument` / `SmolwebSessionEngine` (`smolweb` feature): capsules
  rendered through the engine-native document path: Nematic lowers protocol
  content to `EngineDocument`, then document-canvas lays it out and lowers its
  PaintList to a scene.

Construction seams (fetchers, cookie jars, themes) live on the engine at
registration; the spawn request stays plain data. The session wrappers are
public for hosts with richer seams. Unpublished: this crate rides genet's
in-tree components; consume it as a git dependency.
