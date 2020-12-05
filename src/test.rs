use rug::{Float, Rational};
use rug::float::Round;
use rug::ops::MulAssignRound;

use nodeclient::leaderlog::is_overlay_slot;
use cardano_ouroboros_network::mux;

use super::*;
use cardano_ouroboros_network::mux::Cmd;
use std::path::PathBuf;
use std::io::LineWriter;

#[test]
fn test_is_overlay_slot() {
    pretty_env_logger::init_timed();

    let first_slot_of_epoch = 15724800_i64;
    let mut current_slot = 16128499_i64;
    // let d = Float::with_val(120, 0.32);
    let mut d = Float::with_val(24, Float::parse("0.32").unwrap());
    d.mul_assign_round(100, Round::Nearest);
    let r: Rational = Rational::from((d.to_integer().unwrap(), 100));

    assert_eq!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &r), false);

    // AD test
    current_slot = 15920150_i64;
    assert_eq!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &r), true);
}

// #[test]
// fn test_ping() {
//     pretty_env_logger::init_timed();
//
//     let host = "north-america.relays-new.cardano-testnet.iohkdev.io".to_string();
//     let port = 3001_u16;
//     let network_magic = 1097911063_u32;
//     mux::start(Cmd::Ping, &PathBuf::new(), &host, port, network_magic, &String::new(), &PathBuf::new(), &String::new(), &String::new());
//
// }