# cncli
A community-based cardano-node CLI tool

## Building
```shell script
$ sudo apt-get install libsqlite3-dev
$ cargo install --path . --force
```

## Running

### Ping Command
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

