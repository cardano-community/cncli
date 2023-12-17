extern crate chrono_tz;

use std::env::{set_var, var};
use std::{panic, process};

use structopt::StructOpt;

use cncli::nodeclient::{self, Command};

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
#[structopt(name = "cncli", about = "A community-built cardano-node CLI", version = built_info::version())]
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
    pretty_env_logger::init_timed();

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
            .finish(),
    )
    .unwrap();

    // take_hook() returns the default hook in case when a custom one is not set
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);
        process::exit(1);
    }));

    let args = Cli::from_args();
    nodeclient::start(args.cmd).await;
}

#[cfg(test)]
mod test;
