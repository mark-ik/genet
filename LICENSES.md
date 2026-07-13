# License layout

Cambium currently contains code under two inherited licenses:

- `crates/meristem` retains Linebender's Apache-2.0 license and source headers.
  Its license text is at `crates/meristem/LICENSE`.
- `crates/cambium` retains the MPL-2.0 headers from its Serval source.
- `crates/sprigging` remains MPL-2.0 during extraction. Although it was written
  as engine-neutral code, moving it does not itself authorize relicensing.

The root `LICENSE` contains the Mozilla Public License 2.0 text. Any future
Sprigging relicensing is a separate explicit decision made before publication.
