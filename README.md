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

## License

CNCLI is licensed under the terms of the [Apache License](LICENSE) version 2.
