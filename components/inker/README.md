# inker

`inker` is the engine/renderer controller for the serval engine family and
its hosts (merecat's mere, pelt, strophe, isometry). It owns the question
"*which engine should handle this content,*" taking into account the URI
scheme, the content type, which engines are actually available on the host,
and any user preference — and it defines the three engine kinds the answer
dispatches to: **document engines** (request/response, serializable blocks),
**session engines** (retained documents producing paint frames — the serval
HTML lanes, smolweb native), and **surface engines** (external GPU-texture
producers: scrying / graft / weld). One registry pattern, one routing
vocabulary, one a11y-capability declaration across all three.

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/inker` (adopted 2026-07). The former standalone repository is archived
> and links here.


Inker is the right home for arbitrating among engines when several are valid
for the same input. For full-web pages, both the Servo/wgpu fork (Serval) and
a Wry system webview can serve `https://`; only one of them may be built or
installed on a given host, and the user may prefer one over the other for a
specific domain or node. Resolving that is inker's job.

In the printing-press metaphor: inker pairs each engine to its protocol,
ready to ink the platen.

## What's in the crate

- **`document`** — the portable document model engines produce.
  - `EngineDocument` carrying `address`, `title`, `content_type`, `lang`
    (BCP 47), `provenance` (`source_kind` / `canonical_uri` / `fetched_at`
    / `source_label`), `trust` (`Trusted` / `Tofu` / `Insecure` / `Broken`
    / `Unknown`), `diagnostics` (`UnsupportedConstruct` / `DegradedRendering`
    / `ParseWarning` / `RawSourceFallback`), and `blocks`.
  - `DocumentBlock` variants — structural (`Heading`, `Paragraph`,
    `CodeBlock`, `Quote`, `List`, `Image`, `Preformatted`, `Rule`) plus
    semantic (`FeedHeader`, `FeedEntry`, `MetadataRow`, `Badge`). Each
    documents an AccessKit role; `uxtree` projects them into a real
    `TreeUpdate`.
  - `InlineSpan` (Text / Code / Emphasis / Strong / Link / SoftBreak /
    LineBreak), `inline_text` flattening helper, `outgoing_links` walker
    (covers structural inline links + semantic-block URL fields).
  - **Round-trip rendering** in `document::render` (`impl EngineDocument`):
    `to_markdown()`, `to_gemini()`, `to_knot()`. Knot rendering emits
    semantic blocks as fenced code blocks with their language tag so they
    round-trip through `nematic::KnotEngine`'s polyglot parser; `to_knot()`
    additionally emits a YAML frontmatter block when the document carries
    a `title`, any non-default provenance field, or a known trust state.
  - **Per-block provenance** — `BlockProvenance` + `BlockProvenanceMap`
    sidecar in `document::block_provenance` for documents whose blocks
    came from heterogeneous sources (clips, federated feed merges,
    citation overlays). Sparse: lookup falls back to the document-level
    provenance when no per-block override is recorded, so single-source
    documents pay nothing.
- **`engine`** — the engine trait, input/error vocabulary, and registry.
  - `Engine` trait — `engine_id() -> &str` and
    `render(&EngineInput) -> Result<EngineDocument, EngineError>`.
    Implementations live in protocol-specific crates (`nematic` ships 12;
    `serval` for full web).
  - `EngineInput` — already-fetched content with optional `content_type`.
    Network / disk I/O is the host's job; engines stay wasm32-portable.
  - `EngineRegistry` — engine ID → instance dispatch with `register`,
    `engine`, `contains`, `engine_ids`, and `dispatch` (which honors the
    decision's `engine_id` and emits a `tracing::warn` if the engine is
    unregistered).
  - `EngineError` — owned error vocabulary (`EngineNotFound`,
    `Unsupported`, `InvalidContent`, `NotFound`, `Io`, `Network`).
- **`routing`** — the route-decision vocabulary, default policy, and the
  full priority chain.
  - `EngineRouteRequest` carries `workspace_id`, `view`, `node`, `address`,
    optional `content_type` (server-claimed MIME), and optional
    `pinned_engine` (per-node engine pin — most authoritative signal).
  - `EngineRouteDecision` — `engine_id` + `SurfaceContract`.
  - `EngineRoutePolicy` — rules vector, fallback rule, plus
    `per_host_overrides: HashMap<String, String>` mapping a
    case-insensitive host to an engine ID.
  - `EngineRouteRule` — schemes + content_types + engine_id + mode. Build
    scheme rules with `EngineRouteRule::new(...)`; build content-type rules
    with `EngineRouteRule::content_type(...)`.
  - **Priority chain (in `route_filtered`)**: pin → content-type → per-host
    → scheme → fallback, all gated by an availability filter (typically
    `|id| registry.contains(id)`). Every step skips engines the filter
    rejects, falling through to the next.
  - `SurfaceContract` / `SurfaceContractMode` — host-neutral handoff
    (`CompositedTexture`, `NativeOverlay`, `EmbeddedHost`, `Headless`).
  - Helpers: `address_scheme()`, `host_from_address()`.
- **`sniff`** — best-effort content-type sniffing for unlabelled byte
  streams (file://, gopher item-1, finger replies, drag-and-drop,
  on-disk knot files). `sniff_content_type(bytes) -> Option<&'static str>`
  matches PNG / JPEG / GIF / WebP / SVG signatures, XML / HTML / Atom /
  RSS roots, knot frontmatter, gemtext link lines, markdown markers, and
  falls through to `text/plain` when the head window has no NUL bytes.
  Reads at most the first 1 KiB.
- **Engine ID constants**: `ENGINE_SERVAL_WEB`, `ENGINE_SCRYING_WEB`,
  `ENGINE_NEMATIC_FEED`, `ENGINE_NEMATIC_FILE`,
  `ENGINE_NEMATIC_FINGER`, `ENGINE_NEMATIC_GEMTEXT`,
  `ENGINE_NEMATIC_GOPHER`, `ENGINE_NEMATIC_GUPPY`, `ENGINE_NEMATIC_KNOT`,
  `ENGINE_NEMATIC_MARKDOWN`, `ENGINE_NEMATIC_MISFIN`, `ENGINE_NEMATIC_NEX`,
  `ENGINE_NEMATIC_SCROLL`, `ENGINE_NEMATIC_TEXT`, `ENGINE_NEMATIC_SMOLWEB`
  (umbrella, kept for back-compat — no protocol routes to it any more),
  `ENGINE_GRAPHSHELL_INTERNAL`, `ENGINE_EXTERNAL_PROTOCOL`.

  `ENGINE_SCRYING_WEB` is **opt-in per tile** — not in the default
  policy. Pinning it via `EngineRouteRequest::pinned_engine`
  (or a per-host override) routes the tile through that engine. See
  `design_docs/mere_docs/research/2026-05-11_engine_peers_and_scrying_library_brief.md`
  for the design rationale (preferred non-Servo path; embedded-frame
  vs overlay composition models).
- **Default policy** (full set):
  - Scheme rules: `http`/`https` → Serval; `gemini`/`spartan` →
    `nematic.gemtext`; `gopher` → `nematic.gopher`; `finger` →
    `nematic.finger`; `scroll` → `nematic.scroll`; `misfin` →
    `nematic.misfin`; `nex` → `nematic.nex`; `guppy` → `nematic.guppy`;
    `file` → `nematic.file`; internal schemes (`about`, `graphshell`,
    `mere`) → headless internal; everything else → headless
    external-protocol fallback.
  - Content-type rules (win over scheme): `text/markdown` /
    `text/x-markdown` → `nematic.markdown`; `text/gemini` →
    `nematic.gemtext`; `text/plain` → `nematic.text`; `application/rss+xml`
    / `application/atom+xml` / `application/feed+xml` → `nematic.feed`;
    `text/x-knot` / `application/x-knot` → `nematic.knot`.

## How it relates to other workspace crates

inker sits between [`graphshell`](https://crates.io/crates/graphshell) (which
issues route requests) and the engines themselves;
[`verso-core`](https://crates.io/crates/verso-core) owns the surface identity
inker hands back.

```text
       graphshell::app_state
              │ EngineRouteRequest
              ▼
            inker  ──────►  EngineRouteDecision
              │             (engine_id + SurfaceContract)
              │
              ▼
       engine_id selects: serval | scrying | nematic | wry | internal
                                                       │
                                                       ▼
                                                  verso-core
                                              (SurfaceTargetId)
```

- [`graphshell`](https://crates.io/crates/graphshell) — emits
  `EngineRouteRequest` effects via its `EngineRouter` service trait; consumes
  the returned `EngineRouteDecision`.
- [`verso-core`](https://crates.io/crates/verso-core) — `SurfaceContract.target`
  is `verso_core::SurfaceTargetId`, re-exported through `inker::routing` for
  convenience.
- [`nematic`](https://crates.io/crates/nematic) — implements 12 concrete
  `Engine`s: `markdown`, `gemtext`, `gopher`, `feed`, `text`, `file`,
  `finger`, `knot`, `scroll`, `misfin`, `nex`, `guppy`. Hosts register them
  in one call via `nematic::engines()` and dispatch through the registry.
- [`uxtree`](https://crates.io/crates/uxtree) — projects `EngineDocument`
  into an AccessKit `TreeUpdate` for OS a11y APIs and inspector overlays;
  every `DocumentBlock` and `InlineSpan` variant maps to an AccessKit role.
- **Serval** (Servo/wgpu fork) — referenced by engine ID `serval.web`; lives
  outside the mere workspace. The future "three-head Hekate" mode (smolweb
  extract / middlenet / fullweb negotiator for the same HTML input) is
  Serval's evolution; nematic explicitly does not own an HTML reader-mode
  engine.
- **Wry** (system webview, third-party) — available as an alternative engine;
  not in the default policy but a custom `EngineRouteRule` (or per-host
  override / per-node pin) can target it.

## Status

Pre-1.0 but feature-complete for the engine-controller layer:

- ✓ Engine trait + registry + dispatch
- ✓ Portable document model with provenance / trust / diagnostics
- ✓ Round-trip rendering (markdown / gemini / knot)
- ✓ Engine availability filtering (`route_filtered` skips unregistered engines)
- ✓ Content-type / MIME dispatch (server-claimed type wins over scheme)
- ✓ Per-domain / per-host overrides (user preference at host granularity)
- ✓ Per-node engine pinning (most authoritative signal)
- ✓ 12-engine default policy with full nematic coverage
- ✓ Content-type sniffing for unlabelled byte streams (`sniff` module)
- ✓ Per-block provenance sidecar
- ✓ Knot frontmatter round-trip

Planned expansions left:

- **Pinned-engine surface mode lookup** — currently defaults
  `CompositedTexture`; should consult the engine's preferred mode for
  pinned routes targeting headless engines.
- **Serval "head" preference** — when Serval becomes the three-head
  negotiator, route decisions may need to carry a "preferred head"
  (smolweb extract / middlenet / fullweb) along with the `engine_id`.

## License

MPL-2.0.
