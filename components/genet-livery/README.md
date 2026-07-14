# Genet Livery

`genet-livery` is Genet's integration path for the clean-room Livery CSS
engine. It adapts any `LayoutDom` to Livery's selector substrate, resolves a
concrete Livery style plane, and lays the bounded Cambium lane out without
importing Stylo.

Fullweb documents continue through `genet-layout` and Genet Stylo. Runtime
document routing stays above both concrete paths.

The first layout cut covers the audited physical box subset. Inline formatting
and shaped text, paint emission, and session-engine registration remain before
Cambium can select this path in production.
