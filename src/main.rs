use std::env::{set_var, var};

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

    let args = Cli::from_args();
    nodeclient::start(args.cmd)
}
