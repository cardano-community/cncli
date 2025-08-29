#!/usr/bin/env bash
#
# Send slots to PoolTool, write slots.csv and mail leaderlog
#
# Depending on the day of the epoch we are in (1 to 5) this script will run one
# or more of the tasks above.
# On epoch start: send slots for the current and previous epoch to PoolTool.
# On epoch day 4: calculate next epoch leaderlog, mail it and/or write slots.csv.
#
# Usage:
#   Via systemd timer or ./cncli-leaderlog.sh [--test] [--force-email] [--csv PATH]
#
# Original Author: Leon • HAPPY Staking Pool
# Improvements by Rick • RCADA Pool:    safer strict mode, logging, timeouts, VRF check, test mode, CSV atomic write

# ------------------------------------------------------------------------------
# Pool specific variables (EDIT THESE)
# ------------------------------------------------------------------------------

# export CARDANO_NODE_SOCKET_PATH="/path/to/node.socket"  # (exported by your env/service)

timezone="Etc/UTC"                             # REQUIRED: set correct timezone for leaderlog scheduling
hexStakePool=""                                # REQUIRED: pool id in hex
jsonPoolTool="/usr/local/etc/pooltool.json"    # optional: path to PoolTool json (leave empty to skip)
slotsCsvFile="/var/local/cncli/slots.csv"      # optional: path to write assigned slots CSV (leave empty to skip)
mailLeaderLogTo=""                             # optional: email address to send leaderlog (leave empty to skip)

vrfSigningKeyFile="/etc/cardano/mainnet/keys/vrf.skey"             # REQUIRED
shelleyGenesisFile="/etc/cardano/mainnet/shelley-genesis.json"     # REQUIRED
byronGenesisFile="/etc/cardano/mainnet/byron-genesis.json"         # REQUIRED
dbCnCli="/path/to/cncli.db"                                        # REQUIRED
binCardanoCli=""                               # optional: (override with ex. /usr/local/bin/cardano-cli if needed; defaults to $PATH)
binCnCli=""                                    # optional: (override with ex. /usr/local/bin/cncli if needed; defaults to $PATH)
binPython3=""                                  # optional: (override with ex. /usr/bin/python3 if needed; defaults to $PATH)

# Consensus mode for leader schedule calc: cpraos | praos (leave empty to omit --consensus flag)
consensus_mode=""

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
LOG_DIR="${LOG_DIR:-$HOME/.cncli-leaderlog}"
LOG_FILE="${LOG_DIR}/cncli-leaderlog.log"
LOCK_FILE="${LOG_DIR}/cncli-leaderlog.lock"
CMD_TIMEOUT="${CMD_TIMEOUT:-120s}"             # default timeout for long ops
DEBUG="${DEBUG:-0}"                            # set DEBUG=1 to enable bash -x

# --- CLI flags (optional) ---
TEST="${TEST:-0}"
FORCE_EMAIL="${FORCE_EMAIL:-0}"
CSV_OVERRIDE="${CSV_OVERRIDE:-}"

usage() {
  cat <<EOF
Usage: $0 [--test] [--force-email] [--csv /path/to/slots.csv]
  --test           Run leaderlog for CURRENT epoch now, write CSV, optionally email.
  --force-email    Force email send during --test (ignores timing windows).
  --csv PATH       Override CSV output path for this run only.

Environment equivalents:
  TEST=1 FORCE_EMAIL=1 CSV_OVERRIDE=/tmp/slots.csv $0
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test) TEST=1; shift ;;
    --force-email) FORCE_EMAIL=1; shift ;;
    --csv) CSV_OVERRIDE="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown flag: $1"; usage; exit 1 ;;
  endcase
done 2>/dev/null || true

# ------------------------------------------------------------------------------
# Strict mode + logging + error trap
# ------------------------------------------------------------------------------
set -Eeuo pipefail
if [[ "${DEBUG}" == "1" ]]; then set -x; fi

mkdir -p "${LOG_DIR}"

log() {
  # usage: log "message"
  printf '%s %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" | tee -a "${LOG_FILE}"
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
exec {lock_fd}>"${LOCK_FILE}" || die "Cannot open lock ${LOCK_FILE}"
flock -n "${lock_fd}" || die "Another instance is running (lock: ${LOCK_FILE})"

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
  [[ -r "${jsonPoolTool}" ]] || log "WARN: PoolTool config not readable: ${jsonPoolTool} (sending to PoolTool will be skipped)."
fi

# Prepare output dir for CSV (if set)
if [[ -n "${slotsCsvFile}" ]]; then
  mkdir -p "$(dirname "${slotsCsvFile}")"
fi

# Versions snapshot
log "Starting cncli-leaderlog run"
log "cardano-cli: $(${binCardanoCli} --version | head -n1)"
log "cncli:       $(${binCnCli} --version 2>/dev/null || echo 'unknown')"
log "jq:          $(${binJq} --version)"
log "timeout:     $(${binTimeout} --version | head -n1)"
log "timezone=${timezone} pool=${hexStakePool:0:8}…"

# ------------------------------------------------------------------------------
# Script internal variables (epoch math)
# ------------------------------------------------------------------------------
binCardanoCliMajorVersion="$(${binCardanoCli} --version | head -n1 | awk '{print $2}' | cut -d'.' -f1)"
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
LEADERLOG_JSON="$(mktemp /tmp/leaderlog.XXXXXXXX.json)"
cleanup() { rm -f "${LEADERLOG_JSON}" 2>/dev/null || true; }
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
    poolTotalStake="$(grep -oP "(?<=    \"pool${3^}\": )\d+(?=,?)" <<<"${poolSnapshot}" || true)"
    poolActiveStake="$(grep -oP "(?<=    \"active${3^}\": )\d+(?=,?)" <<<"${poolSnapshot}" || true)"
  else
    local stakeNumbers
    stakeNumbers="$(grep -oP "(?<=    \"$3\": )\d+(?=,?)" <<<"${poolSnapshot}" | tr '\n' ' ' || true)"
    poolTotalStake="$(cut -d' ' -f1 <<<"${stakeNumbers}")"
    poolActiveStake="$(cut -d' ' -f2 <<<"${stakeNumbers}")"
  fi

  if [[ -z "${poolTotalStake}" || -z "${poolActiveStake}" ]]; then
    log "DEBUG poolSnapshot: ${poolSnapshot}"
    die "Could not parse pool stake numbers (total='${poolTotalStake}' active='${poolActiveStake}')"
  fi
  log "Stake parsed: total=${poolTotalStake} active=${poolActiveStake}"

  # Optional consensus flag (only if set)
  local consensus_args=()
  [[ -n "${consensus_mode}" ]] && consensus_args+=(--consensus "${consensus_mode}")

  if ! run_timeout nice -n19 "${binCnCli}" leaderlog \
      --db "${dbCnCli}" --pool-id "${hexStakePool}" --pool-vrf-skey "${vrfSigningKeyFile}" \
      --byron-genesis "${byronGenesisFile}" --shelley-genesis "${shelleyGenesisFile}" \
      --pool-stake "${poolTotalStake}" --active-stake "${poolActiveStake}" \
      "${consensus_args[@]}" \
      --tz "${timezone}" --ledger-set "${1}" > "${LEADERLOG_JSON}"; then
    die "cncli leaderlog failed"
  fi

  # Validate JSON status
  local status
  status="$(${binJq} -r '.status // empty' < "${LEADERLOG_JSON}" || true)"
  if [[ "${status}" != "ok" ]]; then
    log "Leaderlog status not ok. Full JSON follows:"
    cat "${LEADERLOG_JSON}" | tee -a "${LOG_FILE}"
    die "Leaderlog status='${status}'"
  fi
  log "Leaderlog calculation done (status=ok)"
}

mailLeaderLog () {
  # $1 ledger-set, $2 epoch-number
  if [[ -n "${mailLeaderLogTo}" && -r "${LEADERLOG_JSON}" ]]; then
    if command -v "${binMail}" >/dev/null 2>&1; then
      log "Mailing leaderlog to ${mailLeaderLogTo}…"
      if ! { ${binJq} . < "${LEADERLOG_JSON}" | "${binMail}" -s "Leaderlog for $1 epoch (${2})" -- "${mailLeaderLogTo}"; }; then
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
  if [[ -n "${jsonPoolTool}" && -r "${jsonPoolTool}" ]]; then
    log "Retrieving CNCLI database status…"
    local statusJson status
    if ! statusJson="$(run_timeout nice -n19 "${binCnCli}" status \
        --db "${dbCnCli}" --byron-genesis "${byronGenesisFile}" \
        --shelley-genesis "${shelleyGenesisFile}")"; then
      die "cncli status failed"
    fi
    status="$(${binJq} -r '.status // empty' <<<"${statusJson}")"
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
    if grep -q '"error"' <<<"${result}"; then
      log "PoolTool send returned error: ${result}"
      die "sendslots reported error"
    fi
    log "PoolTool sendslots done"
  else
    log "Not sending slots to PoolTool (config missing/unreadable)"
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

  local status
  status="$(${binJq} -r '.status // empty' < "${LEADERLOG_JSON}" || true)"
  if [[ "${status}" == "ok" ]]; then
    log "Writing leaderlog CSV to ${outCsv}…"
    if ! ${binJq} -r '.assignedSlots[] | (.at|tostring) + "," + (.slot|tostring) + "," + (.no|tostring)' < "${LEADERLOG_JSON}" > "${outCsv}.tmp"; then
      die "jq extraction failed for CSV"
    fi
    mv -f "${outCsv}.tmp" "${outCsv}"
    log "CSV written at: ${outCsv}"
  else
    log "Not writing CSV: leaderlog status='${status}'"
  fi
}

# ------------------------------------------------------------------------------
# Test mode: run immediately for CURRENT epoch to validate CSV + email
# ------------------------------------------------------------------------------
if [[ "${TEST}" == "1" ]]; then
  log "TEST mode: generating leaderlog for CURRENT epoch (${currentEpoch})"
  calculateLeaderLog current "${currentEpoch}" stakeSet
  writeLeaderSlots

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
if [[ $dayOfEpoch -eq 0 && $secondsLeftInEpoch -lt 432000 && $secondsLeftInEpoch -gt 431400 ]]; then
  calculateLeaderLog prev $((currentEpoch-1)) stakeGo
  calculateLeaderLog current "${currentEpoch}"  stakeSet
  sendPoolToolSlots
fi

# Run as soon as the leaderlog is available (day 4, ~1.5 days left)
if [[ $dayOfEpoch -eq 4 && $secondsLeftInEpoch -le 129600 && $secondsLeftInEpoch -gt 129000 ]]; then
  calculateLeaderLog next $((currentEpoch+1)) stakeMark
  mailLeaderLog next $((currentEpoch+1))
  writeLeaderSlots
fi

log "Completed cncli-leaderlog run"
