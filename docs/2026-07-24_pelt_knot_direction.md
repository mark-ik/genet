# Pelt and Knot Direction

**Date:** 2026-07-24
**Status:** direction, discussed with Mark 2026-07-24. No code task; records
product posture, two rulings, and one queued receipt. Companion to the
[pelt port boundary](2026-07-24_pelt_port_boundary.md); the mere-side context
is `design_docs/2026-07-24_application_prospects_brief.md` in that repo.

## One engine, three prospects

**The embeddable engine is the flagship.** The product is the component
stack behind `genet-host-api`, not pelt; the boundary doc's direction rule
(components never depend on ports) is what keeps that true. It is
multiprotocol through the host-supplied fetch seam (http(s), smolweb, later
mesh-side protocols supplied by the host), and natively automatable:
genet-probe carries drivability in the DOM itself, where every other
embeddable engine bolts automation on through CDP or the accessibility tree.
Publishing waits on livery obviating stylo (the
[cutover plan](2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md));
the [publish rings plan](2026-07-11_genet_publish_rings_plan.md) applies
after that.

**Pelt is the thin sovereign browser.** Reference host and acceptance runner
first (servoshell posture), with room to grow daily-drive chrome after the
cutover. It stays graph-free: the dependency-cone witness already keeps mere
and turnstone off pelt, and the reverse discipline (pelt grows no mere
dependency) is deliberate. History-as-graph belongs to turnstone embedding the
same engine; pelt and turnstone are two hosts of one engine, not rivals.

**Knot's destination is the authoring browser, not an IDE.** The point
between an IDE and a browser is the read-write document, and inker already
holds the unusual parts: transclusion, evaluated blocks, block provenance. A
full IDE (LSP, debuggers, a project model) is a large lift with entrenched
incumbents and none of the stack's differentiators; the differentiated
destination is browse a document, open source beside live preview, edit,
transclude, publish.

## The 80 percent split (ruled)

Daily-drive browser features split two ways:

- **Engine-side fact surfaces, in components, below the cone line.** Cookies,
  cache, and auth belong to netfetcher; certificate facts surface at the
  fetch seam; downloads are a netfetcher stream with progress facts;
  find-in-page is a document capability behind `genet-host-api`. Every host
  (pelt, mere, turnstone) benefits without depending on another product.
- **Per-host persistence, deliberately divergent.** History, bookmarks,
  sessions, settings. Turnstone persists them as graph engrams, which is its
  identity; pelt persists a local file. The shared piece is the
  navigation-event facts the host contract already implies.

  **Resolved 2026-07-25: the schema is the shared basis.** Mere needing the
  graph and these components to be synonymous is satisfied at the
  content-class layer, not the storage layer. One class definition per kind
  (facets plus engram schema), two materializations: a bookmark is the same
  declared thing in both hosts, stored as a chartulary engram in turnstone and
  as a local file record in pelt. The split above stands unchanged.

  Settings are the exception and get no class. History, bookmarks, and
  sessions are about pages, which are already nodes, so a content class fits
  them. Settings are host configuration, and settings-as-nodes was tried and
  came back a bust in the pane taxonomy revision, with pelt's settings dying
  unported.

This resolves "who builds the boring 80 percent": the majority is component
work that serves turnstone first, and pelt's private remainder is small.

## Text editing (ruled): one primitive, three consumers

Editable text is owed three times over: cambium's `text_input` needs real
selection, IME, and undo to be a credible toolkit control; the fullweb lane
needs `input` and `textarea` for real pages, and contenteditable after that;
the knot editor needs the same machinery. Build it once at the cambium/genet
primitive layer and keep `knot-editor-host` a consumer, which it already is
architecturally. This is the gate for the authoring browser; the promised
nematic lexers (CSS/HTML/JS/Turtle) are small beside it. Owner: the
[text-editing primitive plan](2026-07-25_text_editing_primitive_plan.md),
founded 2026-07-25.

## Queued receipt: an agent drives pelt

Done-condition: an agent completes a scripted task in a headed pelt through
genet-probe on a constrained page (load, resolve targets by DOM-carried
identity, act, assert the outcome), recorded as a receipt. The cone is
genet-probe, the scripted runtime, and pelt-desktop, disjoint from the
cutover's cascade and layout cone, but it is queued deliberately so it does
not compete with livery focus. It does not wait on fullweb, and it turns
"the engine agents can drive" from a claim into evidence.

## Knot as the commons page format (candidate)

Mere-side candidate decision: knot documents as the "page" content class for
the shared-engram commons (small textual diffs make small sync ops, fit for
LoRa budgets; one document renders in pelt over http and in turnstone over the
mesh). Recorded in mere's
`design_docs/mere_docs/research/2026-07-24_shared_engram_commons_brief.md`;
genet's stake is only that the editing primitives above arrive.
