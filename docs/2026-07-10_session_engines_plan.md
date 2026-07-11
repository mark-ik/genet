# Session engines: one formal contract for the content lanes

**Date:** 2026-07-10
**Status:** plan, proposed. API shapes verified against source this session;
no code yet.

Companion to
[2026-07-09_inker_serval_adoption_plan.md](./2026-07-09_inker_serval_adoption_plan.md)
(this doc is its done condition 6, widened per Mark to a formalization pass
across serval, inker, nematic, and errand) and
[2026-07-09_native_automation_plan.md](./2026-07-09_native_automation_plan.md)
(the settle/observe seam below is that plan's quiescence contract, not a
second invention). Supersedes the "errand/nematic concern reorg" open
question in the adoption plan: this is that doc.

Code samples are illustrative unless marked implementation-ready.

## The observation

Pelt carries three "convenience lanes" that are one unspoken contract. As
verified in source:

| | LoadedDocument (static) | ScriptedDocument | SmolwebDocument |
| --- | --- | --- | --- |
| construct | `load` / `parse` | `load` / `parse` / `from_body` | `load` / `parse` |
| render | `frame(w, h) -> Scene` | `frame(w, h) -> Scene` | `frame(w, h) -> Scene` |
| scroll | `scroll_by` / `scroll_at` / `scroll_for_key` | same | same + `scroll_to` |
| activate | `click_at -> ClickOutcome` | `click_at -> bool` | `click_at -> Option<String>` |
| links | (via session) | `links()` | `links()` |
| tick | — | `pump(now_ms)`, `has_pending_work()` | — |
| visibility | — | `set_hidden` | — |

meerkat's content actor dispatches these as a cfg-and-if ladder
(`handlers.rs::render`), and the inker registry only ever sees nematic's
block engines. The `serval.*` routing ids resolve to nothing. Every new host
(merecat first) would re-write the ladder.

The reason the ladder exists: neither of inker's two engine kinds fits.
`Engine` returns `EngineDocument` (blocks — forcing HTML through it is the
lowest-common-denominator mash already rejected for smolweb), and
`SurfaceEngine` produces GPU textures from external producers. The lanes
produce **paint scenes from retained layout sessions**. That is a third kind,
and it deserves a first-class contract, not three lookalike structs.

## The formal model

Every content engine is classified by output type and lifecycle:

| Kind | Trait | Lifecycle | Output | Examples |
| --- | --- | --- | --- | --- |
| Document engine | `Engine` (exists) | request/response | `EngineDocument` (blocks: serializable, storable) | nematic formats, knots, clips, rhai outputs |
| Session engine | `SessionEngine` (new) | retained session | paint frame (`Scene`) + interaction | serval static, serval scripted (Boa/Nova), smolweb native |
| Surface engine | `SurfaceEngine` (exists) | external producer | GPU texture stream | scrying, graft, weld |

The three kinds rhyme deliberately: same registry pattern, same routing
integration (an `EngineRouteDecision` carries an id; the host asks one
facade which kind holds it), same a11y-capability declaration, same
per-engine feature gating in consumers.

This also settles the blocks question structurally: blocks are the
**stored/authored** output (what persists, ships to workers, feeds
linked-data statements), sessions are the **live** output. Neither is a
degraded form of the other, and no lane renders live content through blocks.

## The SessionEngine contract (illustrative)

```rust
/// Spawns retained document sessions for the engine ids it claims.
pub trait SessionEngine<F>: Send + Sync {
    fn engine_id(&self) -> &str;
    fn spawn(&self, request: &SessionSpawnRequest)
        -> Result<Box<dyn DocumentSession<F>>, SessionError>;
    fn a11y_capability(&self) -> A11yCapability { A11yCapability::Partial }
}

/// A live document: retained layout session producing paint frames.
/// Not Send by default; the host drives it from its content thread,
/// exactly as the pelt types are driven today.
pub trait DocumentSession<F> {
    fn frame(&mut self, width: u32, height: u32) -> F;
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool;
    fn scroll_for_key(&mut self, key: ScrollKey) -> bool;
    fn click_at(&mut self, x: f32, y: f32) -> SessionClick;
    fn links(&self) -> Vec<LinkHit>;

    // The scripted lane's extras, defaulted so static lanes ignore them.
    // `settled` is the automation plan's quiescence contract surfacing here.
    fn pump(&mut self, now_ms: f64) {}
    fn settled(&mut self) -> bool { true }
    fn set_hidden(&mut self, hidden: bool) {}
}
```

Two deliberate choices:

1. **Generic frame type `F`.** inker has zero paint dependencies and keeps
   them: the registry is `SessionRegistry<F>`, and a host instantiates
   `F = netrender::Scene`. No netrender edge lands in the contracts crate;
   a future gpui-native or wasm host picks its own frame type.
2. **`SessionClick` is a small enum** (navigate-to-url, handled-internally,
   miss), unifying the three lanes' divergent click returns
   (`ClickOutcome` / `bool` / `Option<String>`).

## What changes where

**inker** gains `session_engine.rs` (traits above + `SessionRegistry<F>`)
and a kind-resolution facade so hosts stop hand-matching: given an engine
id, answer document / session / surface / host-handled. Routing is
untouched; decisions already carry ids.

**serval** gets the formalization Mark asked for: the three lane types
promote OUT of pelt into a new component, **`components/serval-documents`**
(review decision 2026-07-10: "session" is load-bearing family-wide for
app/persistence sessions — mere's session-runtime, merecat's session.rs,
"window = graph-shaped session" — and the trait is already
`DocumentSession`), that implements `SessionEngine` for
`serval.web`, `serval.scripted`, `serval.scripted.nova`, and the smolweb
lane, behind the same feature ladder pelt uses today (tile-surface /
scripted / scripted-nova / smolweb). Pelt returns to being a thin reference
shell that consumes the component like any other host. The convenience
lanes stop being pelt's private vocabulary and become engine-grade
components.

**nematic** (follow-on phase, direction already decided 2026-07-10): absorbs
smolweb-views and becomes the whole smolweb engine family with two products,
`nematic::blocks` (the existing `Engine` impls, for stored/clip/summary
content) and `nematic::views` (the native per-format views that
serval-documents' smolweb session renders through). Per-format features so
apps pick gemini in, spartan out. Block lowering stops being the render
path anywhere.

**errand** absorbs the wire-shaping still in nematic (finger response
shaping), completing "nematic never touches bytes."

**meerkat** collapses the content-actor ladder into registry dispatch: route
the address, resolve the kind, spawn or fetch accordingly. Behavior
identical, receipts via the existing 82+247 suite plus the apparatus scene
checks.

**merecat** consumes the same registries for its content lane, per its
architecture plan's sequencing note. It never learns the ladder existed.

## What deliberately does not change

- `Engine`/`EngineDocument` stays exactly as is: the stored/authored lane
  is load-bearing (knots, clips, rhai outputs, statements walking,
  worker-shippable packets).
- `SurfaceEngine` stays exactly as is; scry/graft/weld are unaffected.
- Routing vocabulary and `mere::routing` are untouched.
- Blocks do not gain view/scene ambitions, and sessions do not gain
  serialization ambitions. The split is the design.

## Phases and done conditions

1. **inker session contracts.** Traits + `SessionRegistry<F>` + kind facade
   land with unit tests; no consumer yet. Done when inker tests cover
   spawn/dispatch/kind-resolution and the crate still has no paint deps.
2. **serval-documents component.** The three pelt types move, implement the
   traits, pelt consumes the component. Done when pelt's viewers and
   reftests are green against the component and pelt-desktop no longer
   defines the document types.
3. **meerkat rides the registry** — RESCOPED per review 2026-07-10 to the
   **render + input path only**. The ladder in `content/handlers.rs` and
   the scripted/smolweb spawn special-casing reduce to registry dispatch;
   `engine_present` stops special-casing `serval.*`. The **observation
   half** (ScriptedDocument::extract, selection/find primitives, the
   engine-subtree a11y walk, dispatch_event/dom_snapshot) deliberately
   keeps concrete-type access through `as_any` downcasts until the
   observation contract lands — that contract is the native automation
   plan's snapshot/drive side, and extending `DocumentSession` ahead of it
   would design it twice. Trigger, recorded family-style: when the
   automation plan's observe/actuate core specifies its read surface, the
   downcasts convert to that contract and this phase's residue retires.
   Done when meerkat's suite is green and no `ENGINE_SERVAL_*` match arms
   remain in the render/input dispatch (observation downcasts are counted
   and listed, not hidden).
4. **merecat content lane.** merecat spawns sessions through the same
   facade for its first web render. Done when the merecat vertical slice
   renders an https page through a registry-dispatched session.
5. **nematic views + errand wire absorption.** smolweb-views merges into
   nematic as `nematic::views`; finger shaping moves to errand; the smolweb
   session in serval-documents consumes nematic views for all formats it has
   views for. Done when smolweb-views is retired as a separate component
   and native coverage includes the wrapper protocols via gemtext/markdown
   view reuse.

## Open questions (review-resolved items recorded 2026-07-10)

- ~~Component naming~~ RESOLVED: `serval-documents` (see above).
- ~~Cookie/fetcher seams~~ RESOLVED in the same spirit as the leaning: one
  construction path, realized as **engine-held seams at registration** (the
  concrete `SessionEngine` is constructed with its fetcher/jar/theme; the
  spawn request stays serializable plain data; nothing is host-installed
  post-spawn).
- ~~Engine activation~~ RESOLVED: stays host-side. It is app state
  (meerkat's node colors ride it), and inker staying policy-neutral is its
  identity. The kind index is a map, deliberately non-generic.
- ~~Worker story~~ RESOLVED: no collision; merecat's content lane starts on
  the shell thread, and the worker lane keeps content-contract packets.
- Script-initiated navigation (`location.href` and successors): the
  scripted lane does not surface it yet, but it is the next thing after
  forms, and `SessionClick` must not ossify as the only navigation channel.
  A session-emitted navigation event (polled like `pump`, or returned from
  it) is the likely shape; design when the scripted lane grows it.

Phase 4 verifier, noted 2026-07-10: merecat's scenario lane landed (open
url, settle, GPU self-capture, assert — no synthetic input), so "renders an
https page through a registry-dispatched session" has a deterministic
receipt, not a screenshot-by-hand.
