use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pallas_crypto::hash::Hasher;
use serde::Deserialize;
use serde_json::{json, Value};

const POOL_ID: &str = "node-parity";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Schedule {
    mode: String,
    first_slot: u64,
    slot_count: u64,
    pool_stake: String,
    active_stake: String,
    f_numerator: String,
    f_denominator: String,
    d_numerator: String,
    nonce_hex: String,
    vrf_skey_hex: String,
    assigned_slots: Vec<u64>,
}

fn schedule(mode: &str, f_numerator: &str, f_denominator: &str, pool_stake: &str, d_numerator: &str) -> Schedule {
    #[derive(Deserialize)]
    struct Fixtures {
        cases: Vec<Schedule>,
    }
    let fixtures: Fixtures = serde_json::from_str(include_str!("data/leaderlog/node-schedules.json")).unwrap();
    fixtures
        .cases
        .into_iter()
        .find(|case| {
            case.mode == mode
                && case.f_numerator == f_numerator
                && case.f_denominator == f_denominator
                && case.pool_stake == pool_stake
                && case.d_numerator == d_numerator
                && case.slot_count < 432_000
        })
        .expect("committed oracle schedule")
}

struct FixtureDir {
    path: PathBuf,
    start_time: u64,
}

impl FixtureDir {
    fn new(case: &Schedule, coefficient: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cncli-node-parity-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&path).unwrap();
        rusqlite::Connection::open(path.join("chain.db"))
            .unwrap()
            .execute_batch("VACUUM;")
            .unwrap();
        let start_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
            - (case.first_slot + 2 * case.slot_count + 300);
        assert!(case.first_slot > case.slot_count);
        assert_eq!((case.first_slot - case.slot_count) % 10, 0);
        fs::write(
            path.join("byron.json"),
            serde_json::to_vec(&json!({
                "startTime": start_time,
                "protocolConsts": {"k": (case.first_slot - case.slot_count) / 10},
                "blockVersionData": {"slotDuration": "1000"}
            }))
            .unwrap(),
        )
        .unwrap();
        // Preserve the supplied decimal lexeme, rather than round-trip it through f64.
        fs::write(
            path.join("shelley.json"),
            format!(
                "{{\"activeSlotsCoeff\":{coefficient},\"networkMagic\":42,\"slotLength\":1,\"epochLength\":{}}}",
                case.slot_count
            ),
        )
        .unwrap();
        fs::write(
            path.join("pool.vrf.skey"),
            serde_json::to_vec(&json!({
                "type": "VrfSigningKey_PraosVRF",
                "description": "public zero-seed node oracle fixture",
                "cborHex": format!("5840{}", case.vrf_skey_hex)
            }))
            .unwrap(),
        )
        .unwrap();
        Self { path, start_time }
    }

    fn args(&self, case: &Schedule, consensus: &str, d: &str) -> Vec<String> {
        vec![
            "leaderlog".into(),
            "--db".into(),
            self.path.join("chain.db").display().to_string(),
            "--byron-genesis".into(),
            self.path.join("byron.json").display().to_string(),
            "--shelley-genesis".into(),
            self.path.join("shelley.json").display().to_string(),
            "--pool-stake".into(),
            case.pool_stake.clone(),
            "--active-stake".into(),
            case.active_stake.clone(),
            format!("--d={d}"),
            "--pool-id".into(),
            POOL_ID.into(),
            "--pool-vrf-skey".into(),
            self.path.join("pool.vrf.skey").display().to_string(),
            "--tz".into(),
            "UTC".into(),
            "--consensus".into(),
            consensus.into(),
            "--shelley-transition-epoch".into(),
            "1".into(),
            "--nonce".into(),
            case.nonce_hex.clone(),
            "--epoch".into(),
            "2".into(),
        ]
    }

    fn assert_not_saved(&self) {
        let count: i64 = rusqlite::Connection::open(self.path.join("chain.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM slots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "invalid input must not save a partial schedule");
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(args: &[String]) -> (ExitStatus, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cncli"))
        .args(args)
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Drain both pipes while waiting so full-slot JSON cannot block the child.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(600);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture leaderlog exceeded 600-second deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().unwrap();
    let stderr = stderr.join().unwrap();
    let body = serde_json::from_slice(&stdout).unwrap_or_else(|error| {
        panic!(
            "invalid CLI JSON ({error}); stdout: {}; stderr: {}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        )
    });
    (status, body)
}

fn assert_schedule(case: &Schedule, consensus: &str, coefficient: &str, d: &str) {
    let fixture = FixtureDir::new(case, coefficient);
    let (status, body) = run(&fixture.args(case, consensus, d));
    assert!(status.success(), "{consensus}: {body}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["epoch"], 2);
    assert_eq!(body["epochNonce"], case.nonce_hex);
    assert_eq!(body["consensus"], consensus);
    assert_eq!(body["poolId"], POOL_ID);
    assert_eq!(body["activeStake"], case.pool_stake.parse::<u64>().unwrap());
    assert_eq!(body["totalActiveStake"], case.active_stake.parse::<u64>().unwrap());
    assert_eq!(body["f"], coefficient.parse::<f64>().unwrap());
    assert_eq!(body["d"], d.parse::<f64>().unwrap());
    assert_eq!(body["epochSlots"], case.assigned_slots.len());
    let expected: Vec<Value> = case
        .assigned_slots
        .iter()
        .enumerate()
        .map(|(index, &slot)| {
            let at = chrono::DateTime::from_timestamp((fixture.start_time + slot) as i64, 0)
                .unwrap()
                .to_rfc3339();
            json!({"no": index + 1, "slot": slot, "slotInEpoch": slot - case.first_slot, "at": at})
        })
        .collect();
    assert_eq!(
        body["assignedSlots"],
        json!(expected),
        "full oracle-assigned schedule must match"
    );
    assert!(case.assigned_slots.windows(2).all(|pair| pair[0] < pair[1]));
    let connection = rusqlite::Connection::open(fixture.path.join("chain.db")).unwrap();
    let (quantity, slots, hash): (u64, String, String) = connection
        .query_row(
            "SELECT slot_qty,slots,hash FROM slots WHERE epoch=2 AND pool_id=?1",
            [POOL_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let compact_slots = serde_json::to_string(&case.assigned_slots).unwrap();
    let sigma = case.pool_stake.parse::<f64>().unwrap() / case.active_stake.parse::<f64>().unwrap();
    let overlay_multiplier = if consensus == "tpraos" {
        1.0 - d.parse::<f64>().unwrap()
    } else {
        1.0
    };
    // These are display-only expectations; eligibility comes solely from oracle slots.
    let ideal = (case.slot_count as f64 * sigma * coefficient.parse::<f64>().unwrap() * overlay_multiplier * 100.0)
        .round()
        / 100.0;
    assert_eq!(body["epochSlotsIdeal"], ideal);
    if ideal == 0.0 {
        assert!(
            body["maxPerformance"].is_null(),
            "preserve the zero-ideal JSON representation"
        );
    } else {
        let performance = (case.assigned_slots.len() as f64 / ideal * 10000.0).round() / 100.0;
        assert_eq!(body["maxPerformance"], performance);
    }
    assert_eq!(quantity, case.assigned_slots.len() as u64);
    assert_eq!(slots, compact_slots);
    assert_eq!(serde_json::from_str::<Vec<u64>>(&slots).unwrap(), case.assigned_slots);
    assert_eq!(hash, Hasher::<256>::hash(compact_slots.as_bytes()).to_string());
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM slots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn tiny_coefficient_half_stake_matches_node() {
    for mode in ["tpraos", "praos", "cpraos"] {
        let case = schedule(if mode == "cpraos" { "praos" } else { mode }, "1", "100000", "1", "0");
        assert_schedule(&case, mode, "0.00001", "0");
    }
}

#[test]
fn coefficient_one_zero_stake_matches_node() {
    for mode in ["tpraos", "praos", "cpraos"] {
        let case = schedule(if mode == "cpraos" { "praos" } else { mode }, "1", "1", "0", "0");
        assert_eq!(case.assigned_slots.len() as u64, case.slot_count);
        assert_schedule(&case, mode, "1", "0");
    }
}

#[test]
fn high_significance_coefficient_matches_node() {
    for mode in ["tpraos", "praos", "cpraos"] {
        let case = schedule(
            if mode == "cpraos" { "praos" } else { mode },
            "12345678901234567",
            "100000000000000000",
            "1",
            "0",
        );
        assert_schedule(&case, mode, "0.12345678901234567", "0");
    }
}

#[test]
fn tpraos_exact_overlay_coefficient_matches_node() {
    let case = schedule("tpraos", "1", "1", "1", "1");
    assert!(!case.assigned_slots.contains(&case.first_slot));
    assert!(!case.assigned_slots.contains(&(case.first_slot + 2500)));
    assert!(case.assigned_slots.contains(&(case.first_slot + 2499)));
    assert_schedule(&case, "tpraos", "1", "0.0004");
}

#[test]
fn praos_and_cpraos_ignore_nonzero_decentralization() {
    let case = schedule("praos", "1", "20", "2", "0");
    assert!(!case.assigned_slots.is_empty());
    for mode in ["praos", "cpraos"] {
        for d in ["0.0004", "0.5", "1"] {
            assert_schedule(&case, mode, "0.05", d);
        }
    }
}

#[test]
fn zero_stake_below_coefficient_one_saves_empty_node_schedule() {
    for mode in ["tpraos", "praos", "cpraos"] {
        let case = schedule(if mode == "cpraos" { "praos" } else { mode }, "1", "20", "0", "0");
        assert!(case.assigned_slots.is_empty());
        assert_schedule(&case, mode, "0.05", "0");
    }
}

#[test]
fn invalid_input_does_not_save_partial_schedule() {
    let case = schedule("praos", "1", "20", "2", "0");
    for mode in ["tpraos", "praos", "cpraos"] {
        for (coefficient, d, replacement) in [
            ("0", "0", None),
            ("-0.1", "0", None),
            ("1.1", "0", None),
            ("0.05", "-0.1", None),
            ("0.05", "1.1", None),
            ("0.05", "NaN", None),
            ("0.05", "0", Some(("--active-stake", "0"))),
            ("0.05", "0", Some(("--pool-stake", "4"))),
            ("0.05", "0", Some(("--nonce", "zz"))),
        ] {
            let fixture = FixtureDir::new(&case, coefficient);
            let mut args = fixture.args(&case, mode, d);
            if let Some((flag, value)) = replacement {
                let index = args.iter().position(|arg| arg == flag).unwrap();
                args[index + 1] = value.into();
            }
            let (status, body) = run(&args);
            assert_eq!(status.code(), Some(1), "{mode}: {body}");
            assert_eq!(body["status"], "error");
            assert!(body["errorMessage"].is_string(), "{body}");
            fixture.assert_not_saved();
        }
    }
}
