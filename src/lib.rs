pub mod nodeclient {
    use structopt::StructOpt;

    mod protocols;

    #[derive(Debug, StructOpt)]
    pub enum Command {
        Ping {
            #[structopt(short, long, help = "cardano-node hostname to connect to")]
            host: String,
            #[structopt(short, long, default_value = "3001", help = "cardano-node port")]
            port: u16,
            #[structopt(long, default_value = "764824073", help = "network magic.")]
            network_magic: u32,
        },
        Validate {
            #[structopt(long, help = "block hash to validate")]
            hash: String,
            #[structopt(short, long, help = "absolute slot to validate")]
            slot: u64,
            #[structopt(short, long, help = "cardano-node hostname to connect to")]
            host: String,
            #[structopt(short, long, default_value = "3000", help = "cardano-node port")]
            port: u16,
        },
        Sync {
            #[structopt(parse(from_os_str), short, long, help = "sqlite database file")]
            db: std::path::PathBuf,
            #[structopt(short, long, help = "cardano-node hostname to connect to")]
            host: String,
            #[structopt(short, long, default_value = "3000", help = "cardano-node port")]
            port: u16,
        },
    }

    pub fn start(cmd: Command) {
        match cmd {
            Command::Ping { ref host, ref port, ref network_magic } => {
                protocols::mux_protocol::ping(host, *port, *network_magic);
            }
            Command::Validate { ref hash, ref slot, ref host, ref port } => {
                println!("VALIDATE hash: {:?}, slot: {:?}, host: {:?}, port: {:?}", hash, slot, host, port);
            }
            Command::Sync { ref db, ref host, ref port } => {
                println!("Starting NodeClient...");
                println!("SYNC db: {:?}, host: {:?}, port: {:?}", db, host, port);
            }
        }
    }
}