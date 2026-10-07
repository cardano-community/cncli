extern crate chrono_tz;

use std::env::{set_var, var};
use std::io::Write;
use std::process;

use structopt::StructOpt;

use cncli::Command;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));

    pub fn version() -> &'static str {
        Box::leak(Box::new(format!(
            "v{} <{}> ({})",
            PKG_VERSION,
            GIT_COMMIT_HASH_SHORT.unwrap_or("unknown"),
            TARGET
        )))
    }
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "cncli", about = "A community-built cardano-node CLI", version = built_info::version()
)]
struct Cli {
    #[structopt(subcommand)]
    cmd: Command,
}

#[tokio::main]
async fn main() {
    match var("RUST_LOG") {
        Ok(_) => {}
        Err(_) => {
            // set a default logging level of info if unset.
            set_var("RUST_LOG", "info");
        }
    }
    let tracing_filter = match var("RUST_LOG") {
        Ok(level) => match level.to_lowercase().as_str() {
            "error" => tracing::Level::ERROR,
            "warn" => tracing::Level::WARN,
            "info" => tracing::Level::INFO,
            "debug" => tracing::Level::DEBUG,
            "trace" => tracing::Level::TRACE,
            _ => tracing::Level::INFO,
        },
        Err(_) => tracing::Level::INFO,
    };

    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing_filter)
            .with_writer(std::io::stderr)
            .finish(),
    )
    .unwrap();

    let args = Cli::from_args();
    if let Err(error) = cncli::start(&args.cmd).await {
        if cncli::is_output_error(error.as_ref()) {
            let _ = writeln!(std::io::stderr().lock(), "Unable to write command result: {error}");
        } else {
            let mut body = serde_json::json!({"status": "error", "errorMessage": error.to_string()});
            if let Command::Ping { host, port, .. } = &args.cmd {
                body["host"] = serde_json::json!(host);
                body["port"] = serde_json::json!(port);
            }
            let mut out = std::io::stdout().lock();
            let result = serde_json::to_writer_pretty(&mut out, &body)
                .map_err(std::io::Error::other)
                .and_then(|_| out.write_all(b"\n"))
                .and_then(|_| out.flush());
            if let Err(write_error) = result {
                let _ = writeln!(std::io::stderr().lock(), "Unable to write command error: {write_error}");
            }
        }
        // Do not wait for uncancellable resolver work during Tokio runtime destruction.
        process::exit(1);
    }
}
