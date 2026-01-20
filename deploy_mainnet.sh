#!/bin/bash
# Rumi Protocol V2 Mainnet Deployment Script
# Generated: 2025-01-19

set -e  # Exit on any error

# =============================================================================
# CONFIGURATION
# =============================================================================

# Team Principals (Controllers)
ROB_PRINCIPAL="fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"
AGNES_PRINCIPAL="wrppb-amng2-jzskb-wcmam-mwrmi-ci52r-bkkre-tzu35-hjfpb-dnl4p-6qe"
GURLEEN_PRINCIPAL="bsu7v-jz2ty-tyonm-dmkdj-nir27-num7e-dtlff-4vmjj-gagxl-xiljg-lqe"
CYCLEOPS_PRINCIPAL="cpbhu-5iaaa-aaaad-aalta-cai"

# Developer Principal (Rob)
DEVELOPER_PRINCIPAL="$ROB_PRINCIPAL"

# External Canisters
ICP_LEDGER_PRINCIPAL="ryjl3-tyaaa-aaaaa-aaaba-cai"
XRC_PRINCIPAL="uf6dk-hyaaa-aaaaq-qaaaq-cai"

# Network
NETWORK="ic"

# icUSD Token Configuration
ICUSD_NAME="icUSD"
ICUSD_SYMBOL="icUSD"
ICUSD_DECIMALS=8
ICUSD_FEE=0  # Zero fee for now

# Stability Pool Configuration
LIQUIDATION_DISCOUNT=10  # 10%
MAX_LTV_RATIO=66         # 66%

# Backend Fee (zero for now)
BACKEND_FEE_E8S=0

echo "============================================"
echo "Rumi Protocol V2 - Mainnet Deployment"
echo "============================================"
echo ""
echo "Network: $NETWORK"
echo "Developer: $DEVELOPER_PRINCIPAL"
echo ""

# =============================================================================
# STEP 1: Create canisters with all controllers
# =============================================================================

echo "[Step 1] Creating canisters..."

CONTROLLERS="--controller $ROB_PRINCIPAL --controller $AGNES_PRINCIPAL --controller $GURLEEN_PRINCIPAL --controller $CYCLEOPS_PRINCIPAL"

# Create all canisters first
dfx canister --network $NETWORK create icusd_ledger $CONTROLLERS
dfx canister --network $NETWORK create rumi_treasury $CONTROLLERS
dfx canister --network $NETWORK create rumi_stability_pool $CONTROLLERS
dfx canister --network $NETWORK create rumi_protocol_backend $CONTROLLERS
dfx canister --network $NETWORK create vault_frontend $CONTROLLERS
dfx canister --network $NETWORK create rumi_protocol_frontend $CONTROLLERS

echo "[Step 1] Canisters created successfully!"
echo ""

# =============================================================================
# STEP 2: Get Canister IDs
# =============================================================================

echo "[Step 2] Getting canister IDs..."

ICUSD_LEDGER_ID=$(dfx canister --network $NETWORK id icusd_ledger)
TREASURY_ID=$(dfx canister --network $NETWORK id rumi_treasury)
STABILITY_POOL_ID=$(dfx canister --network $NETWORK id rumi_stability_pool)
BACKEND_ID=$(dfx canister --network $NETWORK id rumi_protocol_backend)
VAULT_FRONTEND_ID=$(dfx canister --network $NETWORK id vault_frontend)
PROTOCOL_FRONTEND_ID=$(dfx canister --network $NETWORK id rumi_protocol_frontend)

echo "  icusd_ledger:          $ICUSD_LEDGER_ID"
echo "  rumi_treasury:         $TREASURY_ID"
echo "  rumi_stability_pool:   $STABILITY_POOL_ID"
echo "  rumi_protocol_backend: $BACKEND_ID"
echo "  vault_frontend:        $VAULT_FRONTEND_ID"
echo "  rumi_protocol_frontend: $PROTOCOL_FRONTEND_ID"
echo ""

# =============================================================================
# STEP 3: Build canisters (only production canisters, not test_*)
# =============================================================================

echo "[Step 3] Building canisters..."
dfx build --network $NETWORK icusd_ledger
dfx build --network $NETWORK rumi_treasury
dfx build --network $NETWORK rumi_stability_pool
dfx build --network $NETWORK rumi_protocol_backend
dfx build --network $NETWORK vault_frontend
dfx build --network $NETWORK rumi_protocol_frontend
echo "[Step 3] Build complete!"
echo ""

# =============================================================================
# STEP 4: Prepare icUSD Logo (base64 encoded)
# =============================================================================

echo "[Step 4] Preparing icUSD logo..."
LOGO_BASE64=$(base64 -i src/rumi_protocol_backend/icUSD-logo.png | tr -d '\n')
LOGO_DATA_URL="data:image/png;base64,${LOGO_BASE64}"
echo "[Step 4] Logo prepared (${#LOGO_DATA_URL} characters)"
echo ""

# =============================================================================
# STEP 5: Deploy icusd_ledger
# =============================================================================

echo "[Step 5] Deploying icusd_ledger..."

# The minting account is the backend canister
MINTING_ACCOUNT="record { owner = principal \"$BACKEND_ID\"; subaccount = null }"

ICUSD_INIT_ARGS="(variant { Init = record {
    minting_account = $MINTING_ACCOUNT;
    fee_collector_account = opt record { owner = principal \"$TREASURY_ID\"; subaccount = null };
    transfer_fee = $ICUSD_FEE : nat;
    decimals = opt ($ICUSD_DECIMALS : nat8);
    max_memo_length = opt (256 : nat16);
    token_symbol = \"$ICUSD_SYMBOL\";
    token_name = \"$ICUSD_NAME\";
    metadata = vec { 
        record { \"icrc1:logo\"; variant { Text = \"$LOGO_DATA_URL\" } };
        record { \"icrc1:description\"; variant { Text = \"Decentralized stablecoin on the Internet Computer\" } };
    };
    initial_balances = vec {};
    feature_flags = opt record { icrc2 = true };
    maximum_number_of_accounts = null;
    accounts_overflow_trim_quantity = null;
    archive_options = record {
        num_blocks_to_archive = 10000 : nat64;
        max_transactions_per_response = null;
        trigger_threshold = 10000 : nat64;
        max_message_size_bytes = null;
        cycles_for_archive_creation = opt (10000000000000 : nat64);
        node_max_memory_size_bytes = null;
        controller_id = principal \"$ROB_PRINCIPAL\";
    };
}})"

dfx canister --network $NETWORK install icusd_ledger --argument "$ICUSD_INIT_ARGS"
echo "[Step 5] icusd_ledger deployed!"
echo ""

# =============================================================================
# STEP 6: Deploy rumi_treasury
# =============================================================================

echo "[Step 6] Deploying rumi_treasury..."

TREASURY_INIT_ARGS="(record {
    controller = principal \"$ROB_PRINCIPAL\";
    icusd_ledger = principal \"$ICUSD_LEDGER_ID\";
    icp_ledger = principal \"$ICP_LEDGER_PRINCIPAL\";
    ckbtc_ledger = null;
})"

dfx canister --network $NETWORK install rumi_treasury --argument "$TREASURY_INIT_ARGS"
echo "[Step 6] rumi_treasury deployed!"
echo ""

# =============================================================================
# STEP 7: Deploy rumi_stability_pool
# =============================================================================

echo "[Step 7] Deploying rumi_stability_pool..."

STABILITY_POOL_INIT_ARGS="(record {
    protocol_owner = principal \"$ROB_PRINCIPAL\";
    liquidation_discount = $LIQUIDATION_DISCOUNT : nat8;
    max_ltv_ratio = $MAX_LTV_RATIO : nat8;
})"

dfx canister --network $NETWORK install rumi_stability_pool --argument "$STABILITY_POOL_INIT_ARGS"
echo "[Step 7] rumi_stability_pool deployed!"
echo ""

# =============================================================================
# STEP 8: Deploy rumi_protocol_backend
# =============================================================================

echo "[Step 8] Deploying rumi_protocol_backend..."

BACKEND_INIT_ARGS="(variant { Init = record {
    xrc_principal = principal \"$XRC_PRINCIPAL\";
    icusd_ledger_principal = principal \"$ICUSD_LEDGER_ID\";
    icp_ledger_principal = principal \"$ICP_LEDGER_PRINCIPAL\";
    fee_e8s = $BACKEND_FEE_E8S : nat64;
    developer_principal = principal \"$DEVELOPER_PRINCIPAL\";
    treasury_principal = opt principal \"$TREASURY_ID\";
    stability_pool_principal = opt principal \"$STABILITY_POOL_ID\";
}})"

dfx canister --network $NETWORK install rumi_protocol_backend --argument "$BACKEND_INIT_ARGS"
echo "[Step 8] rumi_protocol_backend deployed!"
echo ""

# =============================================================================
# STEP 9: Deploy frontends
# =============================================================================

echo "[Step 9] Deploying frontends..."

dfx canister --network $NETWORK install vault_frontend
dfx canister --network $NETWORK install rumi_protocol_frontend

echo "[Step 9] Frontends deployed!"
echo ""

# =============================================================================
# SUMMARY
# =============================================================================

echo "============================================"
echo "DEPLOYMENT COMPLETE!"
echo "============================================"
echo ""
echo "Canister URLs:"
echo "  Frontend:       https://${PROTOCOL_FRONTEND_ID}.icp0.io"
echo "  Vault Frontend: https://${VAULT_FRONTEND_ID}.icp0.io"
echo ""
echo "Canister IDs:"
echo "  icusd_ledger:          $ICUSD_LEDGER_ID"
echo "  rumi_treasury:         $TREASURY_ID"
echo "  rumi_stability_pool:   $STABILITY_POOL_ID"
echo "  rumi_protocol_backend: $BACKEND_ID"
echo "  vault_frontend:        $VAULT_FRONTEND_ID"
echo "  rumi_protocol_frontend: $PROTOCOL_FRONTEND_ID"
echo ""
echo "Configuration:"
echo "  icUSD Decimals: $ICUSD_DECIMALS"
echo "  Transfer Fee:   $ICUSD_FEE"
echo "  XRC Canister:   $XRC_PRINCIPAL"
echo ""
echo "Controllers (all canisters):"
echo "  Rob:      $ROB_PRINCIPAL"
echo "  Agnes:    $AGNES_PRINCIPAL"
echo "  Gurleen:  $GURLEEN_PRINCIPAL"
echo "  CycleOps: $CYCLEOPS_PRINCIPAL"
echo ""
echo "Next steps:"
echo "1. Test vault creation on the frontend"
echo "2. Verify icUSD minting works"
echo "3. Test liquidation page"
echo "4. Update any frontend environment variables if needed"
echo ""
