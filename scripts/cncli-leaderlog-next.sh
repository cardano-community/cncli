#!/usr/bin/env bash
set -Eeuo pipefail
: "${CNCLI_POOL_ID:?Set your pool ID}"
: "${CNCLI_VRF_SKEY:?Set your VRF signing key path}"
: "${CNCLI_BYRON_GENESIS:?Set your Byron genesis path}"
: "${CNCLI_SHELLEY_GENESIS:?Set your Shelley genesis path}"
: "${CARDANO_NODE_SOCKET_PATH:?Set your node socket path}"
export CARDANO_NODE_SOCKET_PATH
CNCLI_BIN="${CNCLI_BIN:-/usr/local/bin/cncli}"
CARDANO_CLI_BIN="${CARDANO_CLI_BIN:-/usr/local/bin/cardano-cli}"
JQ_BIN="${JQ_BIN:-jq}"
CNCLI_DB="${CNCLI_DB:-/var/lib/cncli/cncli.db}"
CNCLI_NODE_HOST="${CNCLI_NODE_HOST:-127.0.0.1}"
CNCLI_NODE_PORT="${CNCLI_NODE_PORT:-3000}"

echo "BCSH"
SNAPSHOT=$("$CARDANO_CLI_BIN" query stake-snapshot --stake-pool-id "$CNCLI_POOL_ID" --mainnet)
"$CNCLI_BIN" sync --db "$CNCLI_DB" --host "$CNCLI_NODE_HOST" --port "$CNCLI_NODE_PORT" --no-service
if grep -q '"pools"' <<<"$SNAPSHOT"; then
    STAKES=$(grep -oP '(?<=    "stakeMark": )\d+(?=,?)' <<<"$SNAPSHOT" | tr '\n' ' ')
    read -r POOL_STAKE ACTIVE_STAKE REST <<<"$STAKES"
else
    POOL_STAKE=$(grep -oP '(?<=    "poolStakeMark": )\d+(?=,?)' <<<"$SNAPSHOT")
    ACTIVE_STAKE=$(grep -oP '(?<=    "activeStakeMark": )\d+(?=,?)' <<<"$SNAPSHOT")
fi
[[ "$POOL_STAKE" =~ ^[0-9]+$ && "$ACTIVE_STAKE" =~ ^[0-9]+$ ]] || { echo "Invalid snapshot stake amounts" >&2; exit 1; }
BCSH=$("$CNCLI_BIN" leaderlog --db "$CNCLI_DB" --pool-id "$CNCLI_POOL_ID" --pool-vrf-skey "$CNCLI_VRF_SKEY" --byron-genesis "$CNCLI_BYRON_GENESIS" --shelley-genesis "$CNCLI_SHELLEY_GENESIS" --pool-stake "$POOL_STAKE" --active-stake "$ACTIVE_STAKE" --consensus praos --ledger-set next)
"$JQ_BIN" -e '.status == "ok"' <<<"$BCSH" >/dev/null
"$JQ_BIN" . <<<"$BCSH"
EPOCH=$("$JQ_BIN" -er '.epoch' <<<"$BCSH")
echo "\`Epoch $EPOCH\` 🧙🔮:"
SLOTS=$("$JQ_BIN" -er '.epochSlots' <<<"$BCSH")
IDEAL=$("$JQ_BIN" -er '.epochSlotsIdeal' <<<"$BCSH")
PERFORMANCE=$("$JQ_BIN" -er '.maxPerformance' <<<"$BCSH")
echo "\`BCSH  - $SLOTS \`🎰\`,  $PERFORMANCE% \`🍀max, \`$IDEAL\` 🧱ideal"
