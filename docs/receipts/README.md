# Component catalog receipts

These self-contained HTML files are generated from the same retained
`ScriptedDom` and application stylesheet as the executable component catalog:

- `component_catalog_narrow.html`: 420 px browser viewport, 22 rem specimen
- `component_catalog_regular.html`: 900 px browser viewport, 48 rem specimen

Regenerate them from the workspace root:

```console
cargo run -p cambium --example component_catalog -- --write-receipts
```

The example test compares both committed files byte-for-byte with fresh output.
Open them in a browser for visual review. Browser HTML preserves custom-leaf
geometry and interactive overlays; Sprigging's actual paint commands remain
covered by the fast retained-leaf acceptance assertions in the example.
