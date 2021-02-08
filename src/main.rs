extern crate chrono_tz;
extern crate libc;

use std::env::{set_var, var};
use std::{panic, process};

use structopt::StructOpt;

use cncli::nodeclient::{self, Command};

#[derive(Debug, StructOpt)]
#[structopt(name = "cncli", about = "A community-built cardano-node CLI")]
struct Cli {
    #[structopt(subcommand)]
    cmd: Command,
}

fn main() {
    match var("RUST_LOG") {
        Ok(_) => {}
        Err(_) => {
            // set a default logging level of info if unset.
            set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init_timed();

    // take_hook() returns the default hook in case when a custom one is not set
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);
        process::exit(1);
    }));

    let args = Cli::from_args();
    nodeclient::start(args.cmd)
}

#[cfg(test)]
mod test;
