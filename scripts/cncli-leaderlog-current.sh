#!/bin/bash
cardano-cli query leadership-schedule \
--mainnet \
--genesis $CNODE_HOME/shelley-genesis.json \
--stake-pool-id (cat $CNODE_HOME/stakepoolid.txt) \
--vrf-signing-key-file $CNODE_HOME/vrf.skey \
--current

