//! The `spartan` command-line tool (the `cli` feature): fetch, submit, serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use spartan_protocol::{
    FetchOptions, FileHandler, ServerConfig, Status, fetch, serve, submit,
};

const USAGE: &str = "spartan — the Spartan protocol 💪 (spec: github.com/michael-lazar/spartan)

USAGE:
  spartan fetch <spartan://url>
      Fetch a URL and print the body (query components upload per the spec).

  spartan submit <spartan://url> <data...>
      Upload data as the request's data block (the =: prompt flow).

  spartan serve --root DIR [--listen ADDR:PORT]
      Serve a directory of files (index.gmi per directory, gemtext-first).
      Default listen address is 0.0.0.0:300 (may need privileges; try
      --listen 0.0.0.0:3000 for testing).
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
                return Err("usage: spartan fetch <spartan://url>".into());
            };
            let response = block_on(fetch(url, &FetchOptions::default()))?;
            print_response(response)
        }
        "submit" => {
            let Some((url, data_parts)) = positional.split_first() else {
                return Err("usage: spartan submit <spartan://url> <data...>".into());
            };
            if data_parts.is_empty() {
                return Err("spartan submit: the data is empty".into());
            }
            let data = data_parts.join(" ");
            let response = block_on(submit(url, data.as_bytes(), &FetchOptions::default()))?;
            print_response(response)
        }
        "serve" => {
            let root = root.ok_or("spartan serve: --root DIR is required")?;
            let listen: SocketAddr = listen
                .as_deref()
                .unwrap_or("0.0.0.0:300")
                .parse()
                .map_err(|_| "spartan serve: --listen needs ADDR:PORT".to_string())?;
            block_on(async move {
                let listener = tokio::net::TcpListener::bind(listen)
                    .await
                    .map_err(|error| format!("bind {listen}: {error}"))?;
                eprintln!("spartan: serving {} on {listen} (ctrl-c to stop)", root.display());
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

fn print_response(response: spartan_protocol::Response) -> Result<(), String> {
    match response.status {
        Status::Success => {
            eprintln!("2 {}", response.meta);
            use std::io::Write;
            std::io::stdout()
                .write_all(&response.body)
                .map_err(|error| error.to_string())
        }
        status => {
            println!("{status} {}", response.meta);
            Err(format!("request answered: {status}"))
        }
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
