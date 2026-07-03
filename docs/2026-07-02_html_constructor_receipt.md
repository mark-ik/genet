# HTML Constructor Receipt

Date: 2026-07-02

## Scope

Closed the direct HTML-constructor core inside the HTML interface table
bootstrap:

- interface constructors now use the registry's constructor table,
- direct `new` on registered autonomous custom elements now mints real elements,
- direct `new` on registered customized built-ins now mints the right built-in
  tag plus `is=""`,
- wrong-base and unregistered constructor paths now fail with `TypeError`.

This receipt does not claim parser-timed construction, `Document.createElement`
failure fallback/reporting, exact proxy-`NewTarget` property-access counts, or
the broader reentrancy corner cases.

## Landed Behavior

- `bootstrap.js` now keeps a constructor-to-definition map alongside the
  existing name/local-name tables.
- `HTMLElement` and the per-tag HTML interface constructors now reject direct
  bare-interface construction and consult the registered definition for
  subclass construction.
- Autonomous custom-element constructors now infer their tag from the
  registered name instead of only working through the upgrade-time construction
  stack.
- Customized built-in constructors now verify that the active HTML interface
  matches the registered `extends` local name before minting an element.
- Direct construction of a registered plain JS constructor through
  `Reflect.construct(HTMLElement, [], PlainCtor)` now returns the underlying
  element object with the requested prototype, matching the existing
  no-inheritance custom-element bootstrap style.

## Validation

```powershell
cargo fmt -p script-runtime-api

cargo test -p script-runtime-api custom_elements_html_constructor_on_boa --lib
cargo test -p script-runtime-api custom_elements_html_constructor_on_nova --lib

cargo test -p script-runtime-api custom_elements_registry_contract_on_boa --lib
cargo test -p script-runtime-api custom_elements_registry_contract_on_nova --lib
cargo test -p script-runtime-api custom_elements_customized_builtins_on_boa --lib
cargo test -p script-runtime-api custom_elements_customized_builtins_on_nova --lib
cargo test -p script-runtime-api custom_elements_adoption_on_boa --lib
cargo test -p script-runtime-api custom_elements_adoption_on_nova --lib
```

All passed locally.

## Remaining Gaps

- `Document-createElement*` and parser-created-element fallback/error reporting.
- Exact proxy-`NewTarget` / prototype-access-count WPT fidelity.
- Constructor reentrancy corners.
- Parser-timed construction and `document.write` timing.

## Status

Complete for the direct HTML-constructor core. The remaining constructor bucket
is the failure/reporting/reentrancy side rather than bare direct construction.
