use std::str::FromStr;

use bigdecimal::{BigDecimal, One, Zero};
use chrono::NaiveDateTime;
use num_rational::BigRational;
use regex::Regex;

use cncli::nodeclient::math::{ceiling, exp, find_e, ln, round, split_ln};
use cncli::nodeclient::ping;
use nodeclient::leaderlog::is_overlay_slot;
use nodeclient::math::ipow;

use super::*;

#[test]
fn test_is_overlay_slot() {
    pretty_env_logger::init_timed();

    let first_slot_of_epoch = 15724800_i64;
    let mut current_slot = 16128499_i64;
    let d = BigRational::from_str("32/100").unwrap();

    assert!(!is_overlay_slot(&first_slot_of_epoch, &current_slot, &d));

    // AD test
    current_slot = 15920150_i64;
    assert!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &d));
}

#[tokio::test]
async fn test_ping() {
    let host = "preprod-node.play.dev.cardano.org".to_string();
    let port = 30000;
    let network_magic = 1;
    let mut stdout: Vec<u8> = Vec::new();

    ping::ping(&mut stdout, &host, port, network_magic, 2).await;

    assert_eq!(
        &std::str::from_utf8(&stdout).unwrap()[..85],
        "{\n  \"status\": \"ok\",\n  \"host\": \"preprod-node.play.dev.cardano.org\",\n  \"port\": 30000,\n "
    );
}

#[tokio::test]
async fn test_ping_failure_address() {
    let host = "murrika.relays-new.cardano-testnet.iohkdev.io".to_string();
    let port = 30000;
    let network_magic = 1;
    let mut stdout: Vec<u8> = Vec::new();

    ping::ping(&mut stdout, &host, port, network_magic, 2).await;

    let regex_str = ".*failed to lookup address information: .*";
    let regex = Regex::new(regex_str);
    let ping_result = std::str::from_utf8(&stdout).unwrap();
    // println!("ping_result: {}", ping_result);
    assert_eq!(regex.unwrap().is_match(ping_result), true);
}

#[tokio::test]
async fn test_ping_failure_bad_port() {
    let host = "preprod-node.play.dev.cardano.org".to_string();
    let port = 3992;
    let network_magic = 1;
    let mut stdout: Vec<u8> = Vec::new();

    ping::ping(&mut stdout, &host, port, network_magic, 2).await;

    let regex_str = ".*connect(ion)? time(out)?.*";
    let regex = Regex::new(regex_str);
    let ping_result = std::str::from_utf8(&stdout).unwrap();
    println!("ping_result: {}", ping_result);
    assert_eq!(regex.unwrap().is_match(ping_result), true);
}

#[tokio::test]
async fn test_ping_failure_bad_magic() {
    let host = "preview-node.play.dev.cardano.org".to_string();
    let port = 3001;
    let network_magic = 111111;
    let mut stdout: Vec<u8> = Vec::new();

    ping::ping(&mut stdout, &host, port, network_magic, 2).await;

    let regex_str = ".*\"Refused\\(\\d+, \\\\\"version data mismatch.*";
    let regex = Regex::new(regex_str);
    let ping_result = std::str::from_utf8(&stdout).unwrap();
    // println!("ping_result: {}", ping_result);
    assert_eq!(regex.unwrap().is_match(ping_result), true);
}

#[test]
fn test_eps() {
    let eps = BigDecimal::from_str("1.E-24").unwrap();
    // println!("1/10^24 = {}", eps);
    assert_eq!(eps.to_string(), "1E-24");
}

#[test]
fn test_ceiling() {
    let x = BigDecimal::from_str("1234.00000").unwrap();
    let ceil_x = ceiling(&x);
    assert_eq!(ceil_x, BigDecimal::from(1234));

    let y = BigDecimal::from_str("1234.0000000000123").unwrap();
    let ceil_y = ceiling(&y);
    assert_eq!(ceil_y, BigDecimal::from(1235))
}

#[test]
fn test_exp() {
    let x = BigDecimal::zero();
    let exp_x = exp(&x);
    assert_eq!(exp_x, BigDecimal::one());

    let x = BigDecimal::one();
    let exp_x = exp(&x);
    assert_eq!(exp_x.to_string(), "2.7182818284590452353602874043083282");

    let x = BigDecimal::from_str("-54.268914").unwrap();
    let exp_x = exp(&x);
    assert_eq!(exp_x.to_string(), "2.6996664594E-24");
}

#[test]
fn test_find_e() {
    let exp1 = exp(&BigDecimal::one());
    let x = BigDecimal::from_str("8.5").unwrap();
    let n = find_e(&exp1, &x);
    println!("find_e({}) = {}", &x, n);
    assert_eq!(n, 2);
}

#[test]
fn test_split_ln() {
    let exp1 = exp(&BigDecimal::one());
    let x = BigDecimal::from_str("2.9").unwrap();
    let (n, xp) = split_ln(&exp1, &x);
    println!("n: {}, xp: {}", n, xp);
}

#[test]
fn test_ln() {
    let x = BigDecimal::one();
    let ln_x = ln(&x);
    assert_eq!(ln_x.to_string(), "0E-34");

    let x = BigDecimal::from_str("0.95").unwrap();
    let ln_x = ln(&x);
    assert_eq!(ln_x.to_string(), "-0.0512932943875505334261962382072846");
    println!("ln(1-f) = ln (0.95) = {}", ln_x);
}

#[test]
fn test_infinite_range_stuff() {
    let mut range = 1..;

    let mut i = 0;

    while i < 1024 {
        let an = range.by_ref().take(1).next().unwrap();
        let bn = an * an;
        println!("an: {}, bn: {}, range: {:?}", an, bn, &range);
        i += 1;
    }

    // let x:&[i32] = &(1..1025).collect()[..];
    // let y: &[i32] = &(1..1025).map(|m| m * m).collect()[..];
    //
    // println!("x: {:?}", x);
    // println!("y: {:?}", y);
}

#[test]
fn test_pow() {
    let mut x = BigDecimal::from_str("0.2587").unwrap();
    let mut y = ipow(&x, 5);
    println!("{}^5 = {}", x, y);

    x = BigDecimal::from_str("-17.2589").unwrap();
    y = ipow(&x, 5);
    println!("{}^5 = {}", x, y);

    x = BigDecimal::from(2);
    y = ipow(&x, 512);
    println!("{}^512 = {}", x, y);
}

#[test]
fn test_leaderlog_math() {
    let sigma = BigDecimal::from_str("0.0077949348290607914969808129687391").unwrap();
    let c = BigDecimal::from_str("-0.0512932943875505334261962382072846").unwrap();
    //let x = round(&(-c * sigma), 34);
    let x = round(-c * sigma);
    println!("x: {}", x);
    assert_eq!(x.to_string(), "0.0003998278869187860731522824872380")
}

// #[test]
// fn test_ledger_state_1_26_0() {
//     // Calculate values from json
//     let ledger_state: &PathBuf = &PathBuf::from(OsString::from_str("/tmp/ledger-state-guild.json").unwrap());
//     let ledger: Ledger =
//         match serde_json::from_reader::<BufReader<File>, Ledger3>(BufReader::new(File::open(ledger_state).unwrap())) {
//             Ok(ledger3) => ledger3.state_before,
//             Err(error) => {
//                 panic!("Failed to parse ledger state: {:?}", error);
//             }
//         };
//
//     println!("ledger: {:?}", ledger);
// }

#[test]
fn test_date_parsing() {
    let genesis_start_time_sec = NaiveDateTime::parse_from_str("2022-10-25T00:00:00Z", "%Y-%m-%dT%H:%M:%S%.fZ")
        .unwrap()
        .and_utc()
        .timestamp();

    assert_eq!(genesis_start_time_sec, 1666656000);
}

#[test]
fn test_date_parsing2() {
    let genesis_start_time_sec =
        NaiveDateTime::parse_from_str("2024-05-16T17:18:10.000000000Z", "%Y-%m-%dT%H:%M:%S%.fZ")
            .unwrap()
            .and_utc()
            .timestamp();

    assert_eq!(genesis_start_time_sec, 1715879890);
}
