# Pelt presentability execution plan

**Date:** 2026-08-07

**Status:** not started. Seven investigation lanes and their adversarial
verdicts are folded in below. Where a verdict overturned a report, the verdict
wins and the plan says so.

## Ruling

Pelt is judged as a picture and as a first five minutes, not as a feature
list. A gate earns its place by changing what a screenshot looks like or by
removing something a visitor would read as "this is a test harness."

Almost nothing here is new capability. The engine already computes the
document title, paints focus rings, carets, selection highlights and scrollbar
thumbs, supports `:hover`/`:focus` restyle, derives OKLCH palettes, and
implements `prefers-color-scheme` in its cascade. Pelt calls none of it. The
work is wiring, and the plan is ordered by how much each wire shows.

Three things are genuinely missing rather than unwired, and they are the three
real gates: consumption of `text-align` and `visibility` in `genet-layout`,
subresource loading, and DPI awareness.

## What one afternoon buys

G1, then G2. Both are hours, both are additive, and between them every
committed screenshot in `testing/genet/pelt-shots/` changes:

1. **G1 first.** The omnibar is measured at 1.50:1 contrast, pure `(0,0,0)`
   glyphs on `(43,43,51)`, with no field box and no caret. It is the most-read
   element in every chrome shot and it is the worst thing in frame. One const
   string plus one argument that is currently `None`.
2. **G2 second.** Title bar reads the page name instead of a 60-character
   Windows path, stops being the machine's red accent colour, gets an icon,
   and `--size WxH` makes the next capture a chosen size instead of one of
   three hardcoded ones.

If the afternoon runs long, stop after G1's CSS half. It is the single
highest-value change in the document and it cannot break anything else.

## Order and cost

| Gate | Outcome | Cost | Screenshot delta |
|---|---|---|---|
| G1 | the chrome strip reads as an address bar | hours | large, 5 of 6 shots |
| G2 | the window reads as an application | hours | large, all shots |
| G3 | a smolweb capsule is readable and distinct | a day | large, 4 of 6 shots |
| G4 | tiles host capsules: several protocols, one window | a day | a new hero image |
| G5 | one palette across chrome, tiles and capsules | a day | medium, coherence |
| G6 | an HTML document looks like a page | a real gate | large, but only for HTML |
| G7 | a real site loads its own stylesheets and images | a real gate | the whole point of HTML |
| G8 | DPI | a real gate | every shot, silently |
| G9 | captures that assert what they captured | hours | none, insurance |

G1 through G5 are the presentability work. G6 through G8 are engine work that
presentability eventually requires. G9 is a floor.

## Where the lanes disagreed

Surface these before the gates, because they are the most decision-relevant
content in the investigation.

**The tinct dependency split, and it is the biggest one.** The look lane's
headline finding was that cambium already contains the exact tinct-to-omnibar
assembly pelt needs, at no dependency cost, because "illume and tinct are both
workspace path members." They are workspace members, and cambium does not use
them. `components/cambium/cambium/Cargo.toml:24-25` declares
`illume = { version = "=0.0.2", optional = true }` and the same for tinct, with
no `path` and no `workspace = true`, and `[patch.crates-io]` (root
`Cargo.toml:508`) has no entry for either. Cargo.lock resolves cambium's copies
from the registry. Three consequences: `components/tinct` has **zero**
consumers in this workspace, so edits to it reach nothing today; enabling
`cambium/highlight` pulls two crates from crates.io; and the look lane's
proposals 2 and 3 do not compose, because passing a path-`tinct::Seeds` to
cambium's registry-`tinct` `syntax_css` is a hard type error, not a warning.
The same split already exists for illume, where knot-editor-host uses the path
copy and cambium the registry copy.

The verdict called resolving this a manifest-plus-lockfile-plus-decision job.
It is cheaper than that: the workspace declarations carry both `version` and
`path`, so `tinct = { workspace = true, optional = true }` publishes correctly
from a `publish = true` crate. Two lines. But it must land before any tinct
work, and that is G5's first task.

**The highlighted omnibar was ranked top and is worth nothing.** It was sold as
"the single most 'this is a real browser' detail available for the price": a
coloured scheme and host in the URL. illume's URL token is
`https?://[^\s<>()\[\]]+` and nothing else, so `gemini://`, `gopher://`,
`nex://` and `finger://` produce no span at all and the field renders
byte-identically to today. Four of the five smolweb shots get literally
nothing. Independent of that, tinct has exactly **one** `Url` role, so even on
an https URL the whole string becomes one uniformly coloured span. There is no
scheme/host distinction available from this change at any price. Deferred.

**A theme call would have broken the thing it was improving.** The systems lane
proposed calling `TileShell::set_theme` and described the gate as "CSS-only: no
Rust structure changes." `TileSurface::new` builds `sheets` as
`vec![DEFAULT_TILE_CSS, FRISKET_CSS]` (`tile_surface.rs:398`) and `set_theme`
does `self.sheets.truncate(1)` (`:448`). Calling it discards FRISKET_CSS,
`.frisket-content` loses `flex: 1 1 0; min-height: 0`, the content hole
collapses, and the tile documents stop painting. `Chrome::add_stylesheet`
appends and does not have this bug. The two seams are not interchangeable.

**The capture harness lane's motivating premise was the wrong file.** The
testing lane built two proposals around `capture.ps1` being a
`GetForegroundWindow` + `SetForegroundWindow` + `CopyFromScreen` screen grab
with a 900ms sleep. `testing/genet/pelt-shots/capture.ps1` uses
`PrintWindow` with `PW_RENDERFULLCONTENT`, sets
`DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`, and its own header explains why a
screen copy is wrong. The described script is an older file elsewhere. So "no
foreground race, no chance of photographing the wrong window" is already true,
and the scenario harness drops from "this is the screenshot harness" to G9.
What is genuinely missing is a pinned size, an in-process readback, an
assertion of what was captured, and the client rect rather than the window
frame.

**Two size estimates were overturned.** "Linked stylesheets is hours of work at
a single chokepoint" is a day: `--engine scripted` is a second, parallel
document loader that the proposal never mentions; appending linked sheets after
inline sheets inverts the cascade for the normal real-site shape; the walker
ignores `media`, and every fetched sheet is parsed with `MediaList::empty()`,
so a fetched `media="print"` sheet applies to the screen. And "plumb text-align
into parley, a day, a copy of a known-good shape" is two-plus days with a
fixture re-bless: the `no_wrap` pattern it copies is what blocks alignment,
because nowrap is signalled by passing `max_advance = None` and parley computes
free space from the line box, so every `white-space: nowrap; text-align: center`
element stays flush left after the fix.

**Two "already done" claims were wrong in the same direction.** `transform` was
listed as implemented; percentage translate is silently dropped
(`to_transform_3d_matrix(None)`), so `transform: translate(-50%, -50%)` is a
no-op, and `transform-origin` is unimplemented on pelt's path, so every
`rotate()` and `scale()` pivots from the border-box corner instead of the
centre. `:link`, `:visited` and `:any-link` can never match, because
`adapter_stylo.rs:616` is `fn is_link(&self) -> bool { false }` - so even a page
that ships its own link styling renders links unstyled.

**One correction that shrinks work.** `DocumentStyleSheet` was reported to have
three consumers outside document-canvas. It has one:
`components/genet-documents/src/smolweb.rs`. Adding a field to it is cheaper
than G3 was sized for.

## Pinned facts, so gates do not edit the wrong copy

- Pelt runs the **stylo fork**, not Livery. `components/livery/properties.toml`
  and its 149 `[[unimplemented]]` rows describe Livery's deficit relative to
  stylo. None of them are pelt's gaps. Every gap in G6 is a missing *read* in
  `genet-layout`, because `cascade.rs:461` deliberately sets
  `layout.unimplemented = true` so everything parses.
- There are **two** `SmolwebTheme` implementations with identical hex palettes
  and drifted stylesheets. Pelt reaches
  `components/genet-documents/src/smolweb.rs`, via `smolweb_glue.rs:10`. The
  copy with the 48rem measure cap and the "e.g. mapped from tinct" comment is
  `cambium-nematic/src/views/theme.rs`, which pelt does not use. Edit the
  genet-documents copy. Do not collapse them (see deferrals).
- `misfin://` is **not** a fetch scheme. `errand::Scheme::parse` returns None
  for it; errand's misfin support is outbound mail. `titan://` and `scroll://`
  do route. Do not widen `is_smolweb_url` to include misfin.
- `pelt --features tiles` does **not** compile the smolweb branch of
  `LocalFetcher`. In that build a `gemini://` URL falls through to
  `std::fs::read`, fails, and is silently swallowed - a blank tile, not a wall
  of gemtext.
- `netfetch` is off by default. `pelt --engine static https://…` fetches
  nothing in a stock build.
- `LoadedDocument::frame` is the reftest entry point
  (`headless.rs:40-55`). Anything that changes what it emits re-blesses the 7
  committed `.scene` fixtures.
- The red title bar is this machine's: HKCU DWM `ColorPrevalence=1`,
  `AccentColor` = RGB(232,17,35). Pelt sets no caption colour.

---

## G1. The chrome strip

**Cost: hours. Cheap. Do this first.**

### Outcome

The omnibar reads as an address field: legible text in a recessed box that
fills the strip, with a caret when focused and a focus ring. The strip stops
rendering in a serif face. A reload button exists.

### Work

Three lanes proposed pieces of this separately and they are one gate. CSS alone
leaves an unstyled shrink-to-fit box, and the focus ring would then draw at
that box, so the Rust half and the CSS half land together.

- Add an `input` rule to `DEFAULT_CHROME_CSS` (`chrome.rs:197-202`, currently
  five rules with no `input` selector at all): explicit `color`, a recessed
  `background`, a border, padding, and `flex: 1 1 auto; min-width: 0`.
- Add `font-family` on `.toolbar`. The strip currently renders serif because
  nothing sets one, which reads as accidental beside the sans content.
- Lift `button.disabled` off `#888888` (2.75:1 on `#444444`). Both nav buttons
  are always disabled at launch, so every launch screenshot shows two
  washed-out buttons. Dim the background too, not only the text.
- Pass `Some(TextCursor{..})` where `Chrome::frame` passes `None`
  (`chrome.rs:252-259`). `Chrome::focused()` gives the node and
  `TextInput::caret_position()` gives the byte. The affinity does **not** match:
  `cambium::CaretAffinity` and `genet_layout::VisualAffinity` are unrelated
  enums with no `From` impl. Write the map.
- Add a reload button and handle `ChromeIntent::Reload`, which is declared and
  never constructed. Reload must bypass the `url != self.loaded_url` guard.
- Check `--strip left|right`. `.toolbar` is `display: flex` with no
  `flex-direction`, and the vertical strip is 280px wide, so two buttons plus a
  flex-grow omnibar will not fit. No lane noticed this.

### Evidence

- Sample the omnibar glyph pixels and the strip background in a fresh capture;
  contrast must clear 4.5:1. Today it is 1.496:1.
- A `--chrome` capture with the omnibar focused shows a ring and a caret.
- A `--strip left` capture is not visibly broken.
- `chrome_renders_toolbar_text` (`chrome.rs:386`) only asserts a GlyphRun
  exists. Add an assertion on the emitted fill colour so a contrast regression
  is caught.

### Stop rules

- Stop the caret half if `<input>` will not accept `background`/`padding`
  through this path and the field cannot be made to fill the strip. Ship the
  colour fix alone; it is most of the value.
- Do not add a Stop button. The fetch path is synchronous, so it would be a
  placebo.
- Do not add Zoom. There is no scale parameter in `IncrementalLayout::new`.
- Drop Ctrl+L if it grows past an hour. `chrome_view` is a bare
  `fn(&ChromeState) -> ChromeView` with no access to NodeIds, so there is
  nowhere to cache the omnibar's node; it needs the test-only DOM walk promoted
  or a marker attribute.

### Removal receipt

`button.disabled`'s `#888888`. `None` at `chrome.rs:256`. Nothing else is
deleted; `DEFAULT_CHROME_CSS` is still the base sheet G5 layers over.

---

## G2. The window

**Cost: hours. Cheap. Do this second.**

### Outcome

The title bar carries the document's name, in pelt's colour, with pelt's icon,
and the next capture is a chosen size.

### Work

- `fn title(&self) -> Option<String>` on `BrowsableContent` and
  `ViewerContent`, **defaulted to `None`**. There are three `ViewerContent`
  impls and two `BrowsableContent` impls; a defaulted method keeps this at
  hours. `LoadedDocument::inspect().title` already exists and needs no change
  to `document.rs`.
- Call `window.set_title` after creation **and** after a successful content
  swap in `apply_chrome_intents`. `set_title` is called nowhere in the repo
  today, so the title is also stale after every navigation - a separate bug
  from showing the path, and worse in a live demo.
- `#[cfg(windows)] .with_title_background_color(...)` and
  `.with_title_text_color(...)` on the three window builders. winit 0.30.13
  ships these and its own doc names circumventing the accent-colour setting as
  the use case. This also makes captures reproducible across machines.
- `.with_window_icon(...)`. Pelt sets none, anywhere. Generic icon in the title
  bar, Alt-Tab and the taskbar.
- `--size WxH`, threaded to the three headed window sizes (hardcoded 800x600 /
  1000x700 / 1100x750) and to the headless `--out` scene and PNG renders.
  **Not** to `--reftest`: the 7 `.scene` fixtures are authored at 800x600 and a
  size override there silently invalidates all of them. Reject the combination.
- Add `cargo check -p pelt --features tiles,chrome,scripted,smolweb` to CI. CI
  compiles the tile *surface* today (13 tests) but not `tile_viewer.rs`, which
  is where the Windows accesskit panic lived.

### Evidence

- A capture at a requested size, with the achieved size printed. `inner_size()`
  is re-read after creation, so requested and achieved can differ.
- Navigating twice in `--chrome` changes the title twice.
- CI fails on a deliberate syntax error in `tile_viewer.rs`.

### Stop rules

- Stop the caption colour if it needs anything beyond the three builder chains.
  Do not go near `with_decorations(false)` and a hand-drawn caption; that is a
  different project.
- If `--size` in logical vs physical pixels becomes a question, ship physical
  (matching today's `PhysicalSize`) and let G8 revisit it.

### Removal receipt

`title: format!("Pelt — {url}")` at `static_viewer.rs:33` and the hardcoded
`"Pelt — tiles"`. `DEFAULT_WIDTH`/`DEFAULT_HEIGHT` survive as the reftest
constants.

---

## G3. The smolweb capsule

**Cost: a day. The first real one.**

### Outcome

A gemini capsule is readable at a screenshot's width, and two capsules from
different hosts are visibly different documents.

### Work

The capsule has no maximum measure: `available_width` is the viewport minus 2x
horizontal padding, producing ~200-character lines in the 1678px shots.
Measured backgrounds across four hosts are (250,247,245), (246,250,245),
(246,245,250), (245,250,247) - a ~5/255 spread, because `site_palette` emits
`hsl(h, 30%, 97%)`. Per-site identity nobody will ever see.

- Add `max_content_width: Option<f32>` to `DocumentStyleSheet`, clamp
  `available_width`, and centre the column. Additive; `None` leaves the one
  other consumer unchanged. Update the `Default` impl.
- Replace the HSL `site_palette` with a real derivation: push the host hash into
  hue and put the saturation into links, headings, rules and a header band
  rather than a 3% background wash. That is where Lagrange and Geopard get
  capsule identity.
- Decide `body_font_family`. It is currently inert: `text.rs:211-217` pushes
  `GenericFamily::SystemUi` or `Monospace` from `base.monospace` alone and never
  reads the resolved family, so `style.body_font_family = "serif"` is a no-op.
  Either honour it or delete the field. Do not leave a setting that does
  nothing.
- **Preserve whitespace where the format is preformatted.** This is the largest
  visible defect in the capsule shots and the survey missed it, because it is
  only apparent in rendered output. Smolweb text is being reflowed as
  proportional prose with collapsed runs. Measured on 2026-08-07 across four
  live captures in `testing/genet/pelt-shots`: `nex://nightfall.city/`'s ASCII
  banner is mangled and its `=>` lines run into their descriptions;
  `gopher://gopher.floodgap.com/`'s banner reflows into a paragraph;
  `finger://happynetbox.com/`'s 25-profile list collapses into one wrapped blob.
  Gemtext has explicit ``` blocks, a gopher menu is column-aligned by
  construction, and nex and finger are plain text throughout. The existing
  `base.monospace` flag is per-document, so the fix is per-region rather than a
  flag flip: the parse layer already distinguishes these node kinds in
  `components/errand/src/parse/`, so carry that through to the style the
  document canvas applies.
- Style nex link lines. Gemini and gopher colour theirs; nex's `=>` lines render
  as plain text in the same capture set, so either the nex parser is not
  emitting link nodes or the theme does not cover them. Determine which.

### Evidence

- A `--smolweb` capture at 1678px shows lines under ~90 characters.
- The four captures above, retaken: the nex banner is intact, the finger profile
  list is one entry per line, and gopher's menu columns align.
- Two capsules from different hosts, cropped side by side, are distinguishable
  without a colour picker.
- `document-canvas`'s layout tests assert on content origin; centring moves
  those x-coordinates. Re-bless them in this gate, not later.

### Stop rules

- Stop if the tint requires taking a tinct dependency in `genet-documents`.
  That decision belongs to G5 and it drags the dependency split with it. Ship
  a hand-derived palette here and let G5 unify it.
- Do not touch `cambium-nematic`'s copy. Pelt does not use it.

### Removal receipt

The `hsl(h, 30%, 97%)` background formula. `body_font_family` if it is deleted
rather than honoured.

---

## G4. Tiles hosting capsules

**Cost: a day. This is the new hero image.**

### Outcome

One `--tiles` window with an HTML document in one pane and a gemini capsule
beside it. Real tab titles. A visible message when a load fails.

### Work

- Reorder the CLI so `--tiles` wins over the smolweb short-circuit
  (`viewer.rs:233-242`). Note the current behaviour is argument-order
  dependent: `url` is overwritten by every positional while `tile_urls`
  accumulates, so `--tiles gemini://x a.html` already enters the tile lane and
  feeds gemtext to the HTML parser.
- Add `load_tile_session(fetcher, url) -> Box<dyn DocumentSession<Scene>>` to
  `genet-documents`, which already owns the scheme knowledge and both session
  impls. Do **not** reuse `BrowsableContent`: it is
  `pub(crate) trait BrowsableContent: Sized` with an associated
  `fn load(url) -> Result<Self, String>`, so it is not object-safe and can
  never be a trait object. The chrome shell is monomorphised one lane per
  window; tiles need heterogeneous lanes in one `HashMap`, which chrome has
  never had to do.
- Change `docs: HashMap<TileId, LoadedDocument>` to the boxed session and
  rewrite the **seven** call sites (`load_docs`, `navigate_tile`, `frame`,
  `scroll_by`, `scroll_at`, `click_at`, `inspect`).
- Implement `EngineDocument::content_report()` so `inspect()` answers for a
  capsule. This is also what makes tab titles work, so it is not optional.
- Fix `resolve_href` for a base with an authority:
  `resolve_href("gemini://h/a/b.gmi", "/c.gmi")` returns `/c.gmi` today, which
  the fetcher tries to read as a filesystem path. Root-relative links are the
  common gemtext form, so clicking one silently fails - and this is a live bug
  in the shipped `--chrome gemini://…` lane, not only a tiles one. Preserve the
  `\\` arm for the Windows path contract.
- Title tiles from the document. `tile_title` takes the last path segment,
  which is empty for any URL ending in `/`, so every capsule tab reads the
  literal word "tile".
- Replace the silent `if let Ok(doc)` swallow at `tile_surface.rs:465` with a
  placeholder document. Note `StaticDocumentSession` is private, so the
  placeholder needs a second `genet-documents` entry point.
- Give the fetch a timeout. `smolweb_get_bytes` calls plain `errand::fetch`
  inside `block_on`, and all tile documents load **before** `EventLoop::new()`,
  so a capsule that accepts a connection and never responds hangs with no
  window and no output.

### Evidence

- A headless mixed-lane test in `tile_surface`'s existing test module: a row
  split with a `data:text/html` left tile and a gemtext right tile, asserting
  two content layers with non-degenerate rects and a correct `inspect_tile`
  report for each. This mirrors `mixed_document_and_external_tiles`.
- A `--features tiles,smolweb` capture of the four-protocol window.
- Clicking a root-relative gemtext link navigates.

### Stop rules

- Stop if the picker cannot accept a `data:text/gemini` URL. Without it the
  mixed-lane test needs a live capsule and stops being a unit test, which
  removes the receipt this gate is built around.
- Keep the picker in `genet-documents`. `tile_surface.rs` is already 1411
  lines against a 600-line ceiling; the tile-side change is ~15 edited lines.

### Removal receipt

The `LoadedDocument` field type. The `if let Ok(doc)` swallow. The
`is_smolweb_url` scheme list if it is replaced by a shared predicate - without
misfin.

---

## G5. One palette

**Cost: a day. Real. The first thing it does is a dependency fix.**

### Outcome

The chrome strip, the tab bar, the dividers and the status strip share one
derived palette instead of nine independently chosen greys (`#2b2b33`,
`#444444`, `#888888`, `#33333a`, `#2a2a30`, `#4a4a55`, `#1a1a1f`, `#cccccc`,
`#999999`).

### Work

- **First:** point cambium at the workspace tinct and illume. Change
  `components/cambium/cambium/Cargo.toml:24-25` to `workspace = true`. The
  workspace declarations carry both `version` and `path`, so a `publish = true`
  crate still publishes. Until this lands, `components/tinct` has zero
  consumers and any tinct work in pelt produces two `tinct` crates and a type
  error.
- **Second:** fix `TileSurface::set_theme`'s `truncate(1)` to `truncate(2)`, or
  the theme must re-inline all of `FRISKET_CSS`. This is a Rust change, and
  without it the gate breaks tile rendering.
- Add a `theme.rs` to pelt-desktop: two or three named seed sets, `seeds() ->
  tinct::Seeds`, `chrome_css(&Palette)` and `tile_css(&Palette)` emitting the
  selectors the hardcoded constants already use. Woodshed's
  `woodshed-views/src/theme.rs` is the template.
- Call the seams. `Chrome::add_stylesheet` (defined, zero callers) appends and
  is safe. `TileShell::set_theme` is safe only after the truncate fix.
- Keep the seed sets hardcoded. Do not build theme-file loading.

### Evidence

- `cargo tree -p cambium | grep tinct` shows one tinct, from the path.
- A `--tiles` capture after `set_theme` still paints its documents. This is the
  specific regression the seam would otherwise cause.
- Two captures of the same page under two seed sets.

### Stop rules

- Stop if pointing cambium at the workspace copies breaks anything downstream.
  The registry copies are pinned `=0.0.2` / `=0.1.2` and the path copies claim
  the same versions, but they are the development copies and may have drifted.
  If they have, ship G5 with a pelt-local palette module and no tinct, and file
  the split.
- Do not attempt the highlighted omnibar. See deferrals.
- Keep `theme.rs` under 600 lines.

### Removal receipt

The nine hardcoded greys, replaced by derived values. `DEFAULT_CHROME_CSS` and
`DEFAULT_TILE_CSS` survive as the base sheets the theme layers over.

---

## G6. The HTML page

**Cost: a real gate, several days. Land it before G7, not after.**

### Outcome

An HTML document with a stylesheet renders as a page: text aligns where it was
told to, hidden things stay hidden, and links look like links.

### Ordering, and why this precedes subresource loading

Turning on linked stylesheets first makes the screenshot **worse**, not better.
`visibility: hidden` is never read, so every offscreen menu, closed dropdown
and inactive tab panel a real site's CSS hides would paint on top of the page.
`text-align` is hardcoded `Alignment::Start` at all four parley call sites, so
every centred hero heading and right-aligned price column would render flush
left. These are the properties a fetched stylesheet exercises. Fix the reads,
then turn on the fetch.

### Work

- `text-align`. Plumb an `align` field through `InlineContent` from
  `cv.get_inherited_text().text_align`. **The `no_wrap` pattern is not a
  known-good shape to copy**: nowrap is signalled by `max_advance = None`, and
  parley computes free space from the line box, so a nowrap element cannot be
  aligned at all. Moving nowrap onto parley's `StyleProperty::TextWrapMode` and
  passing the real width changes measured widths for every existing nowrap
  test. Budget the re-bless. `no_wrap` is also hardcoded `false` for block
  pseudo content and list markers, so a verbatim copy leaves those start-aligned.
- `visibility: hidden`. **Not** an early-return beside `display: none`.
  Visibility inherits and a descendant may re-set `visible`, so this is
  per-box paint suppression threaded through `walk` - suppressing background,
  border, shadow, marker, glyphs and replaced content while still recursing.
  `genet-livery`'s implementation is a subtree prune and is not a reference for
  this. Decide hit-testing explicitly: a hidden box keeps its full geometry, so
  paint-only suppression ships an invisible menu that still swallows clicks.
- UA sheet holes: `a[href]` colour and underline, `[hidden] { display: none }`,
  monospace for `code`/`pre`/`kbd`/`samp`, borders and padding for form
  controls. Every added rule shifts boxes in every fixture; expect a re-bless.
- `is_link`. `adapter_stylo.rs:616` is `fn is_link(&self) -> bool { false }`, so
  `a:link`, `a:visited` and `:any-link` can never match. Sites style links
  overwhelmingly through those, so this is not a caveat on the UA sheet item, it
  is a bigger gap than the UA sheet item.
- Legacy `clip`. A ~30-line paint read gated on out-of-flow, no layout
  dimension. Covers `.sr-only` / `.screen-reader-text` / `.visuallyhidden`.
  Narrower than it sounds, because the variants that pair `clip` with
  `width:1px;overflow:hidden` already collapse. Note `clip` does not reach
  out-of-flow descendants through `abs_clips`; defer that deliberately.
- Document scrollbar. `IncrementalLayout::append_scrollbars` is written and
  unit-tested and called from nowhere. **Do not do it inside
  `LoadedDocument::frame`** - that is the reftest entry point. Add a new entry
  point. Note this reaches `--engine static`, `--chrome` over HTML and
  `--tiles`, but **not** smolweb, which renders through `scene_from_packet`.

### Evidence

- Local fixtures per property, not a real site.
- A page with a `visibility: hidden` menu does not paint it, and a click over
  where it was does not land on it.
- A page with `text-align: center` centres, including a `white-space: nowrap`
  element.
- Full reftest re-bless, recorded.

### Stop rules

- Stop `text-align` at `Start`/`Middle`/`End` if `Justified` interacts badly
  with the per-line narrowed boxes in `break_and_align_floats`.
- Stop the whole gate if the 7 committed `.scene` fixtures do not currently
  pass. They were blessed at `3f73b93c393` and Livery/Buckram have moved a lot
  since; if they are already red this is a rebless decision before it is an
  implementation decision, and that is the owner's call.
- Route `text-transform`, `text-shadow`, `text-indent`, `word-break`,
  `overflow-wrap`, `vertical-align` and `outline` out of this gate. They are
  real gaps and none of them scrambles a page.
- Route `transform-origin` and percentage `translate` out. Both are real - the
  modal-centring idiom is a no-op today - but they only bite once a real
  stylesheet is loading.

### Removal receipt

The four literal `Alignment::Start` constants. The single `display: none`
paint guard, replaced by a two-condition one.

---

## G7. Subresources

**Cost: a real gate. Sized "hours at a single chokepoint" and it is a day
minimum.**

### Outcome

`pelt --features netfetch https://merelyllc.com` loads the site's own
stylesheets and images.

### Work

- Keep the fetched URL as the document base. `LoadedDocument` has no base
  field; the scripted lane already has one and could be the reference.
- Fetch linked sheets through `linked_stylesheets_with_loader`, which exists,
  is tested, and whose docstring names "the viewer's netfetcher-backed loader"
  as its intended caller. It has no non-test callers. Write a loader adapter
  that joins the raw href against the base.
- **Interleave inline and linked sheets in document order.** Two independent
  walks concatenated inverts the cascade for the normal real-site shape, because
  vector position is the order-of-appearance tiebreaker at equal specificity.
  genet-wpt has the same bug and has never exercised it.
- **Honour `media`.** The walker reads only `rel` and `href`, and every sheet is
  parsed with `MediaList::empty()`, which matches all media. A fetched
  `media="print"` sheet applies to the screen, and print sheets routinely hide
  navigation and force black-on-white. This is the trap that turns the fix into
  a visibly wrong screenshot.
- Widen the `rel` match to whitespace tokens, the way `linked_icon_href`
  already does.
- **Do the scripted lane too.** `ScriptedDocument::build` assembles sheets the
  same broken way and drives its own sessions. It is not in any proposal's file
  list.
- Images: `IncrementalLayout::new_with_resources(..., base_url, loader)` with
  the existing `new` delegating. Seven sites need the loader, not six - the
  splice-graft path at `incremental.rs:2245` is the one that gets missed.
  `<img src>` arrives raw and `background-image` arrives cascade-resolved, so
  the adapter handles both. Note inline `style="background-image:url(...)"`
  resolves through a *different* url_data built inside `cascade_traverse`, where
  every retained path hardcodes `None`; fixing that changes three public
  `genet-layout` signatures. Scope it explicitly or defer it explicitly.
- Turn `netfetch` on by default, or the lane stays invisible. `url` is an
  optional dependency of `genet-documents`, so the adapter needs a gate or the
  dep needs promoting.

### Evidence

- A capture of one real site before and after, same size.
- A local page with a `<style>` block before a `<link>` cascades in the right
  order.
- A page with a `media="print"` sheet is unaffected on screen.
- `--engine scripted` gets the same treatment, proven by fixture.

### Stop rules

- **Stop and reassess if the target sites answer netfetcher's User-Agent with a
  403 or a challenge.** It sends `Mozilla/5.0 (compatible; serval netfetcher)`,
  a non-browser UA that CDN bot filters commonly reject. Check this *before*
  writing any of the above. It is a plausible hard blocker that no amount of
  stylesheet wiring fixes.
- Stop if a target site gates its design system behind one `@import`. `@import`
  is inert (`Stylesheet::from_str` gets `None` for the loader with
  `AllowImportRules::Yes`), so the page still looks unstyled and the gate reads
  as failed. Check the sheets first.
- Cap the number of sheets fetched and set a per-request timeout before this
  reaches the interactive profiles. The load already blocks the event thread.
- **Watch the resize regression.** `ensure_session` rebuilds the whole session
  on every size change and `frame()` runs with the live size during a resize
  drag. Today `sheets` is three tiny strings so that is free; holding a real
  site's stylesheets makes every resize step re-parse hundreds of KB. This is
  caused by G7, not by the network, and no lane mentioned it.

### Removal receipt

`inline_stylesheets`-only sheet assembly in both `LoadedDocument::parse` and
`ScriptedDocument::build`. `NoImageLoader` at the seven retained-session sites.

---

## G8. DPI

**Cost: a real gate. One coherent pass or none.**

### Outcome

Pelt renders at the display's scale factor.

### Work

There is no `ScaleFactorChanged` handling anywhere in `ports/`, windows are
created in `PhysicalSize`, and physical `inner_size()` is fed straight into
layout. This machine reports `LOGPIXELSX=192` on 2560x1600, so every pelt
screenshot taken here is rendered at exactly half the intended size: the 40px
chrome strip, the 16px text and the 26px tile status bar all come out at half
the physical size a real browser would produce in the same image.

Read `scale_factor()` at creation, handle `ScaleFactorChanged`, and scale
layout, rasterization, hit-testing, `content_area_rects`, the drag threshold and
the a11y bounds in one pass.

`TileShell::set_ui_scale` is **not** the path, despite existing and having zero
callers. `ui_scale` is read at exactly three places, all transient drag chrome:
the ghost's size, the ghost's cursor offset, and the drag-arm threshold. It
never reaches layout or rasterization, so feeding it the scale factor changes
nothing a screenshot can see.

### Evidence

- A capture on the 200% laptop and one on a 100% display produce the same
  apparent text size relative to the window.
- Clicks land where they look, at both scales, in `--chrome` and `--tiles`.

### Stop rules

- **Stop if it cannot be one pass.** Scaling the render but not the hit test is
  worse than not doing it: clicks land off-target across the whole surface.
- Check whether stylo's `make_device` can carry a device-pixel-ratio before
  writing three per-viewer changes. If it can, this is a cascade-level change.
- Named partial, if the gate is not taken: pelt is correct today on a
  100%-scale display. Screenshots taken on the iMac or an external 100%
  monitor are honest. That is a legitimate stopgap and should be written down
  rather than discovered.

### Removal receipt

`PhysicalSize` at the three window builders. `set_ui_scale` if the pass
subsumes it.

---

## G9. Captures that assert

**Cost: hours. No screenshot delta. Insurance.**

### Outcome

A capture proves what it captured, and a windowed profile cannot ship broken.

### Work

The scenario harness is a smaller job than the testing lane sized it, because
`capture.ps1` already does `PrintWindow` with `PW_RENDERFULLCONTENT` under
per-monitor DPI awareness. It does not race the foreground and cannot
photograph the wrong window.

- `--frames N` on the windowed profiles: open the real window through the real
  `run_*` path, present N frames, exit 0. Half of it exists unused:
  `StaticViewerConfig::exit_after_first_redraw` is defined, read, and set by
  nothing.
- Note the tiles panic would have been caught by `--frames 1` with **no**
  synthetic input. `SubclassingAdapter::new` panics eagerly on
  `IsWindowVisible`, and pre-fix `sync_a11y` ran unconditionally from `render()`
  on the first RedrawRequested. The "input" in the commit message reads more
  naturally as input *documents*. So the frame count is the load-bearing part
  and the synthetic click is a bonus.
- Capture the **client** rect, not `GetWindowRect`, so the title bar and borders
  stop appearing in shots.
- If a scenario lane is wanted, follow woodshed's convention exactly:
  `PELT_SCENARIO` / `PELT_CAPTURE_DIR` / pinned size, one step per presented
  frame, in-process readback via netrender's `read_rgba8_texture` (which pelt's
  PNG lane already uses, so the padded-row dance woodshed wrote by hand is
  avoidable), and a `scenario.done` whose first line is `RESULT ok|fail`.

### Evidence

- `pelt --tiles a.html b.html --frames 3` exits 0 in CI.
- Reverting `4f72666fe5c` locally makes it go red. Confirm this before
  building anything else in this gate.

### Stop rules

- Stop the scenario lane if `ProbeSurface`'s single `sheet: &'a str` blocks it.
  `TileSurface` lays out under two sheets and the field is private with no
  accessor; widening `ProbeSurface` is a cross-repo change, because woodshed
  and turnstone take genet-probe by git branch.
- Do not build a frame-count smoke as the permanent answer. It is a guess about
  someone else's timing, which is the failure genet-probe's `wait`/`busy()`
  exists to replace.

### Removal receipt

`exit_after_first_redraw`'s dead state, now reachable. The zeroed `Headless`
arms of `run_tile_viewer` / `run_static_viewer`, which build a tree and discard
it.

---

## Deliberate deferrals

Named, with the reason, so they are choices rather than omissions.

**The highlighted omnibar.** illume's URL token is https-only and tinct has one
`Url` role. Four of five smolweb shots get nothing, and no shot gets a
scheme/host split. Delivering the thing that was described needs new roles in
tinct plus a new lexer pass in illume, both crates published, both consumed
from crates.io by cambium.

**A settings page.** `SettingsProvider` has zero implementors, there is no
persistence layer anywhere reachable from pelt, and `--tiles` has no keyboard
input at all: `tile_viewer.rs` has no `KeyboardInput` arm and `TileShell`
exposes no key entry point, so a `text_field` needs winit key translation built
from scratch plus per-pane hit-test dispatch plus focus arbitration. A
read-only settings page that resets on restart is a placebo. The direction doc
(`2026-07-24_pelt_knot_direction.md:47-63`) already ruled settings out of the
shared content-class layer and recorded pelt's settings dying unported.

**An inspector pane.** `ContentReport` is real, built and tested, and G4 makes
it answer for capsules too. But it does not change a screenshot the way G3 and
G4 do, and it shares its integration point with the settings pane. Worth doing
after G4; not before.

**Collapsing the duplicate `SmolwebTheme`.** Two implementations, identical hex
palettes, drifted stylesheets. The right call is to collapse them, and doing it
now means auditing `cambium-nematic`'s consumers in sibling repos that were not
searched. Instead: this plan pins which copy pelt uses, once, at the top.

**`@font-face` and webfonts.** `font-face` does not exist anywhere in
`genet-layout`. There is nothing to fetch *for*. A site whose identity is its
typeface will still look wrong after G7, and that is worth knowing before a
screenshot session rather than during one.

**`@import`.** Inert. A site that gates its design system behind one `@import`
still renders unstyled after G7.

**Async loading, a progress indicator, and Stop.** `C::load` is synchronous and
blocks the event loop, so there is no frame in which to paint a spinner. Per
the no-placebo rule, a fake one is not an option and a Stop button with nothing
to stop must not ship.

**Zoom.** No scale parameter in `IncrementalLayout::new`; `zoom` is explicitly
in the parsed-but-unread set. Blocked behind G8.

**History and bookmarks persistence.** Would introduce the first persistence
layer in the repo, and per the 2026-07-25 resolution the record shape must line
up with turnstone's content classes, which do not exist in genet. Pelt would be
inventing the schema it is supposed to conform to.

**Reftests in `cargo test`.** The `.scene` fixtures embed font-face-specific
glyph ids from Windows system-font discovery and none opts into the bundled
Ahem face, so this test goes red on a Linux CI runner. Making it portable is a
fixture-authoring decision, not a `#[test]`.

**Hover states and cursor shapes.** No still-image delta, and `Chrome::hit_test`
builds a fresh `IncrementalLayout` per call, so hover-on-mousemove is one full
cascade and layout per pointer motion event. The hand cursor over links is the
strongest live-demo signal available; it is not a screenshot item.

**IME.** `genet_render::caret_screen_rect` exists specifically to feed
`set_ime_cursor_area`, and cambium has a full composition model, but
`chrome_viewer` has no `Ime` arm and `Key::Dead(_)` returns `None`. The omnibar
cannot accept CJK or dead-key input. Real, and not presentability.

**Exposing Livery as a CLI engine profile.** It already does base-resolved
subresource fetching and registers font bytes into parley, which would shortcut
G7 and the webfont deferral. Nobody assessed its layout maturity against the
stylo lane on real CSS, so this is a research item, not a gate.

## What is too uncertain to plan around

Stated rather than guessed at.

1. **Whether a real site looks right once its CSS loads.** stylo's parse of
   production CSS - custom properties, nesting, `@supports`, `@layer`,
   container queries - has never been exercised outside WPT fixtures and
   hand-written test CSS. That is the entire payoff of G7 and nobody measured
   it. It is a rendering-fidelity unknown, not a fetch one.
2. **Whether the target sites will serve pelt at all.** The netfetcher UA is
   non-browser. Check before G7, not during.
3. **Whether the 7 committed `.scene` fixtures pass today.** Last blessed at
   `3f73b93c393`. G3, G6 and G7 all re-bless them. If they are already red, the
   baseline is a content judgement the owner has to make first.
4. **The real cost of `text-align`.** The nowrap/`max_advance` collision means
   the honest range is two days to a week depending on how many nowrap tests
   move. G6's size is the least reliable number in this plan.
5. **Whether pelt's tile documents can report quiescence.** G9's `busy()` needs
   an in-flight signal from `LoadedDocument` that may not exist. If it does not,
   `busy()` returns `None` loudly rather than a comfortable lie.
6. **Whether netfetcher's connection pool survives across `block_on` calls.**
   The bridge is a current-thread runtime and hyper-util's keepalive tasks are
   not polled between calls. A serial subresource loop may re-handshake TLS per
   resource. This was asserted as "not a latency cliff" without evidence and
   could flip G7's sizing.
