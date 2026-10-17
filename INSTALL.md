# Install & Automate CNCLI

## Installation

You can install CNCLI using either the release binaries or compiling the Rust code. Both installation examples hereby illustrated are instructions for an Ubuntu Linux server and use standard system paths from the Linux [File System Hierarchy Standard](https://en.wikipedia.org/wiki/Filesystem_Hierarchy_Standard).

### Install the binary release

Choose an explicit reviewed release version and matching target from [releases](https://github.com/cardano-community/cncli/releases). Do not extract a download directly into `/usr/local/bin`. Install `gh` and authenticate as needed for attestation verification.

```bash
set -euo pipefail
version='REPLACE_WITH_RELEASE_VERSION' # without the leading v
target=x86_64-unknown-linux-gnu
archive="cncli-${version}-ubuntu22-${target}.tar.gz"
stage=$(mktemp -d)
cd "$stage"
curl --fail --location --remote-name "https://github.com/cardano-community/cncli/releases/download/v${version}/${archive}"
curl --fail --location --remote-name "https://github.com/cardano-community/cncli/releases/download/v${version}/${archive}.sha256"
sha256sum --check "${archive}.sha256"
gh attestation verify "$archive" --repo cardano-community/cncli
mkdir unpacked
tar xzf "$archive" -C unpacked
./unpacked/cncli --version
./unpacked/cncli --help
```

Before replacement, preserve the previous binary and a backend-consistent database backup. Stop the services during backup/replacement: for SQLite, use its backup API or copy all database state only after every writer has stopped; for redb, stop all users before copying. Never copy/unlock a live redb file. Review storage/nonce changes and rehearse backup/restore on disposable data first. Read the [SQLite schema v5 upgrade notes](#sqlite-schema-v5-upgrade) before opening an existing SQLite database.

```bash
sudo systemctl stop cncli-leaderlog.timer cncli-leaderlog.service cncli-sync.service cncli-sendtip.service
# Perform and verify the backend-consistent backup here, while all writers are stopped.
if test -e /usr/local/bin/cncli; then
  sudo cp -p /usr/local/bin/cncli "/usr/local/bin/cncli.previous.${version}"
fi
sudo install -o root -g root -m 0755 ./unpacked/cncli /usr/local/bin/cncli.new
sudo mv /usr/local/bin/cncli.new /usr/local/bin/cncli
sudo systemctl start cncli-sync.service cncli-sendtip.service
```

After the initial migration completes and sync catches up, require a successful `cncli status` with your database/genesis arguments before running `sudo systemctl start cncli-leaderlog.timer`.

For a first install, create the units below before starting them; omit the stop step if they do not exist. Opening the database can automatically migrate it; restoring a backup is a separate operator action. A verified attestation identifies build provenance, not source correctness or bit-reproducibility. Older releases without attestations do not satisfy this staged verification procedure.

### SQLite schema v5 upgrade

Opening a SQLite database at schema version 0–4 atomically upgrades it to version 5, adding `idx_chain_orphaned_slot_number` on `(orphaned, slot_number)` and `idx_chain_orphaned_block_number` on `(orphaned, block_number)`. These indexes accelerate active-chain reads and suffix updates without changing chain rows, nonces, saved slot schedules, orphan filtering or the 120-second freshness limit. Existing single-column indexes and operator `_681` workaround indexes remain intact; equivalent indexes under different names are not deduplicated.

Run the initial upgrade with one cncli process while other database consumers are stopped. Index creation on large databases may take several minutes and needs additional free disk/WAL space. A lock, disk or index-creation failure propagates to the command; both new index builds and the version marker roll back together, leaving the upgrade retryable. Reopening version 5 skips index builds and metadata writes.

Unmodified v6.8.1 rejects schema version 5. To roll back the binary, restore the consistent pre-migration backup; do not manually lower the schema marker.

### Compile from source

#### Prepare RUST environment

```bash
$ mkdir -p $HOME/.cargo/bin
```

```bash
$ chown -R $USER\: $HOME/.cargo
```

```bash
$ touch $HOME/.profile
```

```bash
$ chown $USER\: $HOME/.profile
```

#### Install rustup - proceed with default install (option 1)

```bash
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```bash
$ source $HOME/.cargo/env
```

The checked-out release's `rust-toolchain.toml` selects its pinned toolchain and components; let rustup install that toolchain when building. Do not substitute a floating stable/nightly or run an unreviewed toolchain update during deployment.

(Only if you want to build for the musl target)
```bash
$ rustup target add x86_64-unknown-linux-musl
```

#### Install dependencies and build cncli

Adjust the ```<latest_tag_name>``` variable in the command to the latest tag available:

```bash
$ source $HOME/.cargo/env
```

```bash
$ sudo apt-get update -y && sudo apt-get install -y automake build-essential pkg-config libffi-dev libgmp-dev libssl-dev libtinfo-dev libsystemd-dev zlib1g-dev make g++ tmux git jq wget libncursesw5 libtool autoconf musl-tools
```

```bash
$ git clone --recurse-submodules https://github.com/cardano-community/cncli
```

```bash
$ cd cncli
```

```bash
$ git checkout <latest_tag_name>
```

```bash
$ cargo build --locked --release --target x86_64-unknown-linux-gnu
```
or
```bash
$ cargo build --locked --release --target x86_64-unknown-linux-musl
```

```bash
$ target/x86_64-unknown-linux-gnu/release/cncli --version
```

Also run the staged binary's `--help` and required tests before using the same stop/backup/root-owned atomic replacement sequence above. A locally built artifact does not acquire release provenance just by being built.

### Checking that cncli is properly installed

Run the following command to check if cncli is correctly installed and available in your system ```PATH``` variable:

```bash
$ command -v cncli
```

It should return ```/usr/local/bin/cncli```.

### Updating cncli from earlier versions

Adjust the ```<latest_tag_name>``` variable in the command to the latest tag available:

Use the checked-out release's pinned toolchain, not a floating `rustup update`.

```bash
$ cd cncli
```

```bash
$ git fetch --all --prune
```

```bash
$ git checkout <latest_tag_name>
```

```bash
$ cargo build --locked --release --target x86_64-unknown-linux-gnu
```
or
```bash
$ cargo build --locked --release --target x86_64-unknown-linux-musl
```

```bash
$ target/x86_64-unknown-linux-gnu/release/cncli --version
```

Run staged `--help`, preserve the previous binary and a backend-consistent backup, then use the operator-controlled stop/atomic replacement/restart sequence above. Never overwrite the running executable with `cargo install --force`.

## Cross Platform build with Nix + Flakes

We are going to build cncli with [Nix](https://nixos.org/guides/install-nix.html) and [Nix Flakes](https://www.tweag.io/blog/2020-05-25-flakes/)

### Install Nix + Flakes

```bash
# Nix single user install
sh <(curl -L https://nixos.org/nix/install)
source ~/.nix-profile/etc/profile.d/nix.sh

# Configure Nix to also use the binary cache from IOHK
# Enable the experimental flakes feature
mkdir -p ~/.config/nix
cat << EOF > ~/.config/nix/nix.conf
trusted-public-keys = hydra.iohk.io:f/Ea+s+dFdN+3Y/G+FDgSq+a5NEWhJGzdjvKNGv0/EQ= cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
substituters = https://hydra.iohk.io https://cache.nixos.org
experimental-features = nix-command flakes
EOF

# Install Flakes
nix-shell -I nixpkgs=channel:nixos-20.03 --packages nixFlakes

# Test Nix+Flakes
nix flake show github:AndrewWestberg/cncli
github:AndrewWestberg/cncli/f8ea45b5e01bed81fbb3b848916219838786cd10
├───devShell
│   ├───aarch64-linux: development environment 'nix-shell'
│   └───x86_64-linux: development environment 'nix-shell'
├───overlay: Nixpkgs overlay
└───packages
    ├───aarch64-linux
    │   └───cncli: package 'cncli-3.1.0'
    └───x86_64-linux
        └───cncli: package 'cncli-3.1.0'
```

### Build the binary

We can now build cncli in a nix-shell that has flakes enabled

```bash
$ nix-shell -I nixpkgs=channel:nixos-20.03 --packages nixFlakes

[nix-shell:~/git/cncli]$ nix build .#cncli
```

### Build Troubleshooting

The Nix Flake build process requires plenty of file resources in $TEMPDIR.
In case you run into ...

* No space left on device
* Too many open files

Have a look [over here](https://github.com/AndrewWestberg/cncli/issues/83#issuecomment-868287041) on how to possibly fix this.

## Automation

Use three dedicated system identities, not root. The examples assume TCP node port 3000, root-owned executables under `/usr/local/bin`, and an operator-provided node socket outside protected homes (for example `/run/cardano-node/node.socket`).

SQLite is the concurrent sync + leaderlog default. Leaderlog needs database **write** access: it opens read/write stores and saves slot assignments. Redb is supported for serialized/offline operations, not shared live writers; schedule it with sync stopped and a backend-consistent disposable copy, never forced unlocking or copying a live file.

### Accounts and directories

Run these deployment commands only on your intended deployment host after review; verification uses a disposable environment. They do not move existing keys or databases.

```bash
sudo groupadd --system cncli-db
sudo groupadd --system cncli-pooltool
sudo useradd --system --user-group --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin cncli-sync
sudo useradd --system --user-group --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin cncli-sendtip
sudo useradd --system --user-group --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin cncli-leaderlog
sudo usermod -a -G cncli-db cncli-sync
sudo usermod -a -G cncli-pooltool cncli-sendtip
sudo usermod -a -G cncli-db,cncli-pooltool cncli-leaderlog
sudo install -d -o cncli-sync -g cncli-db -m 2770 /var/lib/cncli
sudo install -d -o root -g root -m 0755 /etc/cncli
sudo install -d -o cncli-leaderlog -g cncli-leaderlog -m 0700 /var/lib/cncli-leaderlog /var/log/cncli-leaderlog
sudo apt-get install -y jq
sudo install -o root -g root -m 0755 scripts/cncli-leaderlog.sh scripts/cncli-sendslots.sh /usr/local/bin/
```

Install your operator-owned inputs (substitute real source paths; do not place credentials on the command line). Executables, including `cardano-node` and `cardano-cli`, remain root:root 0755. Existing database files and SQLite sidecars must be cncli-sync:cncli-db and 0660, changed only while writers are stopped.

```bash
# Only after stopping every database writer; applies to an existing default database:
sudo find /var/lib/cncli -maxdepth 1 -type f -name 'cncli.db*' -exec chown cncli-sync:cncli-db '{}' + -exec chmod 0660 '{}' +
```

```bash
sudo install -o root -g cncli-pooltool -m 0640 /operator/source/pooltool.json /etc/cncli/pooltool.json
sudo install -o root -g cncli-leaderlog -m 0640 /operator/source/vrf.skey /etc/cncli/vrf.skey
sudo install -o root -g cncli-leaderlog -m 0640 /operator/source/leaderlog.env /etc/cncli/leaderlog.env
sudo install -o root -g root -m 0644 /operator/source/mainnet-byron-genesis.json /etc/cncli/mainnet-byron-genesis.json
sudo install -o root -g root -m 0644 /operator/source/mainnet-shelley-genesis.json /etc/cncli/mainnet-shelley-genesis.json
```

### PoolTool and helper configuration

`/etc/cncli/pooltool.json` contains your own API key and nonempty pool list. Blank API keys/pool IDs and empty pools are rejected before reporting/sending. Replace the placeholders; never commit the real file:

```json
{
  "api_key": "YOUR_POOLTOOL_API_KEY",
  "pools": [
    {
      "name": "YOUR_POOL_NAME",
      "pool_id": "YOUR_POOL_ID",
      "host": "127.0.0.1",
      "port": 3000
    }
  ]
}
```

Example `/etc/cncli/leaderlog.env` (systemd environment syntax, no `export`). Set `hexStakePool` to your actual pool ID. Supply the actual socket path and group; grant only socket access, not membership in a blanket Cardano home/key-directory group:

```text
CARDANO_NODE_SOCKET_PATH=/run/cardano-node/node.socket
hexStakePool=REPLACE_WITH_YOUR_POOL_ID
timezone=Etc/UTC
consensusMode=cpraos
jsonPoolTool=/etc/cncli/pooltool.json
slotsCsvFile=/var/lib/cncli-leaderlog/slots.csv
vrfSigningKeyFile=/etc/cncli/vrf.skey
shelleyGenesisFile=/etc/cncli/mainnet-shelley-genesis.json
byronGenesisFile=/etc/cncli/mainnet-byron-genesis.json
dbCnCli=/var/lib/cncli/cncli.db
binCardanoCli=/usr/local/bin/cardano-cli
binCnCli=/usr/local/bin/cncli
LOG_DIR=/var/log/cncli-leaderlog
```

### Systemd services

Copy each example into its named `/etc/systemd/system/` file. Replace `OPERATOR_SOCKET_GROUP` in leaderlog's unit with the node socket's actual access group (it must already exist). Configure the node to create a socket accessible to that group outside protected homes.

`cncli-sync.service`:

```ini
[Unit]
Description=CNCLI Sync
After=network-online.target

[Service]
Type=simple
User=cncli-sync
Group=cncli-sync
SupplementaryGroups=cncli-db
UMask=0007
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
CapabilityBoundingSet=
RestrictSUIDSGID=yes
ReadWritePaths=/var/lib/cncli
InaccessiblePaths=-/etc/cncli/vrf.skey -/etc/cncli/pooltool.json
Restart=on-failure
RestartSec=5
LimitNOFILE=131072
ExecStart=/usr/local/bin/cncli sync --host 127.0.0.1 --port 3000 --db /var/lib/cncli/cncli.db
KillSignal=SIGINT
StandardOutput=journal
StandardError=journal
SyslogIdentifier=cncli-sync

[Install]
WantedBy=multi-user.target
```

`cncli-sendtip.service`:

```ini
[Unit]
Description=CNCLI Sendtip
After=network-online.target

[Service]
Type=simple
User=cncli-sendtip
Group=cncli-sendtip
SupplementaryGroups=cncli-pooltool
UMask=0007
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
CapabilityBoundingSet=
RestrictSUIDSGID=yes
InaccessiblePaths=-/etc/cncli/vrf.skey
Restart=on-failure
RestartSec=5
LimitNOFILE=131072
ExecStart=/usr/local/bin/cncli sendtip --cardano-node /usr/local/bin/cardano-node --config /etc/cncli/pooltool.json
KillSignal=SIGINT
StandardOutput=journal
StandardError=journal
SyslogIdentifier=cncli-sendtip

[Install]
WantedBy=multi-user.target
```

`cncli-leaderlog.service`:

```ini
[Unit]
Description=CNCLI Leaderlog

[Service]
Type=oneshot
User=cncli-leaderlog
Group=cncli-leaderlog
SupplementaryGroups=cncli-db cncli-pooltool OPERATOR_SOCKET_GROUP
UMask=0007
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
CapabilityBoundingSet=
RestrictSUIDSGID=yes
ReadWritePaths=/var/lib/cncli /var/lib/cncli-leaderlog /var/log/cncli-leaderlog
EnvironmentFile=/etc/cncli/leaderlog.env
ExecStart=/usr/local/bin/cncli-leaderlog.sh

[Install]
WantedBy=multi-user.target
```

`cncli-leaderlog.timer` (preserves the twice-daily schedule):

```ini
[Unit]
Description=CNCLI Leaderlog

[Timer]
OnCalendar=*-*-* 09,21:50:00 UTC
Unit=cncli-leaderlog.service

[Install]
WantedBy=timers.target
```

On a new installation, install the units and run `sudo systemctl daemon-reload`, then start only `cncli-sync.service`. Once it creates `/var/lib/cncli/cncli.db`, stop it and apply the ownership/permission command above before enabling the leaderlog timer. SQLite can create the database as 0640 despite `UMask=0007`; the umask alone does not grant group write.

```bash
sudo systemd-analyze verify /etc/systemd/system/cncli-{sync,sendtip,leaderlog}.service /etc/systemd/system/cncli-leaderlog.timer
sudo systemctl daemon-reload
sudo systemctl enable --now cncli-sync.service cncli-sendtip.service cncli-leaderlog.timer
```

Validate isolation in a disposable systemd environment before deployment: sync/sendtip cannot read the VRF key, sync cannot read PoolTool configuration, none can write `/usr/local/bin`, and leaderlog can read its key and write slot assignments/outputs. These templates are not a claim that isolation has already been verified on your host.


### PostgreSQL
If you would like to store your leaderlog to Postgres you can do so by creating the following table and setting your connection details in `cncli-leaderlog.sh`. You can store slots with or without assigned slot time. For security reasons it's recommended to store the leaderlog without the assigned slot time (`saveToPostgres=secure`).

```
create table leaderlog (
    id bigserial primary key,
    epoch smallint not null,
    nr smallint not null,
    slot bigint default null unique,
    scheduled_at timestamp with time zone default null,
    created_at timestamp not null default now()
);
```

### Helper scripts

The primary helper retains environment-overridable variable names shown above, plus optional `leaderPromFile`, `mailLeaderLogTo`, PostgreSQL variables and binary overrides. Empty `jsonPoolTool`/`slotsCsvFile` disables that optional output. Flags are `--current`, `--next`, `--test`, `--force-email`, `--csv PATH`, and `--postgres`; missing CSV paths and command failures exit nonzero. Keep the timer rather than running duplicate root cron jobs.

For manual use, load your reviewed environment and run as the dedicated leaderlog account; do not run as root. The primary helper preserves timeout/locking and replaces CSV only after successful leaderlog/JSON extraction.

`cncli-sendslots.sh` accepts `CNCLI_BIN`, `CNCLI_DB`, `CNCLI_BYRON_GENESIS`, `CNCLI_SHELLEY_GENESIS`, `CNCLI_POOLTOOL_CONFIG`, `CNCLI_LOG_DIR`, `CNCLI_LOCK_FILE`, and `JQ_BIN`. Defaults match the paths above, with log directory `/var/log/cncli-leaderlog`, lock `$CNCLI_LOG_DIR/sendslots.lock` and jq `jq`. Data/config/log/lock paths must be absolute. It checks a successful database status before sending and checks sendslots' exit status, not text content. Failed sends retain `sendslots.failed.<UTC timestamp>.<unique suffix>.log` and preserve the previous `sendslots.log`; successful sends rotate it and replace it. Only successful rotations older than 15 days are pruned inside that absolute log directory. Failure logs are never pruned automatically.

The tracked current/next/prev wrappers retain ledger selection and presentation. Configure `CNCLI_POOL_ID`, `CNCLI_VRF_SKEY`, `CNCLI_BYRON_GENESIS`, `CNCLI_SHELLEY_GENESIS`, and `CARDANO_NODE_SOCKET_PATH` with operator values. Overrides are `CNCLI_BIN`, `CARDANO_CLI_BIN`, `JQ_BIN`, `CNCLI_DB`, `CNCLI_NODE_HOST`, and `CNCLI_NODE_PORT`. They stop on snapshot, sync or leaderlog failure; they do not turn a failed command into a plausible schedule.
