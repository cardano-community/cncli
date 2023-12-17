pub mod nodeclient {
    use std::fs::File;
    use std::io::{stdout, BufReader};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::string::ParseError;
    use std::thread;
    use std::thread::JoinHandle;

    use serde::Deserialize;
    use structopt::StructOpt;

    use crate::nodeclient::leaderlog::handle_error;

    pub mod leaderlog;
    pub mod math;
    pub mod ping;
    pub mod pooltool;
    pub mod signing;
    pub mod sqlite;
    pub mod sync;
    mod validate;

    pub static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

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
                _ => Ok(LedgerSet::Set),
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
            network_magic: u64,
            #[structopt(short, long, default_value = "2", help = "connect timeout in seconds")]
            timeout_seconds: u64,
        },
        Validate {
            #[structopt(long, help = "full or partial block hash to validate")]
            hash: String,
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
        },
        Sync {
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
            #[structopt(short, long, help = "cardano-node hostname to connect to")]
            host: String,
            #[structopt(short, long, default_value = "3001", help = "cardano-node port")]
            port: u16,
            #[structopt(long, default_value = "764824073", help = "network magic.")]
            network_magic: u64,
            #[structopt(long, help = "Exit at 100% sync'd.")]
            no_service: bool,
            #[structopt(
                short,
                long,
                default_value = "1a3be38bcbb7911969283716ad7aa550250226b76a61fc51cc9a9a35d9276d81",
                help = "shelley genesis hash value"
            )]
            shelley_genesis_hash: String,
        },
        Leaderlog {
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
            #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
            byron_genesis: PathBuf,
            #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
            shelley_genesis: PathBuf,
            #[structopt(long, help = "pool active stake snapshot value in lovelace")]
            pool_stake: u64,
            #[structopt(long, help = "total active stake snapshot value in lovelace")]
            active_stake: u64,
            #[structopt(long = "d", default_value = "0", help = "decentralization parameter")]
            d: f64,
            #[structopt(long, help = "hex string of the extra entropy value")]
            extra_entropy: Option<String>,
            #[structopt(
                long,
                default_value = "current",
                help = "Which ledger data to use. prev - previous epoch, current - current epoch, next - future epoch"
            )]
            ledger_set: LedgerSet,
            #[structopt(long, help = "lower-case hex pool id")]
            pool_id: String,
            #[structopt(parse(from_os_str), long, help = "pool's vrf.skey file")]
            pool_vrf_skey: PathBuf,
            #[structopt(
                long = "tz",
                default_value = "America/Los_Angeles",
                help = "TimeZone string from the IANA database - https://en.wikipedia.org/wiki/List_of_tz_database_time_zones"
            )]
            timezone: String,
            #[structopt(
                short,
                long,
                default_value = "praos",
                help = "Consensus algorithm - Alonzo and earlier uses tpraos, Babbage and later uses praos"
            )]
            consensus: String,
            #[structopt(
                long,
                env = "SHELLEY_TRANS_EPOCH",
                default_value = "-1",
                help = "Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files"
            )]
            shelley_transition_epoch: i64,
        },
        Sendtip {
            #[structopt(
                parse(from_os_str),
                long,
                default_value = "./pooltool.json",
                help = "pooltool config file for sending tips"
            )]
            config: PathBuf,
            #[structopt(
                parse(from_os_str),
                long,
                help = "path to cardano-node executable for gathering version info"
            )]
            cardano_node: PathBuf,
        },
        Sendslots {
            #[structopt(
                parse(from_os_str),
                long,
                default_value = "./pooltool.json",
                help = "pooltool config file for sending slots"
            )]
            config: PathBuf,
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
            #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
            byron_genesis: PathBuf,
            #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
            shelley_genesis: PathBuf,
            #[structopt(
                long,
                env = "SHELLEY_TRANS_EPOCH",
                default_value = "-1",
                help = "Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files"
            )]
            shelley_transition_epoch: i64,
            #[structopt(long, env = "OVERRIDE_TIME", hide_env_values = true, hidden = true)]
            override_time: Option<String>,
        },
        Status {
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
            #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
            byron_genesis: PathBuf,
            #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
            shelley_genesis: PathBuf,
            #[structopt(
                long,
                env = "SHELLEY_TRANS_EPOCH",
                default_value = "-1",
                help = "Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files"
            )]
            shelley_transition_epoch: i64,
        },
        Nonce {
            #[structopt(
                parse(from_os_str),
                short,
                long,
                default_value = "./cncli.db",
                help = "sqlite database file"
            )]
            db: PathBuf,
            #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
            byron_genesis: PathBuf,
            #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
            shelley_genesis: PathBuf,
            #[structopt(long, help = "hex string of the extra entropy value")]
            extra_entropy: Option<String>,
            #[structopt(
                long,
                default_value = "current",
                help = "Which ledger data to use. prev - previous epoch, current - current epoch, next - future epoch"
            )]
            ledger_set: LedgerSet,
            #[structopt(
                long,
                env = "SHELLEY_TRANS_EPOCH",
                default_value = "-1",
                help = "Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files"
            )]
            shelley_transition_epoch: i64,
        },
        Challenge {
            #[structopt(long, help = "validating domain e.g. pooltool.io")]
            domain: String,
        },
        Sign {
            #[structopt(parse(from_os_str), long, help = "pool's vrf.skey file")]
            pool_vrf_skey: PathBuf,
            #[structopt(long, help = "validating domain e.g. pooltool.io")]
            domain: String,
            #[structopt(long, help = "nonce value in lower-case hex")]
            nonce: String,
        },
        Verify {
            #[structopt(parse(from_os_str), long, help = "pool's vrf.vkey file")]
            pool_vrf_vkey: PathBuf,
            #[structopt(
                long,
                help = "pool's vrf hash in hex retrieved from 'cardano-cli query pool-params...'"
            )]
            pool_vrf_vkey_hash: String,
            #[structopt(long, help = "validating domain e.g. pooltool.io")]
            domain: String,
            #[structopt(long, help = "nonce value in lower-case hex")]
            nonce: String,
            #[structopt(long, help = "signature to verify in hex")]
            signature: String,
        },
    }

    pub async fn start(cmd: Command) {
        match cmd {
            Command::Ping {
                ref host,
                ref port,
                ref network_magic,
                ref timeout_seconds,
            } => {
                ping::ping(&mut stdout(), host.as_str(), *port, *network_magic, *timeout_seconds).await;
            }
            Command::Validate { ref db, ref hash } => {
                validate::validate_block(db, hash.as_str());
            }
            Command::Sync {
                ref db,
                ref host,
                ref port,
                ref network_magic,
                ref no_service,
                ref shelley_genesis_hash,
            } => {
                sync::sync(
                    db,
                    host.as_str(),
                    *port,
                    *network_magic,
                    shelley_genesis_hash.as_str(),
                    *no_service,
                )
                .await;
            }
            Command::Leaderlog {
                ref db,
                ref byron_genesis,
                ref shelley_genesis,
                ref pool_stake,
                ref active_stake,
                ref d,
                ref extra_entropy,
                ref ledger_set,
                ref pool_id,
                ref pool_vrf_skey,
                ref timezone,
                ref consensus,
                ref shelley_transition_epoch,
            } => {
                leaderlog::calculate_leader_logs(
                    db,
                    byron_genesis,
                    shelley_genesis,
                    pool_stake,
                    active_stake,
                    d,
                    extra_entropy,
                    ledger_set,
                    pool_id,
                    pool_vrf_skey,
                    timezone,
                    false,
                    consensus,
                    shelley_transition_epoch,
                );
            }
            Command::Nonce {
                ref db,
                ref byron_genesis,
                ref shelley_genesis,
                ref extra_entropy,
                ref ledger_set,
                ref shelley_transition_epoch,
            } => leaderlog::calculate_leader_logs(
                db,
                byron_genesis,
                shelley_genesis,
                &0u64,
                &0u64,
                &0f64,
                extra_entropy,
                ledger_set,
                "nonce",
                &PathBuf::new(),
                "America/Los_Angeles",
                true,
                "praos",
                shelley_transition_epoch,
            ),
            Command::Sendtip {
                ref config,
                ref cardano_node,
            } => {
                if !config.exists() {
                    handle_error("config not found!");
                    return;
                }
                if !cardano_node.exists() {
                    handle_error("cardano-node not found!");
                    return;
                }

                let pooltool_config: PooltoolConfig = get_pooltool_config(config);
                let mut handles: Vec<JoinHandle<_>> = vec![];
                for pool in pooltool_config.pools.into_iter() {
                    let api_key = pooltool_config.api_key.clone();
                    let cardano_node_path = cardano_node.clone();
                    handles.push(thread::spawn(move || {
                        tokio::runtime::Runtime::new().unwrap().block_on(sync::sendtip(
                            pool.name,
                            pool.pool_id,
                            pool.host,
                            pool.port,
                            api_key,
                            &cardano_node_path,
                        ));
                    }));
                }

                for handle in handles {
                    handle.join().unwrap()
                }
            }
            Command::Sendslots {
                ref config,
                ref db,
                ref byron_genesis,
                ref shelley_genesis,
                ref shelley_transition_epoch,
                ref override_time,
            } => {
                if !config.exists() {
                    handle_error("config not found!");
                    return;
                }
                let pooltool_config: PooltoolConfig = get_pooltool_config(config);
                leaderlog::send_slots(
                    db,
                    byron_genesis,
                    shelley_genesis,
                    pooltool_config,
                    shelley_transition_epoch,
                    override_time,
                );
            }
            Command::Status {
                ref db,
                ref byron_genesis,
                ref shelley_genesis,
                ref shelley_transition_epoch,
            } => {
                leaderlog::status(db, byron_genesis, shelley_genesis, shelley_transition_epoch);
            }
            Command::Challenge { ref domain } => {
                signing::create_challenge(domain);
            }
            Command::Sign {
                ref pool_vrf_skey,
                ref domain,
                ref nonce,
            } => {
                if !pool_vrf_skey.exists() {
                    handle_error("vrf.skey not found!");
                    return;
                }
                signing::sign_challenge(pool_vrf_skey, domain, nonce);
            }
            Command::Verify {
                ref pool_vrf_vkey,
                ref pool_vrf_vkey_hash,
                ref domain,
                ref nonce,
                ref signature,
            } => {
                signing::verify_challenge(pool_vrf_vkey, pool_vrf_vkey_hash, domain, nonce, signature);
            }
        }
    }

    fn get_pooltool_config(config: &Path) -> PooltoolConfig {
        let buf = BufReader::new(File::open(config).unwrap());
        serde_json::from_reader(buf).unwrap()
    }

    #[derive(Debug, Deserialize)]
    pub struct PooltoolConfig {
        api_key: String,
        pools: Vec<Pool>,
    }

    #[derive(Debug, Deserialize)]
    struct Pool {
        name: String,
        pool_id: String,
        host: String,
        port: u16,
    }
}
