/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! knot-render — render a knot file out as any export format (plan K5).
//!
//! The export half of the knot evaluation + export plan: parse a knot
//! (CommonMark or djot body), run the polyglot fence expansion, and write
//! the document back out as gemtext, a gophermap, markdown, plain text, or
//! a knot again. Pipe the gemtext into your gemini server's content dir and
//! you are serving knots.
//!
//! ```text
//! knot-render <file.knot> --as gemtext|gophermap|markdown|text|knot
//!             [--djot] [--resolve] [--host <host>] [--port <port>]
//! ```
//!
//! `--djot` parses the body as djot (the experimental djot knot lane);
//! `--host`/`--port` fill the server columns a gophermap requires
//! (default `localhost:70`). `--resolve` runs the K1 transclusion pass
//! over `include file://<path>` fences (paths relative to the knot),
//! splicing the included documents in — the file-lane rehearsal of the
//! policy/fetch/render loop the network fetchers plug into later.

use inker::{
    Engine, EngineInput, Fetched, GophermapContext, TransclusionPolicy, resolve_transclusions,
};
use nematic::{DjotKnotEngine, GemtextEngine, KnotEngine, MarkdownEngine, TextEngine};
use std::path::Path;

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: knot-render <file.knot> --as gemtext|gophermap|markdown|text|knot \
                 [--djot] [--host <host>] [--port <port>]";

    let mut file = None;
    let mut format = "gemtext".to_string();
    let mut djot = false;
    let mut resolve = false;
    let mut host = "localhost".to_string();
    let mut port: u16 = 70;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--as" => {
                i += 1;
                format = args.get(i).cloned().ok_or("--as needs a format")?;
            }
            "--djot" => djot = true,
            "--resolve" => resolve = true,
            "--host" => {
                i += 1;
                host = args.get(i).cloned().ok_or("--host needs a value")?;
            }
            "--port" => {
                i += 1;
                port = args
                    .get(i)
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port needs a number")?;
            }
            other if file.is_none() => file = Some(other.to_string()),
            other => return Err(format!("unknown argument: {other}\n{usage}")),
        }
        i += 1;
    }
    let file = file.ok_or(usage)?;
    let body = std::fs::read_to_string(&file).map_err(|e| format!("read {file}: {e}"))?;

    let mut document = render_knot(&file, &body, djot)?;

    if resolve {
        let base = Path::new(&file)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut fetch = |url: &str| -> Result<Fetched, String> {
            let rel = url
                .strip_prefix("file://")
                .ok_or_else(|| format!("only file:// resolves in the bin (got {url})"))?;
            let path = base.join(rel);
            let body = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            Ok(Fetched {
                content_type: content_type_by_extension(rel),
                body,
            })
        };
        let mut render = |input: &EngineInput| -> Result<inker::EngineDocument, String> {
            let engine: Box<dyn Engine> = match input.content_type.as_deref() {
                Some("text/gemini") => Box::new(GemtextEngine::new()),
                Some("text/markdown") => Box::new(MarkdownEngine::new()),
                Some("text/x-knot") => Box::new(KnotEngine::new()),
                _ => Box::new(TextEngine::new()),
            };
            engine.render(input).map_err(|e| format!("{e:?}"))
        };
        let policy = TransclusionPolicy::for_own_notes(vec!["file".to_string()], 2);
        let outcome = resolve_transclusions(&mut document, &mut fetch, &mut render, &policy);
        for (url, reason) in &outcome.denied {
            eprintln!("denied  {url}: {reason}");
        }
        for (url, error) in &outcome.failed {
            eprintln!("failed  {url}: {error}");
        }
        eprintln!("resolved {} transclusion(s)", outcome.resolved);
    }

    let out = match format.as_str() {
        "gemtext" => document.to_gemini(),
        "gophermap" => document.to_gophermap(&GophermapContext { host, port }),
        "markdown" => document.to_markdown(),
        "text" => document.to_text(),
        "knot" => document.to_knot(),
        other => return Err(format!("unknown format: {other}\n{usage}")),
    };
    print!("{out}");
    Ok(())
}

fn content_type_by_extension(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?;
    let ct = match ext {
        "gmi" | "gemini" => "text/gemini",
        "md" | "markdown" => "text/markdown",
        "knot" => "text/x-knot",
        "txt" => "text/plain",
        _ => return None,
    };
    Some(ct.to_string())
}

fn render_knot(address: &str, body: &str, djot: bool) -> Result<inker::EngineDocument, String> {
    let mut input = EngineInput::new(address, body);
    input.content_type = Some("text/x-knot".to_string());
    let result = if djot {
        DjotKnotEngine::new().render(&input)
    } else {
        KnotEngine::new().render(&input)
    };
    result.map_err(|e| format!("render: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The K5 round-trip property: a gemtext fence in a knot expands into
    /// real blocks at parse time, so exporting back to gemtext restores the
    /// content as *live* gemtext lines — not a fenced sample.
    #[test]
    fn a_gemtext_fence_round_trips_to_live_gemtext() {
        let knot = "\
# notes

Some prose.

```gemtext
## from the capsule
=> gemini://example.org/log.gmi the log
```
";
        let document = render_knot("test.knot", knot, false).expect("render");
        let gemtext = document.to_gemini();
        assert!(
            gemtext.contains("## from the capsule"),
            "fence heading is a live gemtext heading: {gemtext}"
        );
        assert!(
            gemtext.contains("=> gemini://example.org/log.gmi the log"),
            "fence link is a live gemtext link line: {gemtext}"
        );
        assert!(
            !gemtext.contains("```gemtext"),
            "the fence does not export as a fenced sample: {gemtext}"
        );
    }

    #[test]
    fn a_djot_knot_exports_to_gemtext_and_gophermap() {
        let knot = "\
# field notes

See [the docs](https://x.test/docs) for more.
";
        let document = render_knot("test.knot", knot, true).expect("render");

        let gemtext = document.to_gemini();
        assert!(gemtext.contains("# field notes"));
        assert!(gemtext.contains("=> https://x.test/docs the docs"));

        let map = document.to_gophermap(&GophermapContext {
            host: "gopher.example".into(),
            port: 70,
        });
        assert!(map.contains("hthe docs\tURL:https://x.test/docs\tgopher.example\t70\r\n"));
        assert!(map.ends_with(".\r\n"));
    }

    #[test]
    fn plain_text_export_flattens_a_knot() {
        let knot = "# title\n\nProse with [link](https://x.test/).\n";
        let document = render_knot("test.knot", knot, false).expect("render");
        let text = document.to_text();
        assert!(text.contains("title"));
        assert!(text.contains("Prose with link <https://x.test/>."));
    }
}
