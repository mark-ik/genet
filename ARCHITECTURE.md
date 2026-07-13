# Architecture

Cambium is an application toolkit over Serval. Meristem produces and reconciles
view structure. Cambium translates that structure into Serval's neutral DOM,
custom-leaf, presentation, and document-engine seams.

The dependency direction is one-way:

```text
applications -> Cambium -> Serval seams -> rendering and platform

Serval engine crates -X-> Cambium
```

## Ownership

- Meristem owns reactive diffing, messages, view identity, and view sequences.
- Cambium owns application views, controls, composition, and Serval adapters.
- Cambium-Chisel owns retained custom-leaf state and arrangement helpers.
- Serval owns DOM, style, layout, paint, input, accessibility, and browser
  behavior.
- Nematic and other document engines own parsing and protocol-faithful lowering.

Serval remains independently usable without Cambium. Chisel is an extension of
Serval's neutral custom-leaf seam, not a second layout or input engine.

