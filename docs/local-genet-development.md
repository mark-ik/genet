# Local Genet development

Cambium's committed manifests resolve Genet seam crates from crates.io. This
keeps a standalone checkout buildable and makes package verification exercise
the same dependency boundary consumers receive.

To test unpublished Genet seam changes, add source-specific patches to a local,
uncommitted `.cargo/config.toml`:

```toml
[patch.crates-io]
genet-scripted-dom = { path = "../../genet/components/genet-scripted-dom" }
layout-dom-api = { path = "../../genet/components/shared/layout-dom" }
errand = { path = "../../genet/components/errand" }
```

Paths are relative to `.cargo/config.toml`. Keep this override local. A change
that requires it is ready to publish only after the matching Genet seam release
is available from the registry.
