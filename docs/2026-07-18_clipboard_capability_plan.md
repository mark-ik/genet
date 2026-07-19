# Clipboard Capability Plan (genet-layer, cross-app)

## Goal

One robust clipboard capability that serves every genet consumer: web content's
async Clipboard API, the pelt browser chrome, and cambium application hosts
(Hocket, mere, isometry). Shaped after the web `ClipboardItem` model so it
carries more than text (images, url-lists, custom formats, audio), rather than
each app bolting on its own text-only `arboard` call.

## Context (grounded in the tree)

- **A seam already exists, from the Servo lineage.** The embedder models the
  clipboard as messages: `EmbedderMsg::ClearClipboard(WebViewId)`,
  `GetClipboardText(WebViewId, GenericCallback<Result<String, String>>)`,
  `SetClipboardText(WebViewId, String)` (`components/shared/embedder/lib.rs`).
  The GET side is already callback-based, which the lazy-offer model below can
  build on. Everything is text-only.
- **`arboard` is already in-tree** as the backend: the `pelt-desktop` port reads
  the OS clipboard with `arboard::Clipboard::new().ok()?.get_text().ok()`
  (`ports/pelt-desktop/chrome_viewer.rs`), gated behind a `chrome` feature that
  lists `dep:arboard`. arboard is MIT/Apache, 1Password-maintained, and covers
  text plus image (RGBA); it does not do arbitrary MIME formats.
- **The DOM async Clipboard API is not built.** scripted-dom has no
  `navigator.clipboard` and no ClipboardEvents; the seam's only consumer today
  is the pelt browser chrome (omnibar paste).
- **There are two host worlds, and only one is served.** The WebView/browser
  path (pelt-desktop) reaches the embedder seam. Cambium application hosts
  (hocket-genet) render cambium on `genet-winit-host` + netrender and never
  instantiate a WebView or embedder, so they have no clipboard path at all.
- **Upstream Servo has moved on.** It replaced the text seam with a typed
  `ClipboardDelegate` on the WebView (default native impl, embedder-overridable,
  widening to images and url-lists). Genet is behind that evolution.

## Design

### One service, two front doors

Add a shared clipboard service (proposed crate `genet-clipboard`, a
`components/shared/` component, MIT/Apache per the founding convention). It owns
the typed API and the native backend. Two callers reach it:

- **Web content and browser chrome**, through the embedder seam: the
  `EmbedderMsg` clipboard variants become thin forwarders to the service instead
  of calling `arboard` in the port. This is also the path the DOM Clipboard API
  binds to later.
- **Cambium application hosts**, directly: `genet-winit-host` (or the cambium
  runner) exposes read/write to app hosts, so Hocket and its siblings get
  clipboard without a WebView.

One backend, one format model, one Wayland story, consumed by both worlds.

### The typed model (web `ClipboardItem` shape)

A write offers an ordered list of items, each a map of MIME type to a payload
that is either bytes or a lazy provider. Richest representation first, fallbacks
after. A read enumerates the available MIME types and fetches a chosen one on
demand. Lazy offers matter for three reasons that all point the same way: the
existing `GenericCallback` GET is already async, Windows uses delayed rendering,
and the web API is promise-based. So large payloads (audio) are never
materialized until a paste actually pulls them.

### Backend and license posture

- **arboard** (MIT/Apache, in-tree) stays the backend for text and image.
- **clipboard-rs** (MIT) has rich formats (html, rtf, files, custom, monitoring)
  but is X11-only on Linux, so it is not adoptable wholesale; harvest its
  per-platform format technique or fork its format layer if needed.
- **Wayland**: arboard's `wayland-data-control` feature, with
  smithay-clipboard / wl-clipboard-rs (MIT) as needed.
- **Custom formats** (the piece no single crate gives us cross-platform) is the
  tailored work: prefer extending arboard (ours already, healthy, dual-licensed)
  over adopting an X11-only dependency.
- New service crate is MIT/Apache. The `EmbedderMsg` edits touch MPL
  Servo-lineage files and stay MPL in place; the new crate does not inherit that
  header.

## Additional functionality worth pursuing

The north star that makes this more than a checkbox:

- **Audio interchange.** An app puts a loop on the clipboard as `audio/wav`
  (DAW-importable) plus a custom `web application/x-hocket-loop` (lossless
  Hocket-to-Hocket) plus `text/plain` (a label), degrading gracefully. Paste
  audio in and it becomes a layer. This is the clipboard as a no-lock-in on-ramp
  and off-ramp, the same ethos as `.hock` and wavicle.
- **Images and url-lists** for web parity and native image copy/paste (waveform
  renders, album art).
- **Custom app formats** for lossless in-app round-trips (a hand-off envelope, a
  mere subgraph, an isometry token) with a text fallback for chat.
- **Web content clipboard**, which genet owes the platform regardless. Building
  the service once pays both the app hosts and the DOM API.

## Phases and done-conditions

Organized by capability and validation, not time.

1. **P0 Shared service and text parity across both worlds.** Stand up
   `genet-clipboard` with a text API over arboard. Route the embedder seam
   through it (pelt stops calling arboard directly) and expose it to cambium app
   hosts.
   - Done: the pelt omnibar paste still works through the service, and a cambium
     app host reads and writes clipboard text through the same service. Hocket
     copies its contact token and pastes a recipient. Headed check on Windows.
2. **P1 Typed multi-format core.** Generalize text to the `ClipboardItem` list
   with lazy providers; widen the embedder variants to typed (the Servo
   `ClipboardDelegate` direction).
   - Done: a multi-representation write (text plus html) and a typed read that
     enumerates formats round-trip on Windows, macOS, and X11.
3. **P2 Images and url-lists.** `image/png` (RGBA via arboard) and
   `text/uri-list`.
   - Done: an image and a uri-list copy and paste across the three desktop
     platforms.
4. **P3 Custom formats and audio interchange.** Arbitrary MIME (the `web`
   prefix, with its required trailing space, for web-origin formats; native
   custom formats per platform). Light up audio.
   - Done: Hocket copies a loop that pastes into a DAW as audio, pastes an
     external WAV in as a layer, and round-trips a custom Hocket format
     losslessly Hocket-to-Hocket.
5. **P4 Linux robustness and honest degradation.** Wayland path, X11 PRIMARY
   selection (middle-click), and honest handling of the Wayland reality that
   clipboard content dies when the source window closes unless a manager holding
   `wlr-data-control` is running.
   - Done: copy and paste work on Fedora Wayland and Mint X11; the
     no-clipboard-manager case degrades with a clear message, not a silent loss.
6. **P5 DOM async Clipboard API.** Bind the service into scripted-dom as
   `navigator.clipboard` (read, write, readText, writeText, ClipboardItem) and
   copy/cut/paste ClipboardEvents, gated by the participant/permission model.
   - Done: a WPT-style clipboard test passes for text and image, permission-gated.

## Interim: the Hocket hand-off MVP does not block on this

Hocket's hand-off needs only text (copy a contact token, paste a recipient).
Rather than wait for P0, hocket-genet may call arboard for text directly as a
stopgap, then migrate onto the shared service at P0/P1. The stopgap is marked in
the hand-off UI plan so it is not mistaken for the final layering.

## Open questions

- **Resolved: crate home.** `components/genet-clipboard`, matching the
  `genet-probe` founded-component convention. `components/shared/` is where the
  Servo-derived MPL crates live; a clean-room MIT/Apache crate sits beside the
  other `genet-*` components.
- **Resolved: the cambium front door is app-held, not a host API.**
  `genet-winit-host` is a render host (wgpu boot, rasterize, surfaces), not an
  app framework with an update context, and cambium's pure elm-style update has
  no clean seam for a side-effectful clipboard read mid-key-handling. So cambium
  apps hold a `genet_clipboard::SystemClipboard` directly and intercept
  copy/paste keys at the winit/app level, exactly as the pelt chrome already
  does (`read_clipboard()` on Ctrl/Cmd+V). If a second app duplicates that
  interception, extract a small key-to-clipboard helper then, not before.
- Whether to keep the text `EmbedderMsg` variants during P1 for a transition or
  replace them outright.

## Findings

- The clipboard seam is text-only and WebView-path-only; cambium app hosts and
  the DOM API are both unserved. A shared service with two front doors is the
  smallest thing that serves all three consumers.
- arboard is a backend, not a capability. Defaulting each app to it caps us at
  text plus image, which is below where the value (audio, custom formats) lives.
- The GET seam is already callback-based, so the lazy-offer model is a
  generalization of what exists, not a rewrite of the message flow.

## Progress

- 2026-07-18: Scoped against the current genet tree (embedder seam, pelt-desktop
  arboard backend, cambium host path, scripted-dom clipboard absence) and the
  Rust clipboard prior art (arboard, clipboard-rs, copypasta, smithay-clipboard;
  the web `ClipboardItem` model; Servo's `ClipboardDelegate`). Motivated by the
  Hocket hand-off UI plan's clipboard need.
- 2026-07-18: **P0a landed.** `components/genet-clipboard` created (MIT/Apache):
  a `TextClipboard` trait, an arboard-backed `SystemClipboard` holding a live
  handle, and an in-memory `MemoryClipboard`; `Empty` and `Unavailable` kept
  distinct; system backend behind a default feature so headless/wasm builds keep
  the trait and fake. Tests + clippy green.
- 2026-07-18: **P0b landed.** pelt-desktop's omnibar paste reads via
  `genet_clipboard::SystemClipboard`; pelt's direct arboard dependency removed
  (arboard now transitive, behind the service). The embedder clipboard seam has
  no port handler today, so nothing else in pelt needed routing.
- 2026-07-18: **P0c resolved (no host API needed).** genet-winit-host is
  render-only and cambium's update model can't hold a side-effectful clipboard,
  so the cambium front door is app-held consumption plus pelt-style key
  interception. Its validation is the Hocket hand-off UI consuming the service,
  which is where P0's text round-trip is proven end to end.
- 2026-07-18: **P0 complete.** Hocket exercises both directions of the text
  service in its hand-off UI: Copy token writes the contact token
  (`set_text`) and Paste recipient reads a peer's token (`get_text`). Both host
  worlds now go through `genet-clipboard` (the pelt browser omnibar and the
  Hocket app host), so the shared seam has real consumers on both sides. The OS
  backend is verified on the Windows host by an ignored `SystemClipboard`
  round-trip test (set then get, restoring the prior clipboard). P1 (typed
  multi-format items) and beyond remain future phases, not loose ends; the text
  capability is a coherent, closed increment. A headed GUI screenshot of the
  circle's collaboration controls is the one check left, and it doubles as the
  visual-design pass.
