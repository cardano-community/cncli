#!/usr/bin/env bash
#
# Send slots to PoolTool, write slots.csv and mail leaderlog
#
# Depending on the day of the epoch we are in (1 to 5) this script will run one
# or more of the tasks above.
# On day 1: send slots for the current and previous epoch to PoolTool.
# On day 4: calculate and mail next epoch leaderlog and write slots.csv.
#
# Usage:   Via systemd timer or ./cncli-leaderlog.sh
# Author:  Leon • HAPPY Staking Pool
#
# Change the 'Pool specific variables' with values for your pool:
#  - hexStakePool: The hexadecimal hash of your pool
#  - jsonPoolTool: The config file to send slots to PoolTool (leave empty to skip).
#  - slotsCsvFile: The CSV file to write assigned slots to (leave empty to skip).
#  - mailLeaderLogsTo: The address to mail the leaderlog to (leave empty to skip).

export CARDANO_NODE_SOCKET_PATH="/var/lib/cardano/mainnet/node.socket"

# Pool specific variables
timezone="Etc/UTC"
hexStakePool=""
jsonPoolTool="/usr/local/etc/pooltool.json"
slotsCsvFile="/var/local/cncli/slots.csv"
mailLeaderLogTo=""
vrfSigningKeyFile="/etc/cardano/mainnet/keys/vrf.skey"
shelleyGenesisFile="/etc/cardano/mainnet/shelley-genesis.json"
byronGenesisFile="/etc/cardano/mainnet/byron-genesis.json"
binCardanoCli="/usr/local/bin/cardano-cli"
binCnCli="/usr/local/bin/cncli"
dbCnCli="/var/local/cncli/db.sqlite"

# Script internal variables
binCardanoCliMajorVersion=$(cardano-cli --version | head -n 1 | awk '{print $2}' | cut -d'.' -f1)
secondsCardanoStart=$(date +%s -d "2017-09-23 21:44:51 +0000")
daysCardanoStart=$(( secondsCardanoStart / 86400 ))
secondsNow=$(date +%s)
daysNow=$(( secondsNow / 86400 ))
secondsSinceCardanoStart=$(( secondsNow - secondsCardanoStart ))
daysSinceCardanoStart=$(( daysNow - daysCardanoStart ))
secondsLeftInEpoch=$(( 432000 - (secondsSinceCardanoStart % 432000) ))
dayOfEpoch=$(( daysSinceCardanoStart % 5 ))
currentEpoch=$(( ( daysSinceCardanoStart - 1 ) / 5 ))

set -o pipefail

if [[ $dayOfEpoch -eq 0 ]]; then
    echo "Today is the last day of epoch ${currentEpoch}"
else
    echo "Today is day ${dayOfEpoch} of epoch ${currentEpoch}"
fi

calculateLeaderLog ()
{
    if [[ -r "${vrfSigningKeyFile}" ]];
    then
        echo -n "Calculating leaderlog for $1 (${2}) epoch... "
        poolSnapshot=$(nice -n 19 $binCardanoCli query stake-snapshot \
            --stake-pool-id $hexStakePool --mainnet)
        if [[ $? -eq 0 ]]; then echo "done"; else echo "failed!"; fi

        if [[ $binCardanoCliMajorVersion -eq 1 ]];
        then
            poolTotalStake=$(echo "$poolSnapshot" | jq -r .poolStakeMark)
            poolActiveStake=$(echo "$poolSnapshot" | jq -r .activeStakeMark)
        else
            poolTotalStake=$(echo "$poolSnapshot" | jq -r .pools.${hexStakePool}.$3)
            poolActiveStake=$(echo "$poolSnapshot" | jq -r .total.$3)
        fi

        $binCnCli leaderlog \
            --db $dbCnCli --pool-id $hexStakePool --pool-vrf-skey $vrfSigningKeyFile \
            --byron-genesis $byronGenesisFile --shelley-genesis $shelleyGenesisFile \
            --pool-stake $poolTotalStake --active-stake $poolActiveStake \
            --tz $timezone --ledger-set ${1} > /tmp/leaderlog

    else
        echo "The VRF signing key file is not readable."
        exit 1
    fi
}

mailLeaderLog ()
{
    if [[ `jq -r '.status' <<< "$(cat /tmp/leaderlog)"` == "ok" ]];
    then
        if [[ "${mailLeaderLogTo}" != "" ]];
        then
            echo -n "Mailing leaderlog to ${mailLeaderLogTo}... "
            cat /tmp/leaderlog | jq | mail -s "Leaderlog for $1 epoch (${2})" -- $mailLeaderLogTo
            if [[ $? -eq 0 ]]; then echo "done"; else echo "failed!"; fi
        else
            echo "Not mailing leaderlog"
        fi
    else
        cat /tmp/leaderlog
    fi
}

sendPoolToolSlots ()
{
    if [[ "${jsonPoolTool}" != "" && -r "${jsonPoolTool}" ]];
    then
        echo -n "Retrieving CNCLI database status... "
        status=$(${binCnCli} status --db ${dbCnCli} --byron-genesis ${byronGenesisFile} \
            --shelley-genesis ${shelleyGenesisFile} | jq -r '.status' )
        if [[ "${status}" == "error" ]]; then echo "failed!"; echo "${status}"; else echo "done"; fi

        if [[ "${status}" == "ok" ]];
        then
            echo -n "Sending slots to PoolTool... "
            result=$(${binCnCli} sendslots --db ${dbCnCli} --byron-genesis ${byronGenesisFile} \
                --shelley-genesis ${shelleyGenesisFile} --config ${jsonPoolTool})
            if [[ "${result}" == "error" ]]; then echo "failed!"; echo "${result}"; else echo "done"; fi
        fi
    else
        echo "Not sending slots to PoolTool"
    fi
}

writeLeaderSlots ()
{
    if [[ "${slotsCsvFile}" != "" && -w `dirname "${slotsCsvFile}"` ]];
    then
        echo -n "Writing leaderlog to ${slotsCsvFile}... "
        echo /tmp/leaderlog | jq -r '.assignedSlots[] | (.at|tostring) + "," + (.slot|tostring) + "," + (.no|tostring)' > $slotsCsvFile
        if [[ $? -eq 0 ]]; then echo "done"; else echo "failed!"; fi
    else
        echo "Not writing slots.csv"
    fi
}

if [[ $dayOfEpoch -eq 1 ]];
then
    calculateLeaderLog prev $(($currentEpoch-1)) stakeSet
    calculateLeaderLog current $currentEpoch stakeSet
    sendPoolToolSlots
fi

if [[ $dayOfEpoch -eq 4 && $secondsLeftInEpoch -le 129600 && $secondsLeftInEpoch -gt 84600 ]];
then
    calculateLeaderLog next $(($currentEpoch+1)) stakeMark
    mailLeaderLog next $(($currentEpoch+1))
    writeLeaderSlots
fi
