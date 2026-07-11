//! The `nex` command-line tool (the `cli` feature): fetch and serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use nex_protocol::{FetchOptions, FileHandler, ServerConfig, fetch, serve};

const USAGE: &str = "nex — the Nex protocol (spec: nex://nightfall.city/nex/info/specification.txt)

USAGE:
  nex fetch <nex://url>
      Fetch a URL and print the response.

  nex serve --root DIR [--listen ADDR:PORT]
      Serve a directory (index.nex per directory, else a generated listing).
      Default listen address is 0.0.0.0:1900.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some((command, rest)) = args.split_first() else {
        return Err(USAGE.to_string());
    };

    let mut positional = Vec::new();
    let mut root: Option<PathBuf> = None;
    let mut listen: Option<String> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(iter.next().ok_or("--root needs a value")?)),
            "--listen" => listen = Some(iter.next().ok_or("--listen needs a value")?.clone()),
            other if other.starts_with("--") => return Err(format!("Unknown flag {other}")),
            other => positional.push(other.to_string()),
        }
    }

    match command.as_str() {
        "fetch" => {
            let [url] = positional.as_slice() else {
                return Err("usage: nex fetch <nex://url>".into());
            };
            let body = block_on(fetch(url, &FetchOptions::default()))?;
            use std::io::Write;
            std::io::stdout()
                .write_all(&body)
                .map_err(|error| error.to_string())
        }
        "serve" => {
            let root = root.ok_or("nex serve: --root DIR is required")?;
            let listen: SocketAddr = listen
                .as_deref()
                .unwrap_or("0.0.0.0:1900")
                .parse()
                .map_err(|_| "nex serve: --listen needs ADDR:PORT".to_string())?;
            block_on(async move {
                let listener = tokio::net::TcpListener::bind(listen)
                    .await
                    .map_err(|error| format!("bind {listen}: {error}"))?;
                eprintln!("nex: serving {} on {listen} (ctrl-c to stop)", root.display());
                serve(
                    listener,
                    FileHandler::new(root),
                    ServerConfig::default(),
                    async {
                        let _ = tokio::signal::ctrl_c().await;
                    },
                )
                .await
                .map_err(|error| error.to_string())
            })
        }
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("Unknown command '{other}'.\n\n{USAGE}")),
    }
}

fn block_on<T, E: ToString>(
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?
        .block_on(future)
        .map_err(|error| error.to_string())
}
