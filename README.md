# CNCLI

A community-based ```cardano-node``` CLI tool. It's a collection of utilities to enhance and extend beyond those available with the ```cardano-cli```.

![Build Status](https://github.com/cardano-community/cncli/actions/workflows/ci.yml/badge.svg?branch=develop)

## Installation

To install CNCLI using release binaries or compile the Rust code, or update to a newer version, refer to the [installation guide](INSTALL.md). It covers staged checksum/attestation verification and least-authority `systemd` services for sync, sendtip and the leaderlog timer.

All Pallas dependencies are pinned to upstream commit
[`d1dfe10e2bac83c7d3b5371a72814354a5acd1ba`](https://github.com/txpipe/pallas/commit/d1dfe10e2bac83c7d3b5371a72814354a5acd1ba)
for the Cardano-node fixed-point arithmetic fix, pending a release containing it.
Cargo resolves both direct and transitive Pallas crates from this exact revision;
no sibling Pallas checkout is required. CNCLI pins stable Rust 1.98.1;
Pallas retains its own toolchain and MSRV policy.


## Usage & Examples

For a list of CNCLI commands and related usage examples, please refer to the [usage guide](USAGE.md).

Results are written to stdout; tracing logs go to stderr. Operational/validation failures return exit 1 and one JSON error (unless stdout itself failed). Found orphaned blocks remain successful validation results. Shell integrations must check the exit status, not search output for `"error"`; successful sendslots has no success envelope.

Ping's timeout bounds DNS, connection and handshake together. `sync --no-service` succeeds only after reaching/committing the tip and returns its first failure. Sendtip is best-effort latest-tip reporting: newer pending tips replace older ones, rollback/disconnect invalidates stale tips, and HTTP failures wait for the next header rather than replaying a backlog.

Use SQLite for concurrent sync and leaderlog. Redb remains supported for serialized/offline operations; it is not a shared live writer. See [deployment guidance](INSTALL.md#automation).

## Contributors

CNCLI is provided free of charge to the Cardano stake pool operator community. If you want to support its continued development, you can delegate or recommend the pools of our contributors:

- [Andrew Westberg](https://github.com/AndrewWestberg) - [**BCSH**](https://bluecheesestakehouse.com/)
- [Michael Fazio](https://github.com/michaeljfazio) - [**SAND**](https://www.sandstone.io/)
- [Andrea Callea](https://github.com/gacallea/)
- [Thomas Diesler](https://github.com/tdiesler/)

### Contributing

Before submitting a pull request ensure that all tests pass, code is correctly formatted and linted, and that no common mistakes have been made, by running the following commands:

```bash
cargo check
```

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy -- -D warnings
```

```bash
cargo test
```
### Leaderlog node parity

`tests/reference/leaderlog` builds an isolated oracle against the recorded ledger package
pins and freeze. It is not an installed-node version assertion. Expectations in
`tests/data/leaderlog` come from those real Haskell packages; C numerical goldens remain
untouched and are not comparator authority. Extra arithmetic rows at `aRaw=10^34` record
the oracle's division-by-zero error; production leadership uses the guarded integer
reciprocal at the inclusive VRF bound.

```bash
cabal build --project-file=tests/reference/leaderlog/cabal.project exe:jormanager-leaderlog-oracle
ORACLE=$(cabal list-bin --project-file=tests/reference/leaderlog/cabal.project exe:jormanager-leaderlog-oracle)
cargo +1.98.1 build --release --locked
python3 tests/reference/leaderlog/verify.py --check \
  --ledger-source /path/to/pinned/cardano-ledger --oracle "$ORACLE" \
  --cncli target/x86_64-unknown-linux-gnu/release/cncli
NODE_ORACLE="$ORACLE" cargo +1.98.1 test --release --lib live_node_differential -- --ignored
```

The verifier checks every assigned slot, emitted JSON, SQLite slot list and Blake2b hash
using fresh temporary databases and the public zero-seed fixture key. Add `--benchmark`
for one warmup and five measured runs of each 432,000-slot fixture, including child peak
RSS. Repeat schedule tests with `RAYON_NUM_THREADS=1`, `2`, and `4` to check deterministic
ordering. The production loop prepares keys once and collects ordered 1024-slot chunks
in windows bounded by the existing Rayon worker count.

The current pin passed independent node-fixture verification and 116 actual CLI
schedule replays, including full-epoch TPraos/Praos schedules, emitted JSON, SQLite
slots and hashes. The live differential also passed 10,000 deterministic value
cases, 32 boundary searches and 32 randomized schedules. Ordinary Rust regressions
cover all committed signed-floor primitives and comparator classifications/iteration
counts, in addition to node values and schedules. This establishes parity against
the recorded oracle package resolution, not an unspecified installed node.


## License

CNCLI is licensed under the terms of the [Apache License](LICENSE) version 2.
