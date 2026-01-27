#!/bin/bash
# Script to update icUSD logo on the live ledger
# Canister ID: t6bor-paaaa-aaaap-qrd5q-cai

set -e

NETWORK="ic"
ICUSD_LEDGER_ID="t6bor-paaaa-aaaap-qrd5q-cai"

echo "============================================"
echo "Updating icUSD Logo"
echo "============================================"
echo ""

# Check if logo file exists
LOGO_FILE="src/rumi_protocol_backend/icUSD-logo-256.png"
if [ ! -f "$LOGO_FILE" ]; then
    echo "ERROR: Logo file not found at $LOGO_FILE"
    echo "Please copy your new logo to this location first."
    exit 1
fi

echo "Logo file: $LOGO_FILE"
echo "Target ledger: $ICUSD_LEDGER_ID"
echo ""

# Encode logo as base64
echo "Encoding logo as base64..."
LOGO_BASE64=$(base64 -i "$LOGO_FILE" | tr -d '\n')
LOGO_DATA_URL="data:image/png;base64,${LOGO_BASE64}"
echo "Logo data URL length: ${#LOGO_DATA_URL} characters"
echo ""

# Prepare upgrade args with new metadata
UPGRADE_ARGS="(variant { Upgrade = opt record {
    metadata = opt vec { 
        record { \"icrc1:logo\"; variant { Text = \"$LOGO_DATA_URL\" } };
        record { \"icrc1:description\"; variant { Text = \"Decentralized stablecoin on the Internet Computer\" } };
    };
}})"

echo "Upgrading ledger with new logo..."
echo ""

# Perform the upgrade (mode upgrade preserves state)
dfx canister --network $NETWORK install $ICUSD_LEDGER_ID --mode upgrade --wasm src/ledger/ic-icrc1-ledger.wasm --argument "$UPGRADE_ARGS"

echo ""
echo "============================================"
echo "Logo update complete!"
echo "============================================"
echo ""
echo "Verify by checking metadata:"
echo "dfx canister --network $NETWORK call $ICUSD_LEDGER_ID icrc1_metadata '()'"
echo ""
