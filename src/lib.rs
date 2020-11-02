pub mod nodeclient {
    use std::str::FromStr;
    use std::string::ParseError;

    use log::info;
    use structopt::StructOpt;

    mod protocols;
    mod validate;
    mod leaderlog;

    #[derive(Debug)]
    pub enum LedgerSet {
        Mark,
        Set,
        Go,
    }

    impl FromStr for LedgerSet {
        type Err = ParseError;
        fn from_str(ledger_set: &str) -> Result<Self, Self::Err> {
            match ledger_set {
                "next" => Ok(LedgerSet::Mark),
                "current" => Ok(LedgerSet::Set),
                "prev" => Ok(LedgerSet::Go),
                _ => Ok(LedgerSet::Set)
            }
        }
    }

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
        Leaderlog {
            #[structopt(parse(from_os_str), short, long, default_value = "./cncli.db", help = "sqlite database file")]
            db: std::path::PathBuf,
            #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
            byron_genesis: std::path::PathBuf,
            #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
            shelley_genesis: std::path::PathBuf,
            #[structopt(parse(from_os_str), long, help = "ledger state json file")]
            ledger_state: std::path::PathBuf,
            #[structopt(long, default_value = "current", help = "Which ledger data to use. prev - previous epoch, current - current epoch, next - future epoch")]
            ledger_set: LedgerSet,
            #[structopt(long, help = "lower-case hex pool id")]
            pool_id: String,
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
            Command::Leaderlog { ref db, ref byron_genesis, ref shelley_genesis, ref ledger_state, ref ledger_set, ref pool_id } => {
                leaderlog::calculate_leader_logs(db, byron_genesis, shelley_genesis, ledger_state, ledger_set, pool_id);
            }
        }
    }
}