#!/usr/bin/env bash
set -Eeuo pipefail

CNCLI_BIN="${CNCLI_BIN:-/usr/local/bin/cncli}"
CNCLI_DB="${CNCLI_DB:-/var/lib/cncli/cncli.db}"
CNCLI_BYRON_GENESIS="${CNCLI_BYRON_GENESIS:-/etc/cncli/mainnet-byron-genesis.json}"
CNCLI_SHELLEY_GENESIS="${CNCLI_SHELLEY_GENESIS:-/etc/cncli/mainnet-shelley-genesis.json}"
CNCLI_POOLTOOL_CONFIG="${CNCLI_POOLTOOL_CONFIG:-/etc/cncli/pooltool.json}"
CNCLI_LOG_DIR="${CNCLI_LOG_DIR:-/var/log/cncli-leaderlog}"
CNCLI_LOCK_FILE="${CNCLI_LOCK_FILE:-$CNCLI_LOG_DIR/sendslots.lock}"
JQ_BIN="${JQ_BIN:-jq}"

for path in "$CNCLI_DB" "$CNCLI_BYRON_GENESIS" "$CNCLI_SHELLEY_GENESIS" "$CNCLI_POOLTOOL_CONFIG" "$CNCLI_LOG_DIR" "$CNCLI_LOCK_FILE"; do
    [[ "$path" == /* ]] || { echo "Data, config, log and lock paths must be absolute: $path" >&2; exit 1; }
done
mkdir -p "$CNCLI_LOG_DIR"
exec {lock_fd}>"$CNCLI_LOCK_FILE"
flock -n "$lock_fd" || { echo "Another sendslots instance holds the lock" >&2; exit 1; }
status_json=$("$CNCLI_BIN" status --db "$CNCLI_DB" --byron-genesis "$CNCLI_BYRON_GENESIS" --shelley-genesis "$CNCLI_SHELLEY_GENESIS")
"$JQ_BIN" -e '.status == "ok"' <<<"$status_json" >/dev/null || { echo "CNCLI database not synced" >&2; exit 1; }

output=$(mktemp "$CNCLI_LOG_DIR/.sendslots.XXXXXXXX")
trap '[[ ! -e "$output" ]] || rm -f -- "$output"' EXIT
suffix="$(date -u +%Y%m%dT%H%M%SZ).${output##*.}"
if "$CNCLI_BIN" sendslots --db "$CNCLI_DB" --byron-genesis "$CNCLI_BYRON_GENESIS" --shelley-genesis "$CNCLI_SHELLEY_GENESIS" --config "$CNCLI_POOLTOOL_CONFIG" >"$output" 2>&1; then
    if [[ -e "$CNCLI_LOG_DIR/sendslots.log" ]]; then
        mv -- "$CNCLI_LOG_DIR/sendslots.log" "$CNCLI_LOG_DIR/sendslots.$suffix.log"
    fi
    mv -- "$output" "$CNCLI_LOG_DIR/sendslots.log"
    find "$CNCLI_LOG_DIR" -maxdepth 1 -type f -name 'sendslots.[0-9]*.log' -mtime +15 -delete
else
    result=$?
    mv -- "$output" "$CNCLI_LOG_DIR/sendslots.failed.$suffix.log"
    echo "cncli sendslots failed; retained sendslots.failed.$suffix.log" >&2
    exit "$result"
fi
