//! fluorite.moe read-only HTTP API. Configured entirely via env (see `backend::api::Config`);
//! log filter via `RUST_LOG` (default `info`).

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // The configuration lives in `backend/.env`; the variables of the environment
    // win over the file.
    backend::envfile::load();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .compact()
        .init();

    let config = match backend::api::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fluorite-api: {e}");
            std::process::exit(2);
        }
    };
    if !config.output.is_dir() {
        eprintln!(
            "fluorite-api: output directory {} does not exist (set FLUORITE_OUTPUT)",
            config.output.display()
        );
        std::process::exit(2);
    }
    if let Err(e) = backend::api::run(config).await {
        eprintln!("fluorite-api: {e}");
        std::process::exit(1);
    }
}
