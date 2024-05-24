# CNCLI Usage

## Commands & Examples

### Ping Command

This command validates that the remote server is on the given network and returns its response time.

#### Show Ping Help

```bash
$ cncli ping --help
cncli-ping 6.0.0

USAGE:
    cncli ping [OPTIONS] --host <host>

FLAGS:
        --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -h, --host <host>                          cardano-node hostname to connect to
        --network-magic <network-magic>        network magic. [default: 764824073]
    -p, --port <port>                          cardano-node port [default: 3001]
    -t, --timeout-seconds <timeout-seconds>    connect timeout in seconds [default: 2]
```

#### Example Mainnet ping using defaults

```bash
$ cncli ping --host backbone.cardano-mainnet.iohk.io
```

##### Ping Success Result

```bash
{
  "status": "ok",
  "host": "backbone.cardano-mainnet.iohk.io",
  "port": 3001,
  "networkProtocolVersion": 11,
  "dnsDurationMs": 33,
  "connectDurationMs": 131,
  "handshakeDurationMs": 130,
  "durationMs": 296
}
```

#### Example Mainnet ping timeout failure

```bash
$ cncli ping --host backbone.cardano-mainnet.iohk.io --port 9999
```

##### Ping Failure Result

```bash
{
  "status": "error",
  "host": "backbone.cardano-mainnet.iohk.io",
  "port": 9999,
  "errorMessage": "connect timeout"
}
```

#### Example ping to testnet node with mainnet magic failure

```bash
$ cncli ping --host preprod-node.play.dev.cardano.org
```

##### Ping Magic Failure Result

```bash
{
  "status": "error",
  "host": "preprod-node.play.dev.cardano.org",
  "port": 3001,
  "errorMessage": "Refused(11, \"version data mismatch: NodeToNodeVersionData {networkMagic = NetworkMagic {unNetworkMagic = 1}, diffusionMode = InitiatorAndResponderDiffusionMode, peerSharing = PeerSharingDisabled, query = False} /= NodeToNodeVersionData {networkMagic = NetworkMagic {unNetworkMagic = 764824073}, diffusionMode = InitiatorAndResponderDiffusionMode, peerSharing = PeerSharingDisabled, query = False}\")"
}
```

#### Example ping to testnet success

```bash
$ cncli ping --host preprod-node.play.dev.cardano.org --port 3001 --network-magic 1
```

##### Ping Testnet Success Result

```bash
{
  "status": "ok",
  "host": "preprod-node.play.dev.cardano.org",
  "port": 3001,
  "networkProtocolVersion": 11,
  "dnsDurationMs": 2,
  "connectDurationMs": 47,
  "handshakeDurationMs": 57,
  "durationMs": 107
}
```

### Sync Command

This command connects to a remote node and synchronizes blocks to a local sqlite database. The ```validate``` and ```leaderlog``` commands require a synchronized database.

**Note**: to setup ```cncli sync``` as a ```systemd``` service, please refer to the [installation guide](INSTALL.md). When enabled as ```systemd``` service, ```sync``` will continuously keep the ```cncli.db``` database synchronized.

#### Show Sync Help

```bash
$ cncli sync --help
cncli-sync 6.0.0

USAGE:
    cncli sync [FLAGS] [OPTIONS] --host <host>

FLAGS:
        --help          Prints help information
        --no-service    Exit at 100% sync'd.
    -V, --version       Prints version information

OPTIONS:
    -d, --db <db>                                        sqlite database file [default: ./cncli.db]
    -h, --host <host>                                    cardano-node hostname to connect to
        --network-magic <network-magic>                  network magic. [default: 764824073]
    -p, --port <port>                                    cardano-node port [default: 3001]
    -s, --shelley-genesis-hash <shelley-genesis-hash>
            shelley genesis hash value [default: 1a3be38bcbb7911969283716ad7aa550250226b76a61fc51cc9a9a35d9276d81]
```

#### Example sync command

```bash
$ cncli sync --host backbone.cardano-mainnet.iohk.io
```

##### Sync Result

```bash
 2024-01-04T17:21:11.298Z INFO  cncli::nodeclient::sync > get_intersect_blocks took: 308.175µs
 2024-01-04T17:21:15.805Z INFO  cncli::nodeclient::sync > block 9762075 of 9762081:  99.99% sync'd
 2024-01-04T17:21:16.811Z INFO  cncli::nodeclient::sync > block 9762081 of 9762081: 100.00% sync'd
 2024-01-04T17:22:24.287Z INFO  cncli::nodeclient::sync > block 9762082 of 9762082: 100.00% sync'd
 2024-01-04T17:22:38.313Z INFO  cncli::nodeclient::sync > block 9762083 of 9762083: 100.00% sync'd
```

### Status Command

This simple command gives you an ok if the database is fully synced. It will return a status of error if not.

#### Show Status Help

```bash
$ cncli status --help
cncli-status 6.0.0

USAGE:
    cncli status [OPTIONS] --byron-genesis <byron-genesis> --shelley-genesis <shelley-genesis>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --byron-genesis <byron-genesis>                          byron genesis json file
    -d, --db <db>                                                sqlite database file [default: ./cncli.db]
        --shelley-genesis <shelley-genesis>                      shelley genesis json file
        --shelley-transition-epoch <shelley-transition-epoch>
            Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files [env:
            SHELLEY_TRANS_EPOCH=]  [default: -1]
```

#### Status when fully synced

```bash
$ cncli status --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json
```

##### Fully Synced Result

```bash
{
 "status": "ok"
}
```

#### Status when not fully synced

```bash
$ cncli status --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json --db dummy.db
```

##### Not In Sync Result

```bash
{
 "status": "error",
 "errorMessage": "db not fully synced!"
}
```

### Validate Command

This command validates that a block hash or partial block hash is on-chain. You must run ```sync``` command separately to build up the database and have it sync to 100%.

#### Show Validate Help

```bash
$ cncli validate --help
cncli-validate 6.0.0

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

```bash
$ cncli validate --hash 0c4b73
```

##### Validate Success Result

```bash
{
 "status": "ok",
 "block_number": "4891104",
 "slot_number": "12597768",
 "pool_id": "89af15a1c2b8e379aa8ca6d4d9b0134e373122f84f6f45fac2e26c47",
 "hash": "0c4b730183ab2533d423f9af56ed99efd8121f716f82aa95caa3e6c11f10dc8d",
 "prev_hash": "2142685e0912f1956c99551431270c1e199b85cde57fe56554d23ce111504fe9",
 "leader_vrf": "000111925d12aea26b1705ef244fe8930f437be294180b418fba47ebf386e73d5ec7bbd397df5ba44d085171a66266089fba10a089442e207d7ad730849f9293"
}
```

#### Validate block orphaned

```bash
$ cncli validate --hash ab7095
```

##### Validate Orphaned Result

```bash
{
 "status": "orphaned",
 "block_number": "9762067",
 "slot_number": "112822212",
 "pool_id": "ec736597797c68044b8fccd4e895929c0a842f2e9e0a9e221b0a3026",
 "hash": "ab70958f10aac7399453a257b00377dd64615d36544d9a4c44abacc1ac66bf4f",
 "prev_hash": "b84c068276492628bb373f0d1a67a55675f80e692a3767fbffaccc2fd08757e4",
 "leader_vrf": "000130f59c1a9ed0129abea4ba2c1a8a175f0259ce94ef77efa2fc2724638202"
}
```

#### Validate block missing

```bash
$ cncli validate --hash ba53ba11
```

##### Validate Missing Result

```bash
{
 "status": "error",
 "errorMessage": "Query returned no rows"
}
```

### Nonce Command

This command calculates the epoch nonce value. This command requires that you use the ```sync``` command above to build a 100% synchronized ```cncli.db``` database file.

#### Show Nonce Help

```bash
$ cncli nonce --help
cncli-nonce 6.2.0

USAGE:
    cncli nonce [OPTIONS] --byron-genesis <byron-genesis> --shelley-genesis <shelley-genesis>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --byron-genesis <byron-genesis>                          byron genesis json file
    -c, --consensus <consensus>
            Consensus algorithm - Alonzo and earlier uses tpraos, Babbage uses praos, Conway uses cpraos [default:
            praos]
    -d, --db <db>                                                sqlite database file [default: ./cncli.db]
        --epoch <epoch>
            Provide a specific epoch number to calculate for and ignore --ledger-set option

        --extra-entropy <extra-entropy>                          hex string of the extra entropy value
        --ledger-set <ledger-set>
            Which ledger data to use. prev - previous epoch, current - current epoch, next - future epoch [default:
            current]
        --shelley-genesis <shelley-genesis>                      shelley genesis json file
        --shelley-transition-epoch <shelley-transition-epoch>
            Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files [env:
            SHELLEY_TRANS_EPOCH=]  [default: -1]
```

#### Calculate nonce

```bash
$ cncli nonce --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json --ledger-set next
```

##### Nonce Result

```bash
4c80bbb4bbec29a7e828dd0727e6a76878ab031b5abb4aa02c78287b1523f4e5
```

### Leaderlog Command

This command calculates a stake pool's expected slot list. ```prev``` and ```current``` logs are available as long as you have a synchronized database. ```next``` logs are only available 1.5 days before the end of the epoch. You need to use ```.poolStakeMark``` and ```.activeStakeMark``` for ```next```, ```.poolStakeSet``` and ```.activeStakeSet``` for ```current```, ```.poolStakeGo``` and ```.activeStakeGo``` for ```prev```.

Example usage with the ```stake-snapshot```:

```bash
echo "BCSH"
SNAPSHOT=$(/home/westbam/.local/bin/cardano-cli query stake-snapshot --stake-pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --mainnet)
/home/westbam/.cargo/bin/cncli sync --host 127.0.0.1 --port 6000 --no-service
POOL_STAKE=$(echo "$SNAPSHOT" | grep -oP '(?<=    "poolStakeMark": )\d+(?=,?)')
ACTIVE_STAKE=$(echo "$SNAPSHOT" | grep -oP '(?<=    "activeStakeMark": )\d+(?=,?)')
BCSH=`/home/westbam/.cargo/bin/cncli leaderlog --pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --pool-vrf-skey ./bcsh.vrf.skey --byron-genesis /home/westbam/haskell/local/byron-genesis.json --shelley-genesis /home/westbam/haskell/local/shelley-genesis.json --pool-stake $POOL_STAKE --active-stake $ACTIVE_STAKE --consensus praos --ledger-set next`

EPOCH=`jq .epoch <<< $BCSH`
echo "\`Epoch $EPOCH\` 🧙🔮:"

SLOTS=`jq .epochSlots <<< $BCSH`
IDEAL=`jq .epochSlotsIdeal <<< $BCSH`
PERFORMANCE=`jq .maxPerformance <<< $BCSH`
echo "\`BCSH  - $SLOTS \`🎰\`,  $PERFORMANCE% \`🍀max, \`$IDEAL\` 🧱ideal"
```

**Note**: to automate calculating your assigned slots and sending them to [PoolTool](https://pooltool.io/), please refer to the [installation guide](INSTALL.md).

#### Show Leaderlog Help

```bash
$ cncli leaderlog --help
cncli-leaderlog 6.2.0

USAGE:
    cncli leaderlog [OPTIONS] --active-stake <active-stake> --byron-genesis <byron-genesis> --pool-id <pool-id> --pool-stake <pool-stake> --pool-vrf-skey <pool-vrf-skey> --shelley-genesis <shelley-genesis>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --active-stake <active-stake>                            total active stake snapshot value in lovelace
        --byron-genesis <byron-genesis>                          byron genesis json file
    -c, --consensus <consensus>
            Consensus algorithm - Alonzo and earlier uses tpraos, Babbage uses praos, Conway uses cpraos [default:
            praos]
        --d <d>                                                  decentralization parameter [default: 0]
    -d, --db <db>                                                sqlite database file [default: ./cncli.db]
        --epoch <epoch>
            Provide a specific epoch number to calculate for and ignore --ledger-set option

        --extra-entropy <extra-entropy>                          hex string of the extra entropy value
        --ledger-set <ledger-set>
            Which ledger data to use. prev - previous epoch, current - current epoch, next - future epoch [default:
            current]
        --nonce <nonce>
            Provide a nonce value in lower-case hex instead of calculating from the db

        --pool-id <pool-id>                                      lower-case hex pool id
        --pool-stake <pool-stake>                                pool active stake snapshot value in lovelace
        --pool-vrf-skey <pool-vrf-skey>                          pool's vrf.skey file
        --shelley-genesis <shelley-genesis>                      shelley genesis json file
        --shelley-transition-epoch <shelley-transition-epoch>
            Epoch number where we transition from Byron to Shelley. -1 means guess based on genesis files [env:
            SHELLEY_TRANS_EPOCH=]  [default: -1]
        --tz <timezone>
            TimeZone string from the IANA database - https://en.wikipedia.org/wiki/List_of_tz_database_time_zones
            [default: America/Los_Angeles]
```

#### Calculate leaderlog

```bash
$ cncli leaderlog --pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --pool-vrf-skey ./bcsh.vrf.skey --byron-genesis /home/westbam/haskell/local/byron-genesis.json --shelley-genesis /home/westbam/haskell/local/shelley-genesis.json --pool-stake $POOL_STAKE --active-stake $ACTIVE_STAKE --consensus tpraos --ledger-set current
```

##### Leaderlog Success Result

```bash
{
  "status": "ok",
  "epoch": 227,
  "epochNonce": "0e534dd41bb80bfff4a16d038eb52280e9beac7545cc32c9bfc253a6d92010d1",
  "poolId": "00beef284975ef87856c1343f6bf50172253177fdebc756524d43fc1",
  "sigma": 0.0028306163817569175,
  "d": 0,
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

#### Calculate leaderlog failure (too soon for "next" logs, or un-synchronized database)

```bash
$ cncli leaderlog --pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --pool-vrf-skey ./bcsh.vrf.skey --byron-genesis /home/westbam/haskell/local/byron-genesis.json --shelley-genesis /home/westbam/haskell/local/shelley-genesis.json --pool-stake $POOL_STAKE --active-stake $ACTIVE_STAKE --ledger-set next
```

##### Leaderlog Too Soon Result

```bash
{
 "status": "error",
 "errorMessage": "Query returned no rows"
}
```

### Sendtip command

The sendtip command is used to communicate with [pooltool.io](https://pooltool.io) so you can have a green badge on their website with your current tip height.

![pooltool tip image](images/pooltool_sendtip.png)

It is important to point this command at your core nodes. This will help pooltool capture any orphan blocks. There is no guarantee that an orphan block you make will be seen by pooltool. Pointing to your core nodes should help with that.

**Note**: to setup ```cncli sendtip``` as a ```systemd``` service, please refer to the [installation guide](INSTALL.md). When enabled as ```systemd``` service, ```sendtip``` will continuously send your stake pool ```tip``` to PoolTool.


#### Sendtip help

```bash
$ cncli sendtip --help
cncli-sendtip 6.0.0

USAGE:
    cncli sendtip [OPTIONS] --cardano-node <cardano-node>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --cardano-node <cardano-node>    path to cardano-node executable for gathering version info
        --config <config>                pooltool config file for sending tips [default: ./pooltool.json]
```

#### Configuring pooltool.json

You need to create a pooltool.json file so that the sendtip command knows what node(s) to connect to. It also contains your pooltool configuration. Your pooltool api key can be found on your pooltool profile page.

```json
{
  "api_key": "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX",
  "pools": [
    ...
      {
          "name": "TCKR",
          "pool_id": "a7398d649be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
          "host" : "123.123.123.12",
          "port": 3001
      },
      {
          "name": "TCKR1",
          "pool_id": "b73d891285526062d41cd7293746048c6a9a13ab8b591920cf40c706",
          "host" : "123.123.123.35",
          "port": 3001
      },
    ...
  ]
}
```

#### Sending tips to pooltool

```bash
$ cncli sendtip --cardano-node /usr/local/bin/cardano-node --config /root/scripts/pooltool.json
```

##### Sending Tip Result

```bash
2020-11-08T18:37:52.323Z INFO  cncli::nodeclient::protocols::mux_protocol > Connecting to 123.123.123.12:3001
2020-11-08T18:37:52.358Z WARN  cncli::nodeclient::protocols::transaction_protocol > TxSubmissionProtocol::State::Done
2020-11-08T18:37:52.358Z WARN  cncli::nodeclient::protocols::chainsync_protocol   > rollback to slot: 4492799
2020-11-08T18:37:52.359Z WARN  cncli::nodeclient::protocols::chainsync_protocol   > rollback to slot: 13294373
2020-11-08T18:37:54.402Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > Pooltool (TCKR, a7398d64): (4925270, 4d65b09dc1d5c6c2), json: {"success":true,"message":null}
2020-11-08T18:38:36.323Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > Pooltool (TCKR, a7398d64): (4925271, 47d6beb189f24c9e), json: {"success":true,"message":null}
2020-11-08T18:38:37.424Z INFO  cncli::nodeclient::protocols::chainsync_protocol   > Pooltool (TCKR, a7398d64): (4925272, defe4ba88985c305), json: {"success":true,"message":null}
 ...
 ...
```

### Sendslots command

The sendslots command securely sends pooltool the number of slots you have assigned for an epoch and validates the correctness of your past epochs. You must have a synchronized ```cncli.db``` database and have calculated leader logs for every pool in ```pooltool.json``` before calling this command. It should be called within the first 10 minutes of the epoch cutover.

**Note**: to automate sending your pool assigned slots to [PoolTool](https://pooltool.io/), please refer to the [installation guide](INSTALL.md).

#### Sendslots help

```bash
$ cncli sendslots --help
cncli-sendslots 6.0.0

USAGE:
    cncli sendslots [OPTIONS] --byron-genesis <byron-genesis> --shelley-genesis <shelley-genesis>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --byron-genesis <byron-genesis>        byron genesis json file
        --config <config>                      pooltool config file for sending slots [default: ./pooltool.json]
    -d, --db <db>                              sqlite database file [default: ./cncli.db]
        --shelley-genesis <shelley-genesis>    shelley genesis json file
```

#### Sendslots Success

```bash
$ cncli sendslots --byron-genesis ~/haskell/local/byron-genesis.json --shelley-genesis ~/haskell/local/shelley-genesis.json
```

##### Sendslots success Result

```text
2020-12-01T03:34:33.883Z INFO  cncli::nodeclient::leaderlog > Sending: {"apiKey":"d67822d0-0008-4eb5-9e1e-9c30bdb8d82d","poolId":"00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114","epoch":232,"slotQty":25,"hash":"d15b6c8d4c81fe48cff0650c5b59ab20da9765374c58c933dacd058eb38bb670"}
2020-12-01T03:34:33.969Z INFO  cncli::nodeclient::leaderlog > Pooltool Response: {"statusCode":200,"headers":{"Content-Type":"application/json","Access-Control-Allow-Origin":"*"},"body":"{\"success\":true,\"message\":\"We have updated your assigned slots for epoch 232 to be 25 with a hash of d15b6c8d4c81fe48cff0650c5b59ab20da9765374c58c933dacd058eb38bb670.  You must provide an array of slots that matches this hash to have your performance counted.\"}"}
2020-12-01T03:34:33.971Z INFO  cncli::nodeclient::leaderlog > Sending: {"apiKey":"d67822d0-0008-4eb5-9e1e-9c30bdb8d82d","poolId":"00beef8710427e328a29555283c74b202b40bec9a62630a9f03b1e18","epoch":232,"slotQty":24,"hash":"97655646efcfe8a569508d70e6fc46135488fc5600bb95233c3f005106a7f5a3"}
2020-12-01T03:34:34.051Z INFO  cncli::nodeclient::leaderlog > Pooltool Response: {"statusCode":200,"headers":{"Content-Type":"application/json","Access-Control-Allow-Origin":"*"},"body":"{\"success\":true,\"message\":\"We have updated your assigned slots for epoch 232 to be 24 with a hash of 97655646efcfe8a569508d70e6fc46135488fc5600bb95233c3f005106a7f5a3.  You must provide an array of slots that matches this hash to have your performance counted.\"}"}
2020-12-01T03:34:34.053Z INFO  cncli::nodeclient::leaderlog > Sending: {"apiKey":"d67822d0-0008-4eb5-9e1e-9c30bdb8d82d","poolId":"00beef9385526062d41cd7293746048c6a9a13ab8b591920cf40c706","epoch":232,"slotQty":54,"hash":"f12dff6eb3786d04cb2d7f666e92876faa7d5f2a26de77d3affc1aaffa6d81a5"}
2020-12-01T03:34:34.149Z INFO  cncli::nodeclient::leaderlog > Pooltool Response: {"statusCode":200,"headers":{"Content-Type":"application/json","Access-Control-Allow-Origin":"*"},"body":"{\"success\":true,\"message\":\"We have updated your assigned slots for epoch 232 to be 54 with a hash of f12dff6eb3786d04cb2d7f666e92876faa7d5f2a26de77d3affc1aaffa6d81a5.  You must provide an array of slots that matches this hash to have your performance counted.\"}"}
2020-12-01T03:34:34.150Z INFO  cncli::nodeclient::leaderlog > Sending: {"apiKey":"d67822d0-0008-4eb5-9e1e-9c30bdb8d82d","poolId":"00beef284975ef87856c1343f6bf50172253177fdebc756524d43fc1","epoch":232,"slotQty":42,"hash":"30c92d028c99af5ca51dd58293a575b14671d56cd6c846bd1c21126a2addd9ac"}
2020-12-01T03:34:34.222Z INFO  cncli::nodeclient::leaderlog > Pooltool Response: {"statusCode":200,"headers":{"Content-Type":"application/json","Access-Control-Allow-Origin":"*"},"body":"{\"success\":true,\"message\":\"We have updated your assigned slots for epoch 232 to be 42 with a hash of 30c92d028c99af5ca51dd58293a575b14671d56cd6c846bd1c21126a2addd9ac.  You must provide an array of slots that matches this hash to have your performance counted.\"}"}
```

### Sign Command

This command signs an arbitrary message string with the pool's vrf.skey. The output signature can be used to verify that the message came from the pool operator.

#### Show Sign Help

```bash
$ cncli sign --help
cncli-sign 6.0.0

USAGE:
    cncli sign --message <message> --pool-vrf-skey <pool-vrf-skey>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --message <message>                text message to sign
        --pool-vrf-skey <pool-vrf-skey>    pool's vrf.skey file
```

#### Sign a message

```bash
$ cncli sign --message "pooltool.io" --pool-vrf-skey pool.vrf.skey
```

##### Sign Result

```bash
{
  "status": "ok",
  "signature": "8aff63e961aad02852dbb7905f9215d7b1d4ff63f734f7f1b82184004112cca798719941ccf54beca360f844632c2c070e6f8ef11ca177efa240c712ef3d7e9f283db68278088acbe1af381cc9673e08"
}
```

### Verify Command

This command verifies the signature that was used to sign an arbitrary message string with the pool's vrf.skey. This command validates that the message came from the pool operator.

#### Show Verify Help

```bash
$ cncli verify --help
cncli-verify 6.0.0

USAGE:
    cncli verify --message <message> --pool-vrf-vkey <pool-vrf-vkey> --pool-vrf-vkey-hash <pool-vrf-vkey-hash> --signature <signature>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --message <message>                          text message to verify
        --pool-vrf-vkey <pool-vrf-vkey>              pool's vrf.vkey file
        --pool-vrf-vkey-hash <pool-vrf-vkey-hash>
            pool's vrf hash in hex retrieved from 'cardano-cli query pool-params...'

        --signature <signature>                      signature to verify in hex
```

#### Verify a message

```bash
$ cncli verify --message "pooltool.io" --pool-vrf-vkey pool.vrf.vkey --pool-vrf-vkey-hash f58bf0111f8e9b233c2dcbb72b5ad400330cf260c6fb556eb30cefd387e5364c --signature 8aff63e961aad02852dbb7905f9215d7b1d4ff63f734f7f1b82184004112cca798719941ccf54beca360f844632c2c070e6f8ef11ca177efa240c712ef3d7e9f283db68278088acbe1af381cc9673e08
```

##### Verify Result

```bash
{
  "status": "ok"
}
```
