#!/usr/bin/env bash
set -Eeuo pipefail
#
# Send slots to PoolTool, mail leaderlog, store slots as CSV or in PostgreSQL.
#
# Depending on the day of the epoch we are in (1 to 5) this script will run one
# or more of the tasks above.
# On epoch start: send slots for the current and previous epoch to PoolTool.
# On epoch day 4: calculate next epoch leaderlog, mail it and/or write slots.csv.
#
# Usage:
#   Via systemd timer or ./cncli-leaderlog.sh [--test] [--force-email] [--csv PATH] [--postgres]
#
# Original Author: Leon • HAPPY Staking
# Improvements by Rick • RCADA Pool:    safer strict mode, logging, timeouts, VRF check, test mode, CSV atomic write

# ------------------------------------------------------------------------------
# Pool specific variables (EDIT THESE)
# ------------------------------------------------------------------------------

# export CARDANO_NODE_SOCKET_PATH="/path/to/node.socket"  # (exported by your env/service)

timezone="${timezone:-Etc/UTC}"
hexStakePool="${hexStakePool:-}"                 # REQUIRED: operator's pool id in hex
jsonPoolTool="${jsonPoolTool-/etc/cncli/pooltool.json}" # empty disables PoolTool
slotsCsvFile="${slotsCsvFile-/var/lib/cncli-leaderlog/slots.csv}"
leaderPromFile="${leaderPromFile:-}"
mailLeaderLogTo="${mailLeaderLogTo:-}"
saveToPostgres="${saveToPostgres:-none}"
useScriptLogging="${useScriptLogging:-true}"

consensusMode="${consensusMode:-cpraos}"
vrfSigningKeyFile="${vrfSigningKeyFile:-/etc/cncli/vrf.skey}"
shelleyGenesisFile="${shelleyGenesisFile:-/etc/cncli/mainnet-shelley-genesis.json}"
byronGenesisFile="${byronGenesisFile:-/etc/cncli/mainnet-byron-genesis.json}"
dbCnCli="${dbCnCli:-/var/lib/cncli/cncli.db}"
binCardanoCli="${binCardanoCli:-/usr/local/bin/cardano-cli}"
binCnCli="${binCnCli:-/usr/local/bin/cncli}"
binPython3="${binPython3:-python3}"

# ------------------------------------------------------------------------------
# PostgreSQL connection (EDIT THESE)
# ------------------------------------------------------------------------------
export PGUSER="${PGUSER:-}"
export PGHOST="${PGHOST:-}"
export PGDATABASE="${PGDATABASE:-}"
export PGPASSFILE="${PGPASSFILE:-}"

# ------------------------------------------------------------------------------
# Binaries (override via environment if needed; default to PATH)
# ------------------------------------------------------------------------------
binCardanoCli="${binCardanoCli:-cardano-cli}"
binCnCli="${binCnCli:-cncli}"
binJq="${binJq:-jq}"
binMail="${binMail:-mail}"
binTimeout="${binTimeout:-timeout}"
binPython3="${binPython3:-python3}"

# ------------------------------------------------------------------------------
# Behavior / logging
# ------------------------------------------------------------------------------
LOG_DIR="${LOG_DIR:-/var/log/cncli-leaderlog}"
LOG_FILE="${LOG_DIR}/cncli-leaderlog.log"
LOCK_FILE="${LOG_DIR}/cncli-leaderlog.lock"
CMD_TIMEOUT="${CMD_TIMEOUT:-300s}"             # default timeout for long ops
DEBUG="${DEBUG:-0}"                            # set DEBUG=1 to enable bash -x

# --- CLI flags (optional) ---
TEST="${TEST:-0}"
FORCE_EMAIL="${FORCE_EMAIL:-0}"
CSV_OVERRIDE="${CSV_OVERRIDE:-}"
POSTGRES=${POSTGRES:-0}
CURRENT=${CURRENT:-0}
NEXT=${NEXT:-0}

usage() {
  cat <<EOF
Usage: $0 [--test] [--force-email] [--csv /path/to/slots.csv] [--postgres]
  --test           Run leaderlog for CURRENT epoch now, write CSV, optionally email.
  --force-email    Force email send during --test (ignores timing windows).
  --csv PATH       Override CSV output path for this run only.
  --postgres       Test connection to PostgreSQL. Returns connection info on success.
  --current        Force calculation and processing of the current and previous epoch.
  --next           Force calculation and processing of the next epoch.

Environment equivalents:
  TEST=1 FORCE_EMAIL=1 CSV_OVERRIDE=/tmp/slots.csv POSTGRES=1 CURRENT=1 NEXT=1 $0
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test) TEST=1; shift ;;
    --force-email) FORCE_EMAIL=1; shift ;;
    --csv)
      [[ $# -ge 2 && -n "$2" && "$2" != -* ]] || { echo "--csv requires a path" >&2; exit 1; }
      CSV_OVERRIDE="$2"; shift 2 ;;
    --postgres) TEST=1; POSTGRES=1; shift ;;
    --current) CURRENT=1; shift ;;
    --next) NEXT=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown flag: $1"; usage; exit 1 ;;
  esac
done

# ------------------------------------------------------------------------------
# Strict mode + logging + error trap
# ------------------------------------------------------------------------------
if [[ "${DEBUG}" == "1" ]]; then set -x; fi
if [[ "$useScriptLogging" == "1" || "${useScriptLogging,,}" == "true" ]]; then mkdir -p "${LOG_DIR}"; fi

log() {
  # usage: log "message"
  if [[ "$useScriptLogging" == "1" || "${useScriptLogging,,}" == "true" ]];
  then
    printf '%s %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" | tee -a "${LOG_FILE}"
  else
    printf '%s %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*"
  fi
}

die() {
  log "FATAL: $*"
  exit 1
}

on_err() {
  local exit_code=$?
  local line_no=${BASH_LINENO[0]}
  log "ERROR: Script failed at line ${line_no} (exit ${exit_code}). Last command: '${BASH_COMMAND}'"
  log "Hint: check ${LOG_FILE} for full history."
  exit "${exit_code}"
}
trap on_err ERR

# Concurrency lock
if [[ "$useScriptLogging" == "1" || "${useScriptLogging,,}" == "true" ]];
then
  exec {lock_fd}>"${LOCK_FILE}" || die "Cannot open lock ${LOCK_FILE}"
  flock -n "${lock_fd}" || die "Another instance is running (lock: ${LOCK_FILE})"
fi

# ------------------------------------------------------------------------------
# Validate environment / inputs
# ------------------------------------------------------------------------------
[[ -n "${hexStakePool}" ]] || die "hexStakePool is empty (set your pool hex id)."
[[ -S "${CARDANO_NODE_SOCKET_PATH:-}" ]] || die "CARDANO_NODE_SOCKET_PATH not found or not a socket: ${CARDANO_NODE_SOCKET_PATH:-<unset>}"

for x in "${binCardanoCli}" "${binCnCli}" "${binJq}" "${binTimeout}"; do
  command -v "${x}" >/dev/null 2>&1 || die "Missing or not executable (not in PATH?): ${x}"
done

# Warn early if mail is configured but binary missing (email is optional)
if [[ -n "${mailLeaderLogTo}" && ! $(command -v "${binMail}" 2>/dev/null) ]]; then
  log "WARN: mailLeaderLogTo is set, but '${binMail}' not found or not executable; emails will be skipped."
fi

[[ -r "${vrfSigningKeyFile}"   ]] || die "VRF signing key not readable: ${vrfSigningKeyFile}"
[[ -r "${shelleyGenesisFile}"  ]] || die "Shelley genesis not readable: ${shelleyGenesisFile}"
[[ -r "${byronGenesisFile}"    ]] || die "Byron genesis not readable: ${byronGenesisFile}"
[[ -r "${dbCnCli}"             ]] || die "CNCLI DB not readable: ${dbCnCli}"

if [[ -n "${jsonPoolTool}" ]]; then
  [[ -r "${jsonPoolTool}" ]] || die "PoolTool config not readable: ${jsonPoolTool}"
fi

# Prepare output dir for CSV (if set)
if [[ -n "${slotsCsvFile}" ]]; then
  mkdir -p "$(dirname "${slotsCsvFile}")"
fi

# Versions snapshot
log "Starting cncli-leaderlog run"
cardanoVersion="$("${binCardanoCli}" --version)"
cncliVersion="$("${binCnCli}" --version)"
jqVersion="$("${binJq}" --version)"
timeoutVersion="$("${binTimeout}" --version)"
log "cardano-cli: ${cardanoVersion%%$'\n'*}"
log "cncli:       ${cncliVersion}"
log "jq:          ${jqVersion}"
log "timeout:     ${timeoutVersion%%$'\n'*}"
log "timezone:    ${timezone}"
log "pool:        ${hexStakePool}"

# ------------------------------------------------------------------------------
# Script internal variables (epoch math)
# ------------------------------------------------------------------------------
binCardanoCliMajorVersion="$(awk '{print $2}' <<<"${cardanoVersion%%$'\n'*}" | cut -d'.' -f1)"
secondsCardanoStart=$(date +%s -d "2017-09-23 21:44:51 +0000")
daysCardanoStart=$(( secondsCardanoStart / 86400 ))
secondsNow=$(date +%s)
daysNow=$(( secondsNow / 86400 ))
secondsSinceCardanoStart=$(( secondsNow - secondsCardanoStart ))
daysSinceCardanoStart=$(( daysNow - daysCardanoStart ))
secondsLeftInEpoch=$(( 432000 - (secondsSinceCardanoStart % 432000) ))
dayOfEpoch=$(( daysSinceCardanoStart % 5 ))
currentEpoch=$(( ( daysSinceCardanoStart - 1 ) / 5 ))

if [[ $dayOfEpoch -eq 0 ]]; then
  log "Today is the last day of epoch ${currentEpoch}"
else
  log "Today is day ${dayOfEpoch} of epoch ${currentEpoch}"
fi

# Temp file handling
leaderlogJsonFile="$(mktemp /tmp/leaderlog.XXXXXXXX.json)"
cleanup() { shred -uz "${leaderlogJsonFile}" 2>/dev/null || true; }
trap cleanup EXIT

run_timeout() {
  # usage: run_timeout <cmd...>
  if ! "${binTimeout}" --preserve-status "${CMD_TIMEOUT}" "$@"; then
    log "Timeout or failure running: $* (CMD_TIMEOUT=${CMD_TIMEOUT})"
    return 1
  fi
}

# ------------------------------------------------------------------------------
# Functions
# ------------------------------------------------------------------------------
calculateLeaderLog () {
  # $1 ledger-set (prev/current/next), $2 epoch-number, $3 stake key (stakeGo/stakeSet/stakeMark)
  log "Calculating leaderlog for $1 (${2}) epoch…"

  local poolSnapshot
  if ! poolSnapshot="$(run_timeout nice -n19 "${binCardanoCli}" query stake-snapshot \
        --stake-pool-id "${hexStakePool}" --mainnet)"; then
    die "cardano-cli stake-snapshot failed for pool ${hexStakePool}"
  fi

  local poolTotalStake poolActiveStake
  if [[ ${binCardanoCliMajorVersion} -eq 1 ]]; then
    poolTotalStake="$(grep -oP "(?<=    \"pool${3^}\": )\d+(?=,?)" <<<"${poolSnapshot}")" || die "Missing pool stake"
    poolActiveStake="$(grep -oP "(?<=    \"active${3^}\": )\d+(?=,?)" <<<"${poolSnapshot}")" || die "Missing active stake"
  else
    local stakeNumbers
    stakeNumbers="$(grep -oP "(?<=    \"$3\": )\d+(?=,?)" <<<"${poolSnapshot}" | tr '\n' ' ')" || die "Missing snapshot stake"
    poolTotalStake="$(cut -d' ' -f1 <<<"${stakeNumbers}")"
    poolActiveStake="$(cut -d' ' -f2 <<<"${stakeNumbers}")"
  fi

  if [[ ! "${poolTotalStake}" =~ ^[0-9]+$ || ! "${poolActiveStake}" =~ ^[0-9]+$ ]]; then
    log "DEBUG poolSnapshot: ${poolSnapshot}"
    die "Could not parse pool stake numbers (total='${poolTotalStake}' active='${poolActiveStake}')"
  fi
  log "Stake parsed: total=${poolTotalStake} active=${poolActiveStake}"

  # Optional consensus flag (only if set)
  local consensus_args=()
  [[ -n "${consensusMode}" ]] && consensus_args+=(--consensus "${consensusMode}")

  if ! run_timeout nice -n19 "${binCnCli}" leaderlog \
      --db "${dbCnCli}" --pool-id "${hexStakePool}" --pool-vrf-skey "${vrfSigningKeyFile}" \
      --byron-genesis "${byronGenesisFile}" --shelley-genesis "${shelleyGenesisFile}" \
      --pool-stake "${poolTotalStake}" --active-stake "${poolActiveStake}" \
      "${consensus_args[@]}" \
      --tz "${timezone}" --ledger-set "${1}" > "${leaderlogJsonFile}"; then
    die "cncli leaderlog failed"
  fi

  # Validate JSON status
  local status
  status="$("${binJq}" -er '.status' < "${leaderlogJsonFile}")" || die "Invalid leaderlog JSON"
  if [[ "${status}" != "ok" ]]; then
    log "Leaderlog status not ok. Full JSON follows:"
    cat "${leaderlogJsonFile}" | tee -a "${LOG_FILE}"
    die "Leaderlog status='${status}'"
  fi
  log "Leaderlog calculation done (status=ok)"
}

mailLeaderLog () {
  # $1 ledger-set, $2 epoch-number
  if [[ -n "${mailLeaderLogTo}" && -r "${leaderlogJsonFile}" ]]; then
    if command -v "${binMail}" >/dev/null 2>&1; then
      log "Mailing leaderlog to ${mailLeaderLogTo}…"
      if ! { "${binJq}" . < "${leaderlogJsonFile}" | "${binMail}" -s "Leaderlog for $1 epoch (${2})" -- "${mailLeaderLogTo}"; }; then
        die "Mail delivery failed"
      fi
      log "Mail sent"
    else
      log "WARN: mail binary not found (${binMail}); skipping email."
    fi
  else
    log "Not mailing leaderlog (mailLeaderLogTo not set or leaderlog missing)"
  fi
}

sendPoolToolSlots () {
  if [[ -n "${jsonPoolTool}" ]]; then
    [[ -r "${jsonPoolTool}" ]] || die "PoolTool config not readable"
    log "Retrieving CNCLI database status…"
    local statusJson status
    if ! statusJson="$(run_timeout nice -n19 "${binCnCli}" status \
        --db "${dbCnCli}" --byron-genesis "${byronGenesisFile}" \
        --shelley-genesis "${shelleyGenesisFile}")"; then
      die "cncli status failed"
    fi
    status="$("${binJq}" -er '.status' <<<"${statusJson}")" || die "Invalid status JSON"
    if [[ "${status}" != "ok" ]]; then
      log "CNCLI status not ok; payload:"
      log "${statusJson}"
      die "CNCLI status='${status}'"
    fi
    log "CNCLI status ok"

    log "Sending slots to PoolTool…"
    local result
    if ! result="$(run_timeout nice -n19 "${binCnCli}" sendslots \
        --db "${dbCnCli}" --byron-genesis "${byronGenesisFile}" \
        --shelley-genesis "${shelleyGenesisFile}" --config "${jsonPoolTool}")"; then
      die "cncli sendslots failed"
    fi
    # Successful sendslots has no JSON envelope; exit status is authoritative.
    log "PoolTool sendslots done"
  else
    log "Not sending slots to PoolTool (config disabled)"
  fi
}

writeLeaderSlots () {
  local outCsv="${slotsCsvFile}"
  if [[ -n "${CSV_OVERRIDE}" ]]; then
    outCsv="${CSV_OVERRIDE}"
  fi

  if [[ -z "${outCsv}" ]]; then
    log "Not writing CSV: slotsCsvFile not set and no --csv override"
    return
  fi

  mkdir -p "$(dirname "${outCsv}")" || die "Cannot create CSV dir: $(dirname "${outCsv}")"

  local tempCsv
  "${binJq}" -e '.status == "ok" and (.assignedSlots | type == "array")' < "${leaderlogJsonFile}" >/dev/null || die "Invalid leaderlog CSV source"
  tempCsv="$(mktemp "${outCsv}.XXXXXXXX.tmp")"
  if ! "${binJq}" -r '.assignedSlots[] | (.at|tostring) + "," + (.slot|tostring) + "," + (.no|tostring)' < "${leaderlogJsonFile}" > "${tempCsv}"; then
    rm -f -- "${tempCsv}"
    die "jq extraction failed for CSV"
  fi
  if ! mv -f -- "${tempCsv}" "${outCsv}"; then
    rm -f -- "${tempCsv}"
    die "Cannot replace CSV"
  fi
  log "CSV written at: ${outCsv}"
}

writeLeaderProm ()
{
  if [[ -n "${leaderPromFile}" ]]; then
    local slots
    "${binJq}" -e '.status == "ok"' < "${leaderlogJsonFile}" >/dev/null || die "Invalid leaderlog status"
    slots="$("${binJq}" -er '.epochSlots' < "${leaderlogJsonFile}")" || die "Missing slot count"
    printf "assigned_blocks_epoch %d\n" "${slots}" > "${leaderPromFile}"
    log "Wrote total slot count to ${leaderPromFile}."
  else
    log "Not writing total slot count to Prometheus file"
  fi
}

saveToPostgres()
{
  if [[ "${saveToPostgres}" != 'none' ]];
  then
    i=0
    epoch=$("${binJq}" -er '.epoch' < "${leaderlogJsonFile}")
    totalSlots=$("${binJq}" -er '.epochSlots' < "${leaderlogJsonFile}")
    log "Saving '${saveToPostgres}' slots to PostgreSQL... "
    psql -qc "delete from leaderlog where epoch=${epoch} and slot is null"
    rows=$("${binJq}" -c '.assignedSlots[]' < "${leaderlogJsonFile}")

    while read -r row;
    do
      [[ -n "$row" ]] || continue
      no=$("${binJq}" -er '.no' <<<"$row")
      slot=$("${binJq}" -er '.slot' <<<"$row")
      at=$("${binJq}" -er '.at' <<<"$row")
      ts=$(date -d "$at" +%s)

      if [[ "${saveToPostgres}" == "all" || ( "${saveToPostgres}" == "past" && $ts -lt $secondsNow ) ]];
      then
        psql -qc "insert into leaderlog (nr, slot, epoch, scheduled_at) values (${no}, ${slot}, ${epoch}, '${at}')
                  on conflict (slot) do update set nr=${no}, epoch=${epoch}, scheduled_at='${at}'"
      else
        psql -qc "insert into leaderlog (nr, epoch) values (${no}, ${epoch})"
      fi
      i=$((i + 1))
    done <<< "${rows}"

    log "Saved $i of $totalSlots slots to PostgreSQL."
  else
    log "Not saving any slots to PostgreSQL."
  fi
}

# ------------------------------------------------------------------------------
# Test mode: run immediately for CURRENT epoch to validate CSV + email
# ------------------------------------------------------------------------------
if [[ "${TEST}" == "1" ]]; then
  if [[ "${POSTGRES}" == "1" ]]; then
  log "TEST mode: checking connection to PostgeSQL only..."
    psql -c '\conninfo'
    exit 0
  fi

  log "TEST mode: generating leaderlog for CURRENT epoch (${currentEpoch})"
  calculateLeaderLog current "${currentEpoch}" stakeSet
  writeLeaderSlots
  writeLeaderProm
  saveToPostgres

  if [[ "${FORCE_EMAIL}" == "1" ]]; then
    log "TEST mode: force emailing leaderlog"
    mailLeaderLog current "${currentEpoch}"
  else
    log "TEST mode: email not forced (use --force-email to send)"
  fi

  log "TEST mode complete."
  exit 0
fi

# ------------------------------------------------------------------------------
# Scheduler logic
# ------------------------------------------------------------------------------
# Run within 10 minutes of epoch start
if [[ ( $dayOfEpoch -eq 0 && $secondsLeftInEpoch -lt 432000 && $secondsLeftInEpoch -gt 431400 ) || $CURRENT -eq 1 ]]; then
  calculateLeaderLog prev $((currentEpoch-1)) stakeGo
  saveToPostgres
  calculateLeaderLog current "${currentEpoch}" stakeSet
  sendPoolToolSlots
  writeLeaderProm
  saveToPostgres
fi

# Run as soon as the leaderlog is available (day 4, ~1.5 days left)
if [[ ( $dayOfEpoch -eq 4 && $secondsLeftInEpoch -le 129600 && $secondsLeftInEpoch -gt 129000 ) || $NEXT -eq 1 ]]; then
  calculateLeaderLog next $((currentEpoch+1)) stakeMark
  mailLeaderLog next $((currentEpoch+1))
  writeLeaderSlots
  saveToPostgres
fi

log "Completed cncli-leaderlog run"
