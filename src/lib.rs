pub mod nodeclient {
    use log::info;
    use structopt::StructOpt;

    mod protocols;
    mod validate;

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
            #[structopt(long, help = "full or partial block hash to validate")]
            hash: String,
            #[structopt(parse(from_os_str), short, long, default_value = "./cncli.db", help = "sqlite database file")]
            db: std::path::PathBuf,
        },
        Sync {
            #[structopt(parse(from_os_str), short, long, default_value = "./cncli.db", help = "sqlite database file")]
            db: std::path::PathBuf,
            #[structopt(short, long, help = "cardano-node hostname to connect to")]
            host: String,
            #[structopt(short, long, default_value = "3001", help = "cardano-node port")]
            port: u16,
            #[structopt(long, default_value = "764824073", help = "network magic.")]
            network_magic: u32,
        },
    }

    pub fn start(cmd: Command) {
        match cmd {
            Command::Ping { ref host, ref port, ref network_magic } => {
                protocols::mux_protocol::ping(host, *port, *network_magic);
            }
            Command::Validate { ref db, ref hash } => {
                validate::validate_block(db, hash);
            }
            Command::Sync { ref db, ref host, ref port, ref network_magic } => {
                info!("Starting NodeClient...");
                protocols::mux_protocol::sync(db, host, *port, *network_magic);
            }
        }
    }
}