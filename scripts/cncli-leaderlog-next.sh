#!/bin/bash
export CARDANO_NODE_SOCKET_PATH=/home/westbam/haskell/local/db/socket

echo "BCSH"
SNAPSHOT=$(/home/westbam/.local/bin/cardano-cli query stake-snapshot --stake-pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --mainnet)
/home/westbam/.cargo/bin/cncli sync --host 127.0.0.1 --port 6000 --no-service
echo $SNAPSHOT | grep -q "pools"
if [ $? -eq 0 ]; then
	STAKES=$(echo "$SNAPSHOT" | grep -oP '(?<=    "stakeMark": )\d+(?=,?)')
	POOL_STAKE=$(echo $STAKES | cut -d' ' -f1)
	ACTIVE_STAKE=$(echo $STAKES | cut -d' ' -f2)
else
	POOL_STAKE=$(echo "$SNAPSHOT" | grep -oP '(?<=    "poolStakeMark": )\d+(?=,?)')
	ACTIVE_STAKE=$(echo "$SNAPSHOT" | grep -oP '(?<=    "activeStakeMark": )\d+(?=,?)')
fi
BCSH=`/home/westbam/.cargo/bin/cncli leaderlog --pool-id 00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114 --pool-vrf-skey ./bcsh.vrf.skey --byron-genesis /home/westbam/haskell/local/byron-genesis.json --shelley-genesis /home/westbam/haskell/local/shelley-genesis.json --pool-stake $POOL_STAKE --active-stake $ACTIVE_STAKE --consensus praos --ledger-set next`
echo $BCSH | jq .

EPOCH=`echo $BCSH | jq .epoch`
echo "\`Epoch $EPOCH\` 🧙🔮:"

SLOTS=`echo $BCSH | jq .epochSlots`
IDEAL=`echo $BCSH | jq .epochSlotsIdeal`
PERFORMANCE=`echo $BCSH | jq .maxPerformance`
echo "\`BCSH  - $SLOTS \`🎰\`,  $PERFORMANCE% \`🍀max, \`$IDEAL\` 🧱ideal"
