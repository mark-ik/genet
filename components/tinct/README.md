# tinct

Perceptual seed-to-palette derivation for the Merely family (Woodshed,
Strophe, merecat's mere).

> **Home:** [`mark-ik/genet`](https://github.com/mark-ik/genet), at
> `components/tinct` (adopted 2026-07). The former standalone repository is archived
> and links here.


A theme is authored as a handful of **seed** colours — a primary / secondary /
tertiary brand triad, a neutral surface hue, and a light/dark mode. The full
set of UI roles is *derived*: a perceptual surface ladder, a contrast-gated
text hierarchy, and contrast-picked `on_*` colours. Derivation runs in
**OKLCH** (perceptually-even lightness steps, unlike HSL) and gates text on WCAG
contrast.

```rust
use tinct::{Seeds, Srgb, derive_palette};

let seeds = Seeds {
    primary:   Srgb::rgb(0x33, 0x66, 0xC8),
    secondary: Srgb::rgb(0x2E, 0x9D, 0xA6),
    tertiary:  Srgb::rgb(0xE0, 0xA8, 0x46),
    neutral:   Srgb::rgb(0x10, 0x14, 0x22),
    text_header: None,
    text_body:   None,
    success: Srgb::rgb(0x4F, 0xB3, 0x6E),
    danger:  Srgb::rgb(0xD5, 0x4E, 0x4E),
    dark: true,
};
let palette = derive_palette(&seeds);
```

The crate depends only on `serde` and owns its colour type ([`Srgb`]) so hosts
on any toolkit can use it: convert the host colour type to `Srgb` for the seeds
and from the derived `Palette` back. The `oklch` module is public so a host can
derive a richer token set (extra surface steps, accent variants, hue rotations)
on the same maths the base `derive_palette` uses.

## Status

Seed → base `Palette` derivation + the OKLCH primitives. Consumers (Woodshed's
`audio_widgets`, Mere's `register-theme`) layer their own richer token sets on
top. License: MPL-2.0.
