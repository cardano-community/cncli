# cncli
A community-based cardano-node CLI tool

## Building
#### Prepare RUST environment
```shell script
$ mkdir $HOME/.cargo && mkdir $HOME/.cargo/bin
$ chown -R $USER $HOME/.cargo
$ touch $HOME/.profile
$ chown $USER $HOME/.profile
```
#### Install rustup - proceed with default install (option 1)
```shell script
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# To configure your current shell run:
$ source $HOME/.cargo/env

$ rustup install stable
$ rustup default stable
$ rustup update  
```
#### Install dependencies and build cncli
```shell script
$ sudo apt-get install libsqlite3-dev
$ git clone https://github.com/AndrewWestberg/cncli
$ cd cncli
$ git checkout <latest_tag_name>
$ cargo install --path . --force      
$ cncli -V
```

## Updating cncli from earlier versions
```shell script
$ rustup update
$ cd cncli
$ git fetch --all --prune
$ git checkout <latest_tag_name>
$ cargo install --path . --force
```

## Running

### Ping Command
This command validates that the remote server is on the given network and returns its response time.
#### Show Help
```shell script
$ cncli ping --help
cncli-ping 0.1.0

USAGE:
    cncli ping [OPTIONS] --host <host>

FLAGS:
        --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -h, --host <host>                      cardano-node hostname to connect to
        --network-magic <network-magic>    network magic. [default: 764824073]
    -p, --port <port>                      cardano-node port [default: 3001]
```
#### Example Mainnet ping using defaults
```shell script
$ cncli ping --host north-america.relays-new.cardano-mainnet.iohk.io                                 
{
 "status": "ok",
 "host": "north-america.relays-new.cardano-mainnet.iohk.io",
 "port": 3001,
 "durationMs": 118
}
```
#### Example Mainnet ping timeout failure
```shell script
$ cncli ping --host north-america.relays-new.cardano-mainnet.iohk.io --port 9999
{
 "status": "error",
 "host": "north-america.relays-new.cardano-mainnet.iohk.io",
 "port": 9999,
 "errorMessage": "Failed to connect: connection timed out"
}
```
#### Example ping to testnet node with mainnet magic failure
```shell script
$ cncli ping --host north-america.relays-new.cardano-testnet.iohkdev.io         
{
 "status": "error",
 "host": "north-america.relays-new.cardano-testnet.iohkdev.io",
 "port": 3001,
 "errorMessage": "version data mismatch: NodeToNodeVersionData {networkMagic = NetworkMagic {unNetworkMagic = 1097911063}, diffusionMode = InitiatorAndResponderDiffusionMode} /= NodeToNodeVersionData {networkMagic = NetworkMagic {unNetworkMagic = 764824073}, diffusionMode = InitiatorAndResponderDiffusionMode}"
}
```
#### Example ping to testnet success
```shell script
$ cncli ping --host north-america.relays-new.cardano-testnet.iohkdev.io --port 3001 --network-magic 1097911063
{
 "status": "ok",
 "host": "north-america.relays-new.cardano-testnet.iohkdev.io",
 "port": 3001,
 "durationMs": 38
}
```

### Sync Command
This command connects to a remote node and synchronizes blocks to a local sqlite database. The `validate` and `leaderlog` commands require a synchronized database.
#### Show Help
```shell script
$ cncli sync --help
cncli-sync 0.1.0

USAGE:
    cncli sync [OPTIONS] --host <host>

FLAGS:
        --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --db <db>                          sqlite database file [default: ./cncli.db]
    -h, --host <host>                      cardano-node hostname to connect to
        --network-magic <network-magic>    network magic. [default: 764824073]
    -p, --port <port>                      cardano-node port [default: 3001]
```

#### Example sync command
```shell script
$ cncli sync --host 127.0.0.1 --port 6000
 2020-10-31T16:55:35.025Z INFO  cncli::nodeclient > Starting NodeClient...
 2020-10-31T16:55:35.025Z INFO  cncli::nodeclient::protocols::mux_protocol > Connecting to 127.0.0.1:6000 ...
 2020-10-31T16:55:35.030Z WARN  cncli::nodeclient::protocols::handshake_protocol > HandshakeProtocol::State::Done
 2020-10-31T16:55:35.110Z WARN  cncli::nodeclient::protocols::transaction_protocol > TxSubmissionProtocol::State::Done
 2020-10-31T16:55:35.110Z WARN  cncli::nodeclient::protocols::chainsync_protocol   > rollback to slot: 4492799
 2020-10-31T16:55:35.114Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4490511 of 4891060, 91.81% synced
 2020-10-31T16:55:40.646Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4519089 of 4891061, 92.39% synced
 2020-10-31T16:55:46.341Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4544646 of 4891061, 92.92% synced
 2020-10-31T16:55:52.012Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4567647 of 4891061, 93.39% synced
 2020-10-31T16:55:57.815Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4594692 of 4891062, 93.94% synced
 2020-10-31T16:56:03.793Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4624024 of 4891063, 94.54% synced
 2020-10-31T16:56:09.814Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4653024 of 4891063, 95.13% synced
 2020-10-31T16:56:15.808Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4678390 of 4891063, 95.65% synced
 2020-10-31T16:56:21.856Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4704799 of 4891063, 96.19% synced
 2020-10-31T16:56:27.887Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4730288 of 4891063, 96.71% synced
 2020-10-31T16:56:34.167Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4756308 of 4891063, 97.24% synced
 2020-10-31T16:56:40.340Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4782723 of 4891064, 97.78% synced
 2020-10-31T16:56:46.448Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4806428 of 4891064, 98.27% synced
 2020-10-31T16:56:52.675Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4831364 of 4891064, 98.78% synced
 2020-10-31T16:56:59.101Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4863279 of 4891065, 99.43% synced
 2020-10-31T16:57:05.576Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4889661 of 4891065, 99.97% synced
 2020-10-31T16:57:17.958Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4891066 of 4891066, 100.00% synced
 2020-10-31T16:57:30.927Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > block 4891067 of 4891067, 100.00% synced
```

### Validate Command
This command validates that a block hash or partial block hash is on-chain. You must run `sync` command separately to build up the database and have it sync to 100%.
#### Show Help
```shell script
$ cncli validate --help
cncli-validate 0.1.0

USAGE:
    cncli validate [OPTIONS] --hash <hash>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --db <db>        sqlite database file [default: ./cncli.db]
        --hash <hash>    full or partial block hash to validate
```

#### Validate block success
```shell script
$ cncli validate --hash 0c4b73
{
 "status": "ok",
 "block_number": "4891104",
 "slot_number": "12597768",
 "hash": "0c4b730183ab2533d423f9af56ed99efd8121f716f82aa95caa3e6c11f10dc8d",
 "prev_hash": "2142685e0912f1956c99551431270c1e199b85cde57fe56554d23ce111504fe9",
 "leader_vrf": "000111925d12aea26b1705ef244fe8930f437be294180b418fba47ebf386e73d5ec7bbd397df5ba44d085171a66266089fba10a089442e207d7ad730849f9293"
}
```

#### Validate block orphaned
```shell script
$ cncli validate --hash af6d8e
{
 "status": "orphaned",
 "block_number": "4891104",
 "slot_number": "12597768",
 "hash": "af6d8e8a21bd65b6542fecc51da82e59824ad51c43fb2bbc0dcd0c8f20f2adae",
 "prev_hash": "2142685e0912f1956c99551431270c1e199b85cde57fe56554d23ce111504fe9",
 "leader_vrf": "000c6abd406175af91def3c225fb758370d26e506275a9574eb88ebb886490f3a4a6d971c822193bb3a186b8c3d75c890f61bff09fbf7f0066b152a2707f9929"
}
```

#### Validate block missing
```shell script
$ cncli validate --hash ffffff
{
 "status": "error",
 "errorMessage": "Query returned no rows"
}
```

### Leaderlog Command
This command calculates a stakepool's expected slot list. "prev" and "current" logs are available as long as you have a sync'd database. "next" logs are only available 1.5 days before the end of the epoch.

This command requires that you:

1.) use cardano-cli to dump a fresh ledger-state.json file 
```shell script
$ cardano-cli shelley query ledger-state --cardano-mode --mainnet > /tmp/ledger-state-227.json
```
2.) Use the `sync` command above to build a 100% sync'd cncli.db database file.

#### Show Help
```shell script
$ cncli leaderlog --help
cncli-leaderlog 0.1.0

USAGE:
    cncli leaderlog [OPTIONS] --byron-genesis <byron-genesis> --ledger-state <ledger-state> --pool-id <pool-id> --pool-vrf-skey <pool-vrf-skey> --shelley-genesis <shelley-genesis>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --byron-genesis <byron-genesis>        byron genesis json file
    -d, --db <db>                              sqlite database file [default: ./cncli.db]
        --ledger-set <ledger-set>              Which ledger data to use. prev - previous epoch, current - current epoch,
                                               next - future epoch [default: current]
        --ledger-state <ledger-state>          ledger state json file
        --pool-id <pool-id>                    lower-case hex pool id
        --pool-vrf-skey <pool-vrf-skey>        pool's vrf.skey file
        --shelley-genesis <shelley-genesis>    shelley genesis json file
```

#### Calculate leaderlog
```shell script
$ cncli leaderlog --pool-id 00beef284975ef87856c1343f6bf50172253177fdebc756524d43fc1 --pool-vrf-skey ./bcsh2.vrf.skey --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json --ledger-state /tmp/ledger-state-227.json --ledger-set current
{
  "status": "ok",
  "epoch": 227,
  "epochNonce": "0e534dd41bb80bfff4a16d038eb52280e9beac7545cc32c9bfc253a6d92010d1",
  "poolId": "00beef284975ef87856c1343f6bf50172253177fdebc756524d43fc1",
  "sigma": 0.0028306163817569175,
  "d": 0.5,
  "assignedSlots": [
    ...
    {
      "slot": 13083245,
      "slotInEpoch": 382445,
      "at": "2020-11-05T23:58:56-08:00"
    },
    {
      "slot": 13106185,
      "slotInEpoch": 405385,
      "at": "2020-11-06T06:21:16-08:00"
    }
    ...
  ]
}
```

#### Calculate leaderlog failure (too soon for "next" logs, or un-sync'd database)
```shell script
$ cncli leaderlog --pool-id 00beef284975ef87856c1343f6bf50172253177fdebc756524d43fc1 --pool-vrf-skey ./bcsh2.vrf.skey --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json --ledger-state /tmp/ledger-state-227.json --ledger-set next
{
 "status": "error",
 "errorMessage": "Query returned no rows"
}
```