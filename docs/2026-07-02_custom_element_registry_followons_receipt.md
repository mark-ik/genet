# Custom Element Registry Follow-ons Receipt

Date: 2026-07-02

## Scope

Closed the next contained HTML interface table follow-ons after the
`adoptedCallback` slice:

- tighter `CustomElementRegistry` contract behavior in the JS bootstrap,
- shared `HTMLMediaElement` table inheritance for audio/video.

This receipt does not claim parser-timed construction, full constructor-failure
semantics, Shadow DOM registry features, or `ElementInternals`.

## Landed Behavior

- `customElements.define()` now rejects uppercase and reserved custom-element
  names, keeps the element-definition-running guard, and preserves the expected
  constructor/prototype property access order for callback probing.
- `customElements.define()` reads `observedAttributes` only when
  `attributeChangedCallback` exists, and still reads `disabledFeatures` /
  `formAssociated` in the expected order.
- `customElements.whenDefined()` now returns one stable pending promise per
  unresolved valid name and rejects invalid names.
- `customElements.getName()` now throws `TypeError` for non-constructors.
- `HTMLVideoElement` and `HTMLAudioElement` now inherit from a shared
  `HTMLMediaElement`, making `instanceof HTMLMediaElement` true for both.

## Validation

```powershell
cargo fmt -p script-runtime-api

cargo test -p script-runtime-api html_interface_table_on_boa --lib
cargo test -p script-runtime-api custom_elements_registry_contract_on_boa --lib
cargo test -p script-runtime-api custom_elements_registry_contract_on_nova --lib
cargo test -p script-runtime-api custom_elements_customized_builtins_on_boa --lib
```

All passed locally.

## Remaining Gaps

- Parser-created custom-element timing.
- Fuller constructor-failure / `NewTarget` / reentrancy semantics.
- Shadow DOM and scoped-registry adoption/registry cases.
- `ElementInternals` and form-associated custom elements.

## Status

Complete for the contained registry-contract and media-inheritance follow-ons.
The remaining HTML interface table work is the larger parser/constructor/shadow
surface.
