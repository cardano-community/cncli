use std::ops::Sub;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use log::{debug, error, info, trace, warn};
use rusqlite::{Connection, Error, NO_PARAMS};
use serde_cbor::{de, ser, Value};

use crate::nodeclient::protocols::{Agency, Protocol};
use crate::nodeclient::protocols::chainsync_protocol::msg_roll_backward::parse_msg_roll_backward;
use crate::nodeclient::protocols::chainsync_protocol::msg_roll_forward::parse_msg_roll_forward;

mod msg_roll_forward;
mod msg_roll_backward;

pub enum State {
    Idle,
    Intersect,
    CanAwait,
    MustReply,
    Done,
}

pub struct ChainSyncProtocol {
    last_log_time: Instant,
    db: Option<Connection>,
    pub(crate) state: State,
    pub(crate) result: Option<Result<String, String>>,
    pub(crate) is_intersect_found: bool,
}

impl Default for ChainSyncProtocol {
    fn default() -> Self {
        ChainSyncProtocol {
            last_log_time: Instant::now().sub(Duration::from_secs(6)),
            db: None,
            state: State::Idle,
            result: None,
            is_intersect_found: false,
        }
    }
}

impl ChainSyncProtocol {
    const DB_VERSION: i64 = 1;

    pub(crate) fn init_database(&mut self, db_path: &PathBuf) -> Result<(), Error> {
        let db = Connection::open(db_path)?;
        {
            db.execute("CREATE TABLE IF NOT EXISTS db_version (version INTEGER PRIMARY KEY)", NO_PARAMS)?;
            let mut stmt = db.prepare("SELECT version FROM db_version")?;
            let mut rows = stmt.query(NO_PARAMS)?;
            let version: i64 = match rows.next()? {
                None => { -1 }
                Some(row) => {
                    row.get(0)?
                }
            };

            // Upgrade their database to version 1
            if version < 1 {
                db.execute("CREATE TABLE IF NOT EXISTS chain (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            block_number INTEGER NOT NULL, \
            slot_number INTEGER NOT NULL, \
            hash TEXT NOT NULL, \
            prev_hash TEXT NOT NULL, \
            node_vkey TEXT NOT NULL, \
            node_vrf_vkey TEXT NOT NULL, \
            eta_vrf_0 TEXT NOT NULL, \
            eta_vrf_1 TEXT NOT NULL, \
            leader_vrf_0 TEXT NOT NULL, \
            leader_vrf_1 TEXT NOT NULL, \
            block_size INTEGER NOT NULL, \
            block_body_hash TEXT NOT NULL, \
            pool_opcert TEXT NOT NULL, \
            unknown_0 INTEGER NOT NULL, \
            unknown_1 INTEGER NOT NULL, \
            unknown_2 TEXT NOT NULL, \
            protocol_major_version INTEGER NOT NULL, \
            protocol_minor_version INTEGER NOT NULL \
            )", NO_PARAMS)?;
            }

            // Upgrade their database to version ...
            // if version < ... {}

            // Update the db version now that we've upgraded the user's database fully
            if version < 0 {
                db.execute("INSERT INTO db_version (version) VALUES (?1)", &[&ChainSyncProtocol::DB_VERSION])?;
            } else {
                db.execute("UPDATE db_version SET version=?1", &[&ChainSyncProtocol::DB_VERSION])?;
            }
        }
        self.db = Some(db);

        Ok(())
    }

    fn msg_find_intersect(&self, chain_blocks: Vec<(u64, Vec<u8>)>) -> Vec<u8> {

        // figure out how to fix this extra clone later
        let msg: Value = Value::Array(
            vec![
                Value::Integer(4), // message_id
                // Value::Array(points),
                Value::Array(chain_blocks.iter().map(|(slot, hash)| Value::Array(vec![Value::Integer(*slot as i128), Value::Bytes(hash.clone())])).collect())
            ]
        );

        ser::to_vec_packed(&msg).unwrap()
    }

    fn msg_request_next(&self) -> Vec<u8> {
        // we just send an array containing the message_id for this one.
        ser::to_vec_packed(&Value::Array(vec![Value::Integer(0)])).unwrap()
    }
}

impl Protocol for ChainSyncProtocol {
    fn protocol_id(&self) -> u16 {
        return 0x0002u16;
    }

    fn get_agency(&self) -> Agency {
        return match self.state {
            State::Idle => { Agency::Client }
            State::Intersect => { Agency::Server }
            State::CanAwait => { Agency::Server }
            State::MustReply => { Agency::Server }
            State::Done => { Agency::None }
        };
    }

    fn send_data(&mut self) -> Option<Vec<u8>> {
        return match self.state {
            State::Idle => {
                trace!("ChainSyncProtocol::State::Idle");
                if !self.is_intersect_found {
                    // request an intersect with the server so we know where to start syncing blocks
                    let chain_blocks = vec![
                        // Last byron block of mainnet
                        (4492799, hex::decode("f8084c61b6a238acec985b59310b6ecec49c0ab8352249afd7268da5cff2a457").unwrap()),
                        // Last byron block of testnet
                        (1598399, hex::decode("7e16781b40ebf8b6da18f7b5e8ade855d6738095ef2f1c58c77e88b6e45997a4").unwrap()),
                    ];
                    let payload = self.msg_find_intersect(chain_blocks);
                    self.state = State::Intersect;
                    Some(payload)
                } else {
                    // request the next block from the server.
                    let payload = self.msg_request_next();
                    self.state = State::CanAwait;
                    Some(payload)
                }
            }
            State::Intersect => {
                debug!("ChainSyncProtocol::State::Intersect");
                None
            }
            State::CanAwait => {
                debug!("ChainSyncProtocol::State::CanAwait");
                None
            }
            State::MustReply => {
                debug!("ChainSyncProtocol::State::MustReply");
                None
            }
            State::Done => {
                debug!("ChainSyncProtocol::State::Done");
                None
            }
        };
    }

    fn receive_data(&mut self, data: Vec<u8>) {
        //msgRequestNext         = [0]
        //msgAwaitReply          = [1]
        //msgRollForward         = [2, wrappedHeader, tip]
        //msgRollBackward        = [3, point, tip]
        //msgFindIntersect       = [4, points]
        //msgIntersectFound      = [5, point, tip]
        //msgIntersectNotFound   = [6, tip]
        //chainSyncMsgDone       = [7]

        let cbor_value: Value = de::from_slice(&data[..]).unwrap();
        match cbor_value {
            Value::Array(cbor_array) => {
                match cbor_array[0] {
                    Value::Integer(message_id) => {
                        match message_id {
                            1 => {
                                // Server wants us to wait a bit until it gets a new block
                                self.state = State::MustReply;
                            }
                            2 => {
                                // MsgRollForward
                                // println!("MsgRollForward: {:?}", cbor_array);
                                let (msg_roll_forward, tip) = parse_msg_roll_forward(cbor_array);

                                if self.last_log_time.elapsed().as_millis() > 5_000 {
                                    info!("slot {} of {}.", msg_roll_forward.slot_number, tip.slot_number);
                                    self.last_log_time = Instant::now()
                                }
                                self.state = State::Idle;

                                // testing only so we sync only a single block
                                // self.state = State::Done;
                                // self.result = Some(Ok(String::from("Done")))
                            }
                            3 => {
                                // MsgRollBackward
                                let slot = parse_msg_roll_backward(cbor_array);
                                warn!("rollback to slot: {}", slot);
                                self.state = State::Idle;
                            }
                            5 => {
                                debug!("MsgIntersectFound: {:?}", cbor_array);
                                self.is_intersect_found = true;
                                self.state = State::Idle;
                            }
                            6 => {
                                error!("MsgIntersectNotFound: {:?}", cbor_array);
                                self.is_intersect_found = true; // should start syncing at first byron block. will probably crash later, but oh well.
                                self.state = State::Idle;
                            }
                            7 => {
                                warn!("MsgDone: {:?}", cbor_array);
                                self.state = State::Done;
                                self.result = Some(Ok(String::from("Done")))
                            }
                            _ => {
                                error!("Got unexpected message_id: {}", message_id);
                            }
                        }
                    }
                    _ => {
                        error!("Unexpected cbor!")
                    }
                }
            }
            _ => {
                error!("Unexpected cbor!")
            }
        }
    }
}
