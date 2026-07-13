# Architecture

Cambium is an application toolkit over Genet. Meristem produces and reconciles
view structure. Cambium translates that structure into Genet's neutral DOM,
custom-leaf, presentation, and document-engine seams.

The dependency direction is one-way:

```text
applications -> Cambium -> Genet seams -> rendering and platform

Genet engine crates -X-> Cambium
```

## Ownership

- Meristem owns reactive diffing, messages, view identity, and view sequences.
- Cambium owns application views, controls, composition, and Genet adapters.
- Sprigging owns retained custom-leaf state and arrangement helpers.
- Genet owns DOM, style, layout, paint, input, accessibility, and browser
  behavior.
- Nematic and other document engines own parsing and protocol-faithful lowering.

Genet remains independently usable without Cambium. Sprigging is an extension
of Genet's neutral custom-leaf seam, not a second layout or input engine.

The checkout and published seam crates still use `serval` names. Those are
current compatibility identifiers, not the product ownership term.
