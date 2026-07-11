# serval-documents

Serval's retained document sessions: the static, scripted, and smolweb
content lanes as inker **session engines** (the third engine kind — spawn a
session, take paint frames, scroll, click, settle).

> **Home:** [`mark-ik/serval`](https://github.com/mark-ik/serval), at
> `components/serval-documents`. Born 2026-07-10 in the session-engines
> plan: these types began as pelt's convenience lanes and were promoted to
> an engine-grade component; pelt is now one consumer among hosts
> (merecat's mere, meerkat).

- `LoadedDocument` / `StaticSessionEngine` (`serval.web`): fetched HTML laid
  out by serval's cascade, no scripts.
- `ScriptedDocument` sessions / `ScriptedSessionEngine<E, _>` (`scripted`
  feature): a live page whose JS runs on Boa (or Nova on the nova rung),
  with the tick + quiescence seam (`pump` / `settled`).
- `SmolwebDocument` / `SmolwebSessionEngine` (`smolweb` feature): capsules
  rendered natively through `nematic::views` — errand parse, per-format
  views, serval layout.

Construction seams (fetchers, cookie jars, themes) live on the engine at
registration; the spawn request stays plain data. The session wrappers are
public for hosts with richer seams. Unpublished: this crate rides serval's
in-tree components; consume it as a git dependency.
