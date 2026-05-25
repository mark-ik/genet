# serval

serval is a web engine derived from [Servo](https://servo.org), adapted for
embedding in the Mere ecosystem. It keeps Servo's Rust foundation and diverges
where the ecosystem's needs differ. It is a work in progress.

## How it differs from Servo

- **Scripting.** SpiderMonkey is removed. JavaScript is built around the Nova
  engine on native, with Boa as the conformance oracle. See
  `docs/2026-05-20_serval_script_engine_plan.md` and
  `docs/2026-05-25_js_execution_strategy.md`.
- **Layout.** `serval-layout` is a profile-neutral engine: a box tree laid out
  by Taffy through `stylo_taffy`, styled by the Stylo cascade, with text shaped
  by parley. See `docs/2026-05-25_box_tree_trait_impl_plan.md`.
- **Rendering.** serval emits a paint list that netrender (a vello-based
  renderer) consumes.
- **Profiles.** Capabilities are tiered (static-html, interactive-html,
  scripted, fullweb) so each build pulls only what its profile needs. See
  `docs/2026-05-12_serval_profile_ladder_plan.md`.
- **Entrypoint.** `ports/pelt` is a script-free validation viewer and the
  default workspace member.

## Build

serval builds with cargo on the pinned toolchain (rust 1.95.0, set by
`rust-toolchain.toml`; `rustup` applies it automatically).

```shell
cargo build
cargo run -p pelt -- --engine viewer [URL]
```

Plain `cargo build` / `cargo run` target `ports/pelt`, the default member. The
default build runs on a stock Windows toolchain. Re-including `tests/unit/script`
in the workspace brings back the heavier mozjs build environment; see
`docs/2026-05-16_workspace_audit_snapshot.md`.

## Architecture

Design docs live in `docs/`, named by date. The most recent workspace-audit
snapshot there describes current state.

## License

serval is a derivative of Servo and is licensed under MPL-2.0. Upstream Servo:
[servo.org](https://servo.org), [book.servo.org](https://book.servo.org).
