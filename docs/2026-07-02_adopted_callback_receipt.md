# AdoptedCallback Receipt

Date: 2026-07-02

## Scope

Closed the missing core `adoptedCallback` lane in the HTML interface table
bootstrap for normal document-to-document adoption:

- cross-document insertion via `appendChild` / `insertBefore`,
- `Document.adoptNode`,
- `ownerDocument` tracking for detached nodes created or adopted into non-primary
  documents,
- ancestor moves that carry upgraded custom-element descendants.

This receipt covers the minimal document-adoption slice. It does not claim
Shadow DOM, scoped registries, or the broader `customElementRegistry`
adoption semantics.

## Landed Behavior

- `Node.prototype.ownerDocument` now resolves from the current root document
  when connected, otherwise from the last creating/adopting document.
- `Document#createElement`, `createElementNS`, `createTextNode`,
  `createComment`, and `createDocumentFragment` stamp detached nodes with the
  creating document.
- Cross-document insertion updates inclusive descendant ownership and enqueues
  `adoptedCallback(oldDocument, newDocument)` through the existing custom-element
  reaction queue.
- Connected cross-document moves enqueue reactions in the expected order:
  `disconnected`, then `adopted`, then `connected`.
- `Document#adoptNode` removes a node from its parent, updates document
  ownership, and enqueues `adoptedCallback`; connected nodes also enqueue
  `disconnectedCallback`.
- `cloneNode` and `Text#splitText` now mint replacement nodes in the current
  node's `ownerDocument` instead of always using the primary document.

## Validation

```powershell
cargo fmt -p script-runtime-api

cargo test -p script-runtime-api custom_elements_adoption_on_boa --lib
cargo test -p script-runtime-api custom_elements_adoption_on_nova --lib

cargo test -p script-runtime-api custom_elements_customized_builtins_on_boa --lib
cargo test -p script-runtime-api dom_characterdata_identity_on_boa --lib
cargo test -p script-runtime-api dom_created_doc_queryable_on_boa --lib
```

All passed locally.

## Remaining Gaps

- `custom-elements/registries/adoption.window.js` still needs Shadow DOM /
  `customElementRegistry` support.
- Parser-timed custom-element construction is still a separate open item.
- Registry validation/error ordering and `HTMLMediaElement` inheritance are still
  separate follow-ons.

## Status

Complete for the minimal `adoptedCallback` / `adoptNode` / detached
`ownerDocument` follow-on. Broader custom-element adoption semantics remain
open where they depend on Shadow DOM or registry surface not yet implemented.
