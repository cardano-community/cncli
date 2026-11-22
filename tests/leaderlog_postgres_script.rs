#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, PartialEq, Eq)]
struct LeaderlogRow {
    id: i64,
    epoch: i64,
    nr: i64,
    slot: Option<i64>,
    scheduled_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PsqlCall {
    args: Vec<String>,
    stdin: String,
}

struct Harness {
    dir: PathBuf,
    bin_dir: PathBuf,
    db_path: PathBuf,
    socket_path: PathBuf,
    _socket_listener: UnixListener,
    psql_log: PathBuf,
    jq_log: PathBuf,
    date_log: PathBuf,
    cncli_log: PathBuf,
    fixtures_dir: PathBuf,
}

impl Harness {
    fn new(name: &str) -> Option<Self> {
        let real_jq = find_in_path("jq");
        let real_python3 = find_in_path("python3");
        let real_date = find_in_path("date");
        if cfg!(target_env = "musl") && (real_jq.is_none() || real_python3.is_none()) {
            return None;
        }
        let real_jq = real_jq.expect("jq must be installed on linux-gnu test runner");
        let real_python3 = real_python3.expect("python3 must be installed on linux-gnu test runner");
        let real_date = real_date.expect("date must be installed on linux test runner");

        let dir = std::env::temp_dir().join(format!("cncli-pg-script-{name}-{}", uuid::Uuid::now_v7()));
        let bin_dir = dir.join("bin");
        let log_dir = dir.join("logs");
        let fixtures_dir = dir.join("fixtures");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&fixtures_dir).unwrap();

        let socket_path = dir.join("node.socket");
        let socket_listener = UnixListener::bind(&socket_path).unwrap();

        for file in ["vrf.skey", "shelley.json", "byron.json", "cncli.db", "pooltool.json"] {
            fs::write(dir.join(file), "{}").unwrap();
        }

        let db_path = dir.join("postgres.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE leaderlog (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                epoch INTEGER NOT NULL,
                nr INTEGER NOT NULL,
                slot INTEGER DEFAULT NULL UNIQUE,
                scheduled_at TEXT DEFAULT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .unwrap();
        drop(conn);

        let psql_log = dir.join("psql_calls.jsonl");
        let jq_log = dir.join("jq_calls.log");
        let date_log = dir.join("date_calls.log");
        let cncli_log = dir.join("cncli_calls.log");

        write_executable(
            &bin_dir.join("shred"),
            r#"#!/usr/bin/env bash
rm -f -- "${@: -1}"
"#,
        );

        write_executable(
            &bin_dir.join("cardano-cli"),
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--version" ]]; then
  echo "cardano-cli 10.5.1.0 - linux-x86_64 - ghc-9.6"
  exit 0
fi
cat <<'JSON'
{
  "pools": {
    "pool": {
      "stakeMark": 1000000,
      "stakeSet": 1000000,
      "stakeGo": 1000000
    }
  },
  "total": {
    "stakeMark": 10000000,
    "stakeSet": 10000000,
    "stakeGo": 10000000
  }
}
JSON
"#,
        );

        write_executable(
            &bin_dir.join("cncli"),
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> {cncli_log:?}
if [[ "${{1:-}}" == "--version" ]]; then
  echo "cncli 6.8.3"
  exit 0
fi
subcmd="${{1:-}}"
case "$subcmd" in
  leaderlog)
    ledger_set="current"
    while [[ $# -gt 0 ]]; do
      if [[ "$1" == "--ledger-set" ]]; then
        ledger_set="$2"
        break
      fi
      shift
    done
    cat {fixtures_dir:?}/"${{ledger_set}}.json"
    ;;
  status)
    echo '{{"status":"ok"}}'
    ;;
  sendslots)
    exit 0
    ;;
  *)
    echo "unexpected cncli subcommand: $subcmd" >&2
    exit 1
    ;;
esac
"#,
                cncli_log = cncli_log.display().to_string(),
                fixtures_dir = fixtures_dir.display().to_string(),
            ),
        );

        write_executable(
            &bin_dir.join("jq"),
            &format!(
                r#"#!/usr/bin/env bash
printf '%s\n' "${{*//$'\n'/ }}" >> {jq_log:?}
exec {real_jq:?} "$@"
"#,
                jq_log = jq_log.display().to_string(),
                real_jq = real_jq.display().to_string(),
            ),
        );

        write_executable(
            &bin_dir.join("date"),
            &format!(
                r#"#!/usr/bin/env bash
if [[ "$*" != "-u +%Y-%m-%dT%H:%M:%SZ" ]]; then
  printf '%s\n' "$*" >> {date_log:?}
fi
if [[ -n "${{FAKE_NOW_EPOCH:-}}" && $# -eq 1 && "$1" == "+%s" ]]; then
  printf '%s\n' "${{FAKE_NOW_EPOCH}}"
  exit 0
fi
exec {real_date:?} "$@"
"#,
                date_log = date_log.display().to_string(),
                real_date = real_date.display().to_string(),
            ),
        );

        write_executable(
            &bin_dir.join("psql"),
            &format!(
                r#"#!{}
import json, os, sqlite3, sys

args = sys.argv[1:]
stdin_sql = ""
inline_cmd = None

idx = 0
single_tx = False
on_error_stop = False
while idx < len(args):
    arg = args[idx]
    if arg == "--single-transaction" or arg == "-1":
        single_tx = True
    elif arg in ("-v", "--set", "--variable") and idx + 1 < len(args):
        if args[idx + 1] == "ON_ERROR_STOP=1":
            on_error_stop = True
        idx += 1
    elif arg.startswith("--set=") and arg.split("=", 1)[1] == "ON_ERROR_STOP=1":
        on_error_stop = True
    elif arg in ("-c", "-qc") and idx + 1 < len(args):
        inline_cmd = args[idx + 1]
        idx += 1
    idx += 1

if inline_cmd is None:
    stdin_sql = sys.stdin.read()

with open({psql_log:?}, "a", encoding="utf-8") as log_file:
    log_file.write(json.dumps({{"args": args, "stdin": stdin_sql if inline_cmd is None else inline_cmd}}) + "\n")

forced_exit = os.environ.get("FAKE_PSQL_EXIT_CODE")
if forced_exit:
    sys.exit(int(forced_exit))

if inline_cmd == "\\conninfo":
    print("You are connected to database")
    sys.exit(0)

script = inline_cmd if inline_cmd is not None else stdin_sql
conn = sqlite3.connect({db_path:?}, isolation_level=None)
try:
    if single_tx:
        conn.execute("BEGIN")
    buf = ""
    for line in script.splitlines(keepends=True):
        buf += line
        if sqlite3.complete_statement(buf):
            stmt = buf.strip()
            buf = ""
            if stmt:
                conn.execute(stmt)
    if buf.strip():
        conn.execute(buf.strip())
    if single_tx:
        conn.execute("COMMIT")
except sqlite3.Error as err:
    sys.stderr.write(f"psql sql error: {{err}}\n")
    if single_tx:
        try:
            conn.execute("ROLLBACK")
        except sqlite3.Error:
            pass
    if on_error_stop or inline_cmd is not None:
        sys.exit(3)
finally:
    conn.close()
"#,
                real_python3.display(),
                psql_log = psql_log.display().to_string(),
                db_path = db_path.display().to_string(),
            ),
        );

        Some(Self {
            dir,
            bin_dir,
            db_path,
            socket_path,
            _socket_listener: socket_listener,
            psql_log,
            jq_log,
            date_log,
            cncli_log,
            fixtures_dir,
        })
    }

    fn set_leaderlog_fixture(&self, ledger_set: &str, payload: &Value) {
        fs::write(
            self.fixtures_dir.join(format!("{ledger_set}.json")),
            serde_json::to_vec_pretty(payload).unwrap(),
        )
        .unwrap();
    }

    fn set_raw_leaderlog_fixture(&self, ledger_set: &str, raw: &str) {
        fs::write(self.fixtures_dir.join(format!("{ledger_set}.json")), raw).unwrap();
    }

    fn reset_logs(&self) {
        for log in [&self.psql_log, &self.jq_log, &self.date_log, &self.cncli_log] {
            let _ = fs::remove_file(log);
        }
    }

    fn script_command(&self, mode: &str, args: &[&str]) -> Command {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("cncli-leaderlog.sh");
        let orig_path = std::env::var("PATH").unwrap_or_default();
        let combined_path = format!("{}:{orig_path}", self.bin_dir.display());

        let mut cmd = Command::new("bash");
        cmd.arg(script)
            .args(args)
            .env("PATH", combined_path)
            .env("timezone", "Etc/UTC")
            .env(
                "hexStakePool",
                "0123456789abcdef0123456789abcdef0123456789abcdef01234567",
            )
            .env("CARDANO_NODE_SOCKET_PATH", &self.socket_path)
            .env("jsonPoolTool", "")
            .env("slotsCsvFile", "")
            .env("leaderPromFile", "")
            .env("mailLeaderLogTo", "")
            .env("saveToPostgres", mode)
            .env("useScriptLogging", "true")
            .env("LOG_DIR", self.dir.join("logs"))
            .env("vrfSigningKeyFile", self.dir.join("vrf.skey"))
            .env("shelleyGenesisFile", self.dir.join("shelley.json"))
            .env("byronGenesisFile", self.dir.join("byron.json"))
            .env("dbCnCli", self.dir.join("cncli.db"))
            .env("binCardanoCli", self.bin_dir.join("cardano-cli"))
            .env("binCnCli", self.bin_dir.join("cncli"))
            .env("binJq", self.bin_dir.join("jq"));
        cmd
    }

    fn run(&self, mode: &str, args: &[&str]) -> Output {
        self.script_command(mode, args).output().unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(&self.db_path).unwrap()
    }

    fn rows(&self) -> Vec<LeaderlogRow> {
        let conn = self.db();
        let mut stmt = conn
            .prepare("SELECT id, epoch, nr, slot, scheduled_at FROM leaderlog ORDER BY id ASC")
            .unwrap();
        stmt.query_map([], |row| {
            Ok(LeaderlogRow {
                id: row.get(0)?,
                epoch: row.get(1)?,
                nr: row.get(2)?,
                slot: row.get(3)?,
                scheduled_at: row.get(4)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn psql_calls(&self) -> Vec<PsqlCall> {
        read_jsonl(&self.psql_log)
    }

    fn jq_calls(&self) -> Vec<String> {
        read_lines(&self.jq_log)
    }

    fn save_to_postgres_jq_calls(&self) -> Vec<String> {
        self.jq_calls()
            .into_iter()
            .filter(|line| {
                line != "--version"
                    && line != "-er .status"
                    && !line.starts_with("-e .status == \"ok\"")
                    && !line.contains("(.at|tostring) + \",\"")
                    && line != "-er .epochSlots"
            })
            .collect()
    }

    fn slot_date_calls(&self) -> Vec<String> {
        read_lines(&self.date_log)
            .into_iter()
            .filter(|line| line != "+%s" && line != "+%s -d 2017-09-23 21:44:51 +0000")
            .collect()
    }

    fn cncli_calls(&self) -> Vec<String> {
        read_lines(&self.cncli_log)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn read_lines(path: &Path) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    read_lines(path)
        .into_iter()
        .map(|line| serde_json::from_str(&line).unwrap())
        .collect()
}

fn leaderlog_payload(epoch: u64, slots: Vec<Value>) -> Value {
    json!({
        "status": "ok",
        "epoch": epoch,
        "epochNonce": "00".repeat(32),
        "consensus": "cpraos",
        "epochSlots": slots.len(),
        "epochSlotsIdeal": slots.len() as f64,
        "maxPerformance": 100.0,
        "poolId": "0123456789abcdef0123456789abcdef0123456789abcdef01234567",
        "sigma": 0.1,
        "activeStake": 1_000_000,
        "totalActiveStake": 10_000_000,
        "d": 0.0,
        "f": 0.05,
        "assignedSlots": slots,
    })
}

fn slot_entry(no: u64, slot: u64, at: &str) -> Value {
    json!({
        "no": no,
        "slot": slot,
        "slotInEpoch": slot % 432_000,
        "at": at,
    })
}

#[test]
fn all_mode_batches_65_slots_in_single_jq_and_psql_session_without_date_calls() {
    let Some(harness) = Harness::new("all-65") else {
        return;
    };

    let slots: Vec<Value> = (1..=65_u64)
        .map(|i| {
            let minute = (i - 1) / 60;
            let second = (i - 1) % 60;
            slot_entry(
                i,
                135_000_000 + i * 20,
                &format!("2024-09-23T21:{minute:02}:{second:02}+00:00"),
            )
        })
        .collect();
    harness.set_leaderlog_fixture("current", &leaderlog_payload(512, slots));

    let csv_path = harness.dir.join("slots.csv");
    let prom_path = harness.dir.join("leader.prom");
    let output = harness
        .script_command("all", &["--test"])
        .env("slotsCsvFile", &csv_path)
        .env("leaderPromFile", &prom_path)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Saving 'all' slots to PostgreSQL... "));
    assert!(stdout.contains("Saved 65 of 65 slots to PostgreSQL."));

    let psql_calls = harness.psql_calls();
    assert_eq!(psql_calls.len(), 1, "expected exactly 1 psql session");
    assert_eq!(
        psql_calls[0].args,
        vec!["--single-transaction", "-v", "ON_ERROR_STOP=1", "-q"]
    );
    let sql_lines: Vec<&str> = psql_calls[0].stdin.lines().collect();
    assert_eq!(sql_lines.len(), 2, "expected 1 DELETE + 1 multi-row INSERT");
    assert_eq!(
        sql_lines[0],
        "DELETE FROM leaderlog WHERE epoch = 512 AND slot IS NULL;"
    );
    assert!(sql_lines[1].starts_with("INSERT INTO leaderlog (nr, slot, epoch, scheduled_at) VALUES "));
    assert!(sql_lines[1].ends_with(
        "ON CONFLICT (slot) DO UPDATE SET nr = EXCLUDED.nr, epoch = EXCLUDED.epoch, scheduled_at = EXCLUDED.scheduled_at;"
    ));

    assert_eq!(
        harness.save_to_postgres_jq_calls().len(),
        1,
        "saveToPostgres must extract all slot fields in 1 jq pass"
    );
    assert!(
        harness.slot_date_calls().is_empty(),
        "all mode must not spawn date for slot timestamps"
    );

    let rows = harness.rows();
    assert_eq!(rows.len(), 65);
    assert_eq!(rows[0].epoch, 512);
    assert_eq!(rows[0].nr, 1);
    assert_eq!(rows[0].slot, Some(135_000_020));
    assert_eq!(rows[0].scheduled_at.as_deref(), Some("2024-09-23T21:00:00+00:00"));
    assert_eq!(rows[64].nr, 65);
    assert_eq!(rows[64].slot, Some(135_001_300));
    assert_eq!(rows[64].scheduled_at.as_deref(), Some("2024-09-23T21:01:04+00:00"));

    assert_eq!(fs::read_to_string(&prom_path).unwrap(), "assigned_blocks_epoch 65\n");
    assert_eq!(fs::read_to_string(&csv_path).unwrap().lines().count(), 65);
}

#[test]
fn export_modes_none_secure_and_past_preserve_rows_and_contiguous_order() {
    let Some(harness) = Harness::new("modes") else {
        return;
    };

    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(
            512,
            vec![
                slot_entry(1, 1001, "2024-09-23T21:44:50+00:00"),
                slot_entry(2, 1002, "2024-09-23T21:44:51+00:00"),
                slot_entry(3, 1003, "2024-09-23T21:44:40+00:00"),
                slot_entry(4, 1004, "2024-09-23T21:45:10+00:00"),
            ],
        ),
    );

    // 1. none mode
    let none_out = harness.run("none", &["--test"]);
    assert!(none_out.status.success());
    assert!(String::from_utf8_lossy(&none_out.stdout).contains("Not saving any slots to PostgreSQL."));
    assert!(harness.psql_calls().is_empty());
    assert!(harness.save_to_postgres_jq_calls().is_empty());
    assert_eq!(harness.jq_calls().len(), 2, "only --version and .status jq calls");
    assert!(harness.slot_date_calls().is_empty());
    assert!(harness.rows().is_empty());

    // 2. secure mode (with stale null-slot row for epoch 512 and another epoch 511)
    harness
        .db()
        .execute_batch(
            "INSERT INTO leaderlog (nr, epoch) VALUES (99, 512);
             INSERT INTO leaderlog (nr, epoch) VALUES (7, 511);",
        )
        .unwrap();
    harness.reset_logs();

    let secure_out = harness.run("secure", &["--test"]);
    assert!(secure_out.status.success());
    assert!(String::from_utf8_lossy(&secure_out.stdout).contains("Saved 4 of 4 slots to PostgreSQL."));
    assert_eq!(harness.psql_calls().len(), 1);
    assert_eq!(harness.save_to_postgres_jq_calls().len(), 1);
    assert_eq!(harness.jq_calls().len(), 3);
    assert!(
        harness.slot_date_calls().is_empty(),
        "secure mode must skip slot date calls"
    );

    let secure_rows = harness.rows();
    assert_eq!(secure_rows.len(), 5);
    assert_eq!(secure_rows[0].epoch, 511);
    assert_eq!(secure_rows[0].nr, 7);
    for (idx, row) in secure_rows[1..].iter().enumerate() {
        assert_eq!(row.epoch, 512);
        assert_eq!(row.nr, (idx + 1) as i64);
        assert_eq!(row.slot, None);
        assert_eq!(row.scheduled_at, None);
    }

    // 3. past mode at exact boundary 2024-09-23T21:44:51+00:00 (1727127891):
    //    nr=1 (1727127890 < now)  -> past (full)
    //    nr=2 (1727127891 == now) -> future/non-past (secure)
    //    nr=3 (1727127880 < now)  -> past (full)
    //    nr=4 (1727127910 > now)  -> future (secure)
    harness.reset_logs();
    let past_out = harness
        .script_command("past", &["--test"])
        .env("FAKE_NOW_EPOCH", "1727127891")
        .output()
        .unwrap();
    assert!(
        past_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&past_out.stderr)
    );
    assert_eq!(harness.psql_calls().len(), 1);
    assert_eq!(harness.save_to_postgres_jq_calls().len(), 1);

    let date_calls = harness.slot_date_calls();
    assert_eq!(
        date_calls,
        vec!["-f - +%s"],
        "past mode must batch timestamps in 1 date call"
    );

    let past_rows: Vec<LeaderlogRow> = harness.rows().into_iter().filter(|row| row.epoch == 512).collect();
    assert_eq!(past_rows.len(), 4);
    assert!(
        past_rows.windows(2).all(|pair| pair[0].id < pair[1].id),
        "ids must stay strictly increasing"
    );
    assert_eq!(
        past_rows
            .iter()
            .map(|r| (r.nr, r.slot, r.scheduled_at.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (1, Some(1001), Some("2024-09-23T21:44:50+00:00")),
            (2, None, None),
            (3, Some(1003), Some("2024-09-23T21:44:40+00:00")),
            (4, None, None),
        ]
    );
}

#[test]
fn empty_epochs_run_only_null_slot_delete_and_skip_date_across_modes() {
    let Some(harness) = Harness::new("empty") else {
        return;
    };

    harness.set_leaderlog_fixture("current", &leaderlog_payload(515, vec![]));

    for mode in ["all", "past", "secure"] {
        harness
            .db()
            .execute_batch(
                "DELETE FROM leaderlog;
                 INSERT INTO leaderlog (nr, epoch) VALUES (1, 515);
                 INSERT INTO leaderlog (nr, slot, epoch, scheduled_at) VALUES (2, 2002, 515, '2024-09-23T00:00:00Z');
                 INSERT INTO leaderlog (nr, epoch) VALUES (1, 514);",
            )
            .unwrap();
        harness.reset_logs();

        let output = harness
            .script_command(mode, &["--test"])
            .env("FAKE_NOW_EPOCH", "1727127891")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "mode={mode} failed: stdout={stdout}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("Saved 0 of 0 slots to PostgreSQL."));

        let psql_calls = harness.psql_calls();
        assert_eq!(psql_calls.len(), 1);
        assert_eq!(
            psql_calls[0].stdin.trim(),
            "DELETE FROM leaderlog WHERE epoch = 515 AND slot IS NULL;"
        );
        assert_eq!(harness.save_to_postgres_jq_calls().len(), 1);
        assert!(
            harness.slot_date_calls().is_empty(),
            "empty epoch in mode={mode} must not call date"
        );

        let rows = harness.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].epoch, rows[0].nr, rows[0].slot), (515, 2, Some(2002)));
        assert_eq!((rows[1].epoch, rows[1].nr, rows[1].slot), (514, 1, None));
    }
}

#[test]
fn conflict_upserts_and_single_quote_escaping_update_existing_slots() {
    let Some(harness) = Harness::new("upsert-quotes") else {
        return;
    };

    harness
        .db()
        .execute_batch(
            "INSERT INTO leaderlog (nr, slot, epoch, scheduled_at) VALUES (9, 5001, 511, '2024-09-20T00:00:00+00:00');
             INSERT INTO leaderlog (nr, epoch) VALUES (2, 512);",
        )
        .unwrap();
    let original_id: i64 = harness
        .db()
        .query_row("SELECT id FROM leaderlog WHERE slot = 5001", [], |row| row.get(0))
        .unwrap();

    // Include single quotes, doubled single quotes, and duplicate conflict key in one payload.
    let quoted_at = "2024-09-23T21:44:10'O''Clock+00:00";
    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(
            512,
            vec![
                slot_entry(1, 5001, "2024-09-23T21:44:00+00:00"),
                slot_entry(2, 5001, quoted_at),
                slot_entry(3, 5002, "2024-09-23T21:44:20+00:00"),
            ],
        ),
    );

    let output = harness.run("all", &["--test"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(harness.psql_calls().len(), 1);

    let rows = harness.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, original_id);
    assert_eq!(rows[0].epoch, 512);
    assert_eq!(rows[0].nr, 2);
    assert_eq!(rows[0].slot, Some(5001));
    assert_eq!(rows[0].scheduled_at.as_deref(), Some(quoted_at));
    assert_eq!(rows[1].epoch, 512);
    assert_eq!(rows[1].nr, 3);
    assert_eq!(rows[1].slot, Some(5002));

    // Verify past mode also upserts conflicting past slots via EXCLUDED values.
    harness.reset_logs();
    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(
            512,
            vec![
                slot_entry(10, 5002, "2024-09-23T21:44:30+00:00"),
                slot_entry(11, 5003, "2024-09-23T21:46:00+00:00"),
            ],
        ),
    );
    let past_out = harness
        .script_command("past", &["--test"])
        .env("FAKE_NOW_EPOCH", "1727127891")
        .output()
        .unwrap();
    assert!(past_out.status.success());
    assert_eq!(harness.psql_calls().len(), 1);

    let updated = harness.rows();
    assert_eq!(updated.len(), 3);
    assert_eq!(
        (updated[1].nr, updated[1].slot, updated[1].scheduled_at.as_deref()),
        (10, Some(5002), Some("2024-09-23T21:44:30+00:00"))
    );
    assert_eq!(
        (updated[2].nr, updated[2].slot, updated[2].scheduled_at.as_deref()),
        (11, None, None)
    );
}

#[test]
fn cutover_and_day4_runs_persist_each_epoch_in_one_psql_session() {
    let Some(harness) = Harness::new("cutover-day4") else {
        return;
    };

    harness.set_leaderlog_fixture(
        "prev",
        &leaderlog_payload(511, vec![slot_entry(1, 4001, "2024-09-18T22:00:00+00:00")]),
    );
    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(512, vec![slot_entry(1, 5001, "2024-09-23T22:00:00+00:00")]),
    );
    harness.set_leaderlog_fixture(
        "next",
        &leaderlog_payload(513, vec![slot_entry(1, 6001, "2024-09-28T22:00:00+00:00")]),
    );

    let prom_path = harness.dir.join("cutover.prom");
    let pooltool_path = harness.dir.join("pooltool.json");
    let cutover_out = harness
        .script_command("past", &["--current"])
        .env("FAKE_NOW_EPOCH", "1727127891")
        .env("jsonPoolTool", &pooltool_path)
        .env("leaderPromFile", &prom_path)
        .output()
        .unwrap();
    assert!(
        cutover_out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&cutover_out.stderr)
    );

    assert_eq!(
        harness.psql_calls().len(),
        2,
        "cutover must open 1 psql session for prev and 1 for current"
    );
    assert_eq!(harness.save_to_postgres_jq_calls().len(), 2);
    assert!(
        harness.cncli_calls().iter().any(|call| call.starts_with("sendslots ")),
        "cutover must still invoke cncli sendslots"
    );
    assert_eq!(fs::read_to_string(&prom_path).unwrap(), "assigned_blocks_epoch 1\n");

    harness.reset_logs();
    let csv_path = harness.dir.join("next_slots.csv");
    let next_out = harness
        .script_command("secure", &["--next"])
        .env("slotsCsvFile", &csv_path)
        .output()
        .unwrap();
    assert!(next_out.status.success());
    assert_eq!(harness.psql_calls().len(), 1);
    assert_eq!(harness.save_to_postgres_jq_calls().len(), 1);
    assert_eq!(
        fs::read_to_string(&csv_path).unwrap(),
        "2024-09-28T22:00:00+00:00,6001,1\n"
    );

    let rows = harness.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0].epoch, rows[0].slot), (511, Some(4001)));
    assert_eq!((rows[1].epoch, rows[1].slot), (512, None));
    assert_eq!((rows[2].epoch, rows[2].slot), (513, None));
}

#[test]
fn validation_errors_abort_before_psql_and_sql_errors_roll_back_with_exit_code() {
    let Some(harness) = Harness::new("errors") else {
        return;
    };

    let invalid_payloads = [
        json!({"status": "ok", "epochSlots": 0, "assignedSlots": []}),
        json!({"status": "ok", "epoch": "512", "epochSlots": 0, "assignedSlots": []}),
        json!({"status": "ok", "epoch": 512.5, "epochSlots": 0, "assignedSlots": []}),
        json!({"status": "ok", "epoch": 512, "assignedSlots": []}),
        json!({"status": "ok", "epoch": 512, "epochSlots": -1, "assignedSlots": []}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 0, "assignedSlots": "not-an-array"}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"slot": 100, "at": "2024-09-23T00:00:00Z"}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": "1", "slot": 100, "at": "2024-09-23T00:00:00Z"}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "at": "2024-09-23T00:00:00Z"}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100.2, "at": "2024-09-23T00:00:00Z"}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100, "at": null}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100, "at": ""}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100, "at": 12345}]}),
        json!({"status": "ok", "epoch": 512, "epochSlots": 1, "assignedSlots": [{"no": 1, "slot": 100, "at": "2024-09-23\nT00:00:00Z"}]}),
    ];

    for payload in &invalid_payloads {
        harness.reset_logs();
        harness.set_raw_leaderlog_fixture("current", &serde_json::to_string(payload).unwrap());
        let out = harness.run("all", &["--test"]);
        assert!(!out.status.success(), "expected failure for invalid payload: {payload}");
        assert!(
            harness.psql_calls().is_empty(),
            "invalid payload must fail before opening psql: {payload}"
        );
    }

    // Invalid timestamp string in past mode must also fail before opening psql.
    harness.reset_logs();
    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(512, vec![slot_entry(1, 100, "not-a-valid-timestamp")]),
    );
    let bad_date_out = harness
        .script_command("past", &["--test"])
        .env("FAKE_NOW_EPOCH", "1727127891")
        .output()
        .unwrap();
    assert!(!bad_date_out.status.success());
    assert!(harness.psql_calls().is_empty());

    // Mid-transaction SQL failure: DELETE runs first, past INSERT runs second, future INSERT (nr=99)
    // hits trigger -> ON_ERROR_STOP=1 aborts and --single-transaction rolls back both DELETE and past INSERT.
    harness
        .db()
        .execute_batch(
            "INSERT INTO leaderlog (nr, epoch) VALUES (5, 512);
             CREATE TRIGGER fail_on_nr_99
             BEFORE INSERT ON leaderlog
             WHEN NEW.nr = 99
             BEGIN
                 SELECT RAISE(ABORT, 'simulated constraint failure');
             END;",
        )
        .unwrap();
    harness.reset_logs();
    harness.set_leaderlog_fixture(
        "current",
        &leaderlog_payload(
            512,
            vec![
                slot_entry(1, 7001, "2024-09-23T21:44:00+00:00"),
                slot_entry(99, 7002, "2024-09-23T21:46:00+00:00"),
            ],
        ),
    );

    let sql_err_out = harness
        .script_command("past", &["--test"])
        .env("FAKE_NOW_EPOCH", "1727127891")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&sql_err_out.stdout);
    assert_eq!(sql_err_out.status.code(), Some(3), "stdout={stdout}");
    assert!(!stdout.contains("Saved 2 of 2 slots to PostgreSQL."));
    assert_eq!(harness.psql_calls().len(), 1);

    let surviving_rows = harness.rows();
    assert_eq!(surviving_rows.len(), 1, "DELETE and earlier INSERT must roll back");
    assert_eq!(
        (surviving_rows[0].epoch, surviving_rows[0].nr, surviving_rows[0].slot),
        (512, 5, None)
    );

    // Connection failure exit code (2) from psql is also propagated.
    harness.reset_logs();
    let conn_err_out = harness
        .script_command("all", &["--test"])
        .env("FAKE_PSQL_EXIT_CODE", "2")
        .output()
        .unwrap();
    assert_eq!(conn_err_out.status.code(), Some(2));

    // --postgres connection-check flag remains intact.
    harness.reset_logs();
    let pg_check_out = harness.run("all", &["--postgres"]);
    assert!(pg_check_out.status.success());
    assert_eq!(harness.psql_calls().len(), 1);
    assert_eq!(harness.psql_calls()[0].args, vec!["-c", "\\conninfo"]);
}
