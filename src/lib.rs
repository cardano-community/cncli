use std::io::stdout;
use std::path::PathBuf;
use std::str::FromStr;
use std::string::ParseError;

use structopt::StructOpt;

use crate::nodeclient::dumpblock;
use crate::nodeclient::sync::pooltool;
use crate::nodeclient::{leaderlog, ping, sign, snapshot, sync, validate};

pub(crate) mod nodeclient;

pub static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputError {
    #[error("JSON output error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("stdout error: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) fn write_json<W: std::io::Write>(out: &mut W, value: &impl serde::Serialize) -> Result<(), OutputError> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

pub(crate) fn write_line<W: std::io::Write>(out: &mut W, value: &str) -> Result<(), OutputError> {
    writeln!(out, "{value}")?;
    out.flush()?;
    Ok(())
}

// The binary is a separate crate; keep the marker private and expose only its identity check.
pub fn is_output_error(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error.is::<OutputError>() {
            return true;
        }
        match error.source() {
            Some(source) => error = source,
            None => return false,
        }
    }
}

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
    DumpBlock {
        #[structopt(long, help = "Slot number of the intersect point")]
        intersect_slot: u64,
        #[structopt(long, help = "Block hash of the intersect point (hex)")]
        intersect_hash: String,
        #[structopt(short, long, help = "cardano-node hostname to connect to")]
        host: String,
        #[structopt(short, long, default_value = "3001", help = "cardano-node port")]
        port: u16,
        #[structopt(long, default_value = "764824073", help = "network magic.")]
        network_magic: u64,
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
        #[structopt(long, help = "Use the redb database instead of sqlite")]
        use_redb: bool,
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
            help = "Consensus algorithm - Alonzo and earlier uses tpraos, Babbage uses praos, Conway uses cpraos"
        )]
        consensus: String,
        #[structopt(
            long,
            env = "SHELLEY_TRANS_EPOCH",
            help = "Epoch number where we transition from Byron to Shelley. Omitted means guess based on genesis files"
        )]
        shelley_transition_epoch: Option<u64>,
        #[structopt(
            long,
            help = "Provide a nonce value in lower-case hex instead of calculating from the db"
        )]
        nonce: Option<String>,
        #[structopt(
            long,
            help = "Provide a specific epoch number to calculate for and ignore --ledger-set option"
        )]
        epoch: Option<u64>,
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
            help = "Epoch number where we transition from Byron to Shelley. Omitted means guess based on genesis files"
        )]
        shelley_transition_epoch: Option<u64>,
        #[structopt(long, env = "OVERRIDE_TIME", hide_env_values = true, hidden = true)]
        override_time: Option<String>,
    },
    Status {
        #[structopt(
            parse(from_os_str),
            short,
            long,
            default_value = "./cncli.db",
            help = "sqlite or redb database file"
        )]
        db: PathBuf,
        #[structopt(parse(from_os_str), long, help = "byron genesis json file")]
        byron_genesis: PathBuf,
        #[structopt(parse(from_os_str), long, help = "shelley genesis json file")]
        shelley_genesis: PathBuf,
        #[structopt(
            long,
            env = "SHELLEY_TRANS_EPOCH",
            help = "Epoch number where we transition from Byron to Shelley. Omitted means guess based on genesis files"
        )]
        shelley_transition_epoch: Option<u64>,
    },
    Nonce {
        #[structopt(
            parse(from_os_str),
            short,
            long,
            default_value = "./cncli.db",
            help = "sqlite or redb database file"
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
            help = "Epoch number where we transition from Byron to Shelley. Omitted means guess based on genesis files"
        )]
        shelley_transition_epoch: Option<u64>,
        #[structopt(
            short,
            long,
            default_value = "praos",
            help = "Consensus algorithm - Alonzo and earlier uses tpraos, Babbage uses praos, Conway uses cpraos"
        )]
        consensus: String,
        #[structopt(
            long,
            help = "Provide a specific epoch number to calculate for and ignore --ledger-set option"
        )]
        epoch: Option<u64>,
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
    Snapshot {
        #[structopt(parse(from_os_str), long, help = "cardano-node socket path")]
        socket_path: PathBuf,
        #[structopt(long, default_value = "764824073", help = "network magic.")]
        network_magic: u64,
        #[structopt(long, default_value = "mark", help = "Snapshot name to retrieve (mark, set, go)")]
        name: String,
        #[structopt(
            long,
            default_value = "1",
            help = "The network identifier, (1 for mainnet, 0 for testnet)"
        )]
        network_id: u8,
        #[structopt(
            long,
            default_value = "stake",
            help = "The prefix for stake addresses, (stake for mainnet, stake_test for testnet)"
        )]
        stake_prefix: String,
        #[structopt(long, default_value = "mark.csv", help = "The name of the output file (CSV format)")]
        output_file: String,
    },
    PoolStake {
        #[structopt(parse(from_os_str), long, help = "cardano-node socket path")]
        socket_path: PathBuf,
        #[structopt(long, default_value = "764824073", help = "network magic.")]
        network_magic: u64,
        #[structopt(
            long,
            default_value = "mark",
            help = "PoolStake snapshot name to retrieve (mark, set, go)"
        )]
        name: String,
        #[structopt(
            long,
            default_value = "1",
            help = "The network identifier, (1 for mainnet, 0 for testnet)"
        )]
        network_id: u8,
        #[structopt(long, default_value = "mark.csv", help = "The name of the output file (CSV format)")]
        output_file: String,
    },
}

pub async fn start(cmd: &Command) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match cmd {
        Command::Ping {
            host,
            port,
            network_magic,
            timeout_seconds,
        } => {
            ping::ping(&mut stdout(), host, *port, *network_magic, *timeout_seconds).await?;
        }
        Command::Validate { db, hash } => validate::validate_block(db, hash)?,
        Command::DumpBlock {
            intersect_slot,
            intersect_hash,
            host,
            port,
            network_magic,
        } => {
            dumpblock::run(nodeclient::dumpblock::Args {
                intersect_slot: *intersect_slot,
                intersect_hash: intersect_hash.clone(),
                host: host.clone(),
                port: *port,
                network_magic: *network_magic,
            })
            .await?;
        }
        Command::Sync {
            db,
            host,
            port,
            network_magic,
            no_service,
            shelley_genesis_hash,
            use_redb,
        } => {
            sync::sync(
                db,
                host,
                *port,
                *network_magic,
                shelley_genesis_hash,
                *no_service,
                *use_redb,
            )
            .await?;
        }
        Command::Leaderlog {
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
            consensus,
            shelley_transition_epoch,
            nonce,
            epoch,
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
                nonce,
                epoch,
            )?;
        }
        Command::Nonce {
            db,
            byron_genesis,
            shelley_genesis,
            extra_entropy,
            ledger_set,
            shelley_transition_epoch,
            consensus,
            epoch,
        } => {
            leaderlog::calculate_leader_logs(
                db,
                byron_genesis,
                shelley_genesis,
                &0,
                &0,
                &0.0,
                extra_entropy,
                ledger_set,
                "nonce",
                &PathBuf::new(),
                "America/Los_Angeles",
                true,
                consensus,
                shelley_transition_epoch,
                &None,
                epoch,
            )?;
        }
        Command::Sendtip { config, cardano_node } => {
            let pooltool_config = pooltool::get_pooltool_config(config)?;
            if !cardano_node.is_file() {
                return Err("cardano-node not found!".into());
            }
            let client = reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(15))
                .build()?;
            let mut tasks = tokio::task::JoinSet::new();
            for pool in pooltool_config.pools {
                let api_key = pooltool_config.api_key.clone();
                let cardano_node_path = cardano_node.clone();
                let client = client.clone();
                tasks.spawn(async move {
                    sync::sendtip(
                        pool.name,
                        pool.pool_id,
                        pool.host,
                        pool.port,
                        api_key,
                        &cardano_node_path,
                        client,
                    )
                    .await
                });
            }
            while let Some(result) = tasks.join_next().await {
                result??;
            }
        }
        Command::Sendslots {
            config,
            db,
            byron_genesis,
            shelley_genesis,
            shelley_transition_epoch,
            override_time,
        } => {
            let config = pooltool::get_pooltool_config(config)?;
            let db = db.clone();
            let byron = byron_genesis.clone();
            let shelley = shelley_genesis.clone();
            let transition = *shelley_transition_epoch;
            let override_time = override_time.clone();
            tokio::task::spawn_blocking(move || {
                leaderlog::send_slots(&db, &byron, &shelley, config, &transition, &override_time)
            })
            .await??;
        }
        Command::Status {
            db,
            byron_genesis,
            shelley_genesis,
            shelley_transition_epoch,
        } => {
            leaderlog::status(db, byron_genesis, shelley_genesis, shelley_transition_epoch)?;
        }
        Command::Challenge { domain } => {
            sign::create_challenge(domain)?;
        }
        Command::Sign {
            pool_vrf_skey,
            domain,
            nonce,
        } => {
            sign::sign_challenge(pool_vrf_skey, domain, nonce)?;
        }
        Command::Verify {
            pool_vrf_vkey,
            pool_vrf_vkey_hash,
            domain,
            nonce,
            signature,
        } => {
            sign::verify_challenge(pool_vrf_vkey, pool_vrf_vkey_hash, domain, nonce, signature)?;
        }
        Command::Snapshot {
            socket_path,
            network_magic,
            name,
            network_id,
            stake_prefix,
            output_file,
        } => {
            snapshot::dump(
                socket_path,
                *network_magic,
                name,
                *network_id,
                stake_prefix,
                output_file,
            )
            .await?;
        }
        Command::PoolStake {
            socket_path,
            network_magic,
            name,
            network_id,
            output_file,
        } => {
            snapshot::pool_stake_dump(socket_path, *network_magic, name, *network_id, output_file).await?;
        }
    }
    Ok(())
}
