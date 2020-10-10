use structopt::StructOpt;
use cncli::nodeclient::{self, Command};

#[derive(Debug, StructOpt)]
#[structopt(name = "cncli", about = "A community-built cardano-node CLI")]
struct Cli {
    #[structopt(subcommand)]
    cmd: Command,
}

fn main() {
    let args = Cli::from_args();
    nodeclient::start(args.cmd)
}
