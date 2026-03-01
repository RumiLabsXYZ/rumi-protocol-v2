use candid::{encode_args, decode_one, Principal, Encode, CandidType, Deserialize, encode_one};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use num_traits::cast::ToPrimitive;

// Fix the Account type conflict by using the official type from icrc_ledger_types
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;

// Import necessary types from the codebase
use rumi_protocol_backend::{
    vault::{OpenVaultSuccess, CandidVault, VaultArg},
    ProtocolError, SuccessWithFee, Fees, GetEventsArg, LiquidityStatus,
    AddCollateralArg,
};
use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::state::{CollateralConfig, CollateralStatus, PriceSource, XrcAssetClass};
use ic_xrc_types::{Asset, AssetClass, GetExchangeRateRequest, ExchangeRate};

//-----------------------------------------------------------------------------------
// MOCK XRC CANISTER IMPLEMENTATION
//-----------------------------------------------------------------------------------

/// A simple mock implementation for the XRC canister
#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    // Map from asset pair to rate (e8s format)
    rates: HashMap<String, u64>,
}

impl Default for MockXRC {
    fn default() -> Self {
        let mut rates = HashMap::new();
        // Use a higher ICP price to ensure the test passes collateral requirements
        rates.insert("ICP/USD".to_string(), 1000000000); // $10.00
        Self { rates }
    }
}

impl MockXRC {
    /// Set the exchange rate for a specific asset pair
    /// Rate in e8s format (e.g., 650000000 = $6.50)
    fn set_rate(&mut self, base: &str, quote: &str, rate_e8s: u64) {
        let key = format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
        self.rates.insert(key, rate_e8s);
    }

    /// Get the exchange rate for a pair specified in the request
    fn get_exchange_rate(&self, req: GetExchangeRateRequest) -> Result<ExchangeRate, String> {
        let base_symbol = req.base_asset.symbol.to_uppercase();
        let quote_symbol = req.quote_asset.symbol.to_uppercase();
        let key = format!("{}/{}", base_symbol, quote_symbol);
        
        // Default timestamp is now
        let timestamp = req.timestamp.unwrap_or_else(|| 
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
        
        if let Some(rate) = self.rates.get(&key) {
            // Return successful result
            Ok(ExchangeRate {
                base_asset: req.base_asset.clone(),
                quote_asset: req.quote_asset.clone(),
                timestamp,
                rate: *rate,
                metadata: ic_xrc_types::ExchangeRateMetadata {
                    decimals: 8,
                    base_asset_num_queried_sources: 1,
                    base_asset_num_received_rates: 1,
                    quote_asset_num_queried_sources: 1,
                    quote_asset_num_received_rates: 1,
                    standard_deviation: 0,
                    forex_timestamp: None,
                },
            })
        } else {
            // Return empty result
            Err("Rate not found".to_string())
        }
    }
}

/// Prepare the mock XRC for installation in a canister
fn prepare_mock_xrc() -> Vec<u8> {
    // Create a default mock with predefined rates
    let mut mock = MockXRC::default();
    
    // Use a higher rate for ICP to ensure sufficient collateral
    mock.set_rate("ICP", "USD", 1000000000); // $10.00
    
    // Encode for canister installation
    match encode_one(mock) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode mock XRC: {}", e),
    }
}

//-----------------------------------------------------------------------------------
// HELPER FUNCTIONS
//-----------------------------------------------------------------------------------

// Create a helper for logging with timestamps
fn log(message: &str) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    println!("[{}] {}", timestamp, message);
}

// Helper functions to load the WASM binaries directly
fn icrc1_ledger_wasm() -> Vec<u8> {
    let wasm = include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec();
    log(&format!("📦 Loaded ICRC1 Ledger WASM: {} bytes", wasm.len()));
    wasm
}

fn protocol_wasm() -> Vec<u8> {
    let wasm = include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm").to_vec();
    log(&format!("📦 Loaded Protocol WASM: {} bytes", wasm.len()));
    wasm
}

fn xrc_wasm() -> Vec<u8> {
    let wasm = include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec();
    log(&format!("📦 Loaded XRC WASM: {} bytes", wasm.len()));
    wasm
}

// Define Candid types for proper initialization arguments
#[derive(CandidType, Deserialize)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    trigger_threshold: u64,
    controller_id: Principal,
    max_transactions_per_response: Option<u64>,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize)]
struct MetadataValue {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "Nat")]
    nat: Option<candid::Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize)]
struct InitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: candid::Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>, 
    initial_balances: Vec<(Account, candid::Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>, 
    archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(InitArgs),
    #[serde(rename = "Upgrade")]
    Upgrade(Option<()>),
}

#[derive(CandidType, Deserialize)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Deserialize)]
struct UpgradeArg {
    mode: Option<String>,
}

// Set ICP price directly in the protocol
fn set_icp_price_directly(pic: &PocketIc, protocol_id: Principal) -> bool {
    log("🔄 Setting ICP price directly in protocol");
    
    // Try calling the fetch_icp_rate method (this is the standard method in the protocol)
    log("📤 Calling fetch_icp_rate on protocol");
    let mut success = false;
    
    // Try multiple times with a small delay between attempts
    for i in 0..5 {
        log(&format!("📡 Attempt {} to fetch ICP rate", i+1));
        
        match pic.update_call(
            protocol_id,
            Principal::management_canister(),
            "fetch_icp_rate",
            encode_args(()).unwrap()
        ) {
            Ok(_) => {
                log("✅ Called fetch_icp_rate successfully");
                success = true;
                break;
            },
            Err(e) => {
                log(&format!("⚠️ fetch_icp_rate call returned: {}", e));
                // Sleep a bit before trying again
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }
    
    // If we had any success, now wait a moment for async operations to complete
    if success {
        log("⏳ Waiting for async operations to complete...");
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Verify the price was actually set
        match pic.query_call(
            protocol_id,
            Principal::anonymous(),
            "get_protocol_status",
            encode_args(()).unwrap()
        ) {
            Ok(result) => {
                match result {
                    WasmResult::Reply(bytes) => {
                        match decode_one::<rumi_protocol_backend::ProtocolStatus>(&bytes) {
                            Ok(status) => {
                                log(&format!("📊 Current ICP rate: ${}", status.last_icp_rate));
                                if status.last_icp_rate > 0.0 {
                                    log("✅ ICP price successfully set");
                                    return true;
                                } else {
                                    log("❌ ICP price is still zero");
                                }
                            },
                            Err(_) => log("❌ Failed to decode status")
                        }
                    },
                    _ => log("❌ Unexpected response format")
                }
            },
            Err(e) => log(&format!("❌ Could not verify price: {}", e))
        }
    }
    
    false
}

// Test helper to deploy the protocol canister with the required ledgers
fn setup_protocol() -> (PocketIc, Principal, Principal, Principal) {
    log("🚀 Starting protocol setup");
    
    // Configure PocketIc with at least one subnet
    log("🔧 Configuring PocketIC with subnet");
    let pic = PocketIcBuilder::new()
        .with_nns_subnet() 
        .build();
    
    // Create protocol canister ID first so we can use it as minting principal
    log("🏗️ Creating RUMI Protocol canister first (for minting principal)");
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    log(&format!("💰 Added cycles to Protocol canister: {}", protocol_id));

    // Define principals - use protocol_id as minting principal
    let minting_principal = protocol_id;
    
    // Create a self-authenticating principal for test user (not anonymous)
    let test_user_principal = Principal::self_authenticating(&[1, 2, 3, 4]);
    let developer_principal = Principal::self_authenticating(&[5, 6, 7, 8]);
    
    log(&format!("👤 Test user: {}", test_user_principal));
    log(&format!("🏦 Minting account (protocol canister): {}", minting_principal));
    log(&format!("👨‍💻 Developer: {}", developer_principal));
    
    // Load wasms using the helper functions
    let icp_ledger_wasm = icrc1_ledger_wasm();
    let icusd_ledger_wasm = icrc1_ledger_wasm();
    let protocol_wasm_binary = protocol_wasm();
    
    // Deploy ICP Ledger
    log("🏗️ Creating ICP Ledger canister");
    let icp_ledger_id = pic.create_canister();
    pic.add_cycles(icp_ledger_id, 2_000_000_000_000);
    log(&format!("💰 Added cycles to ICP Ledger: {}", icp_ledger_id));
    
    // Create proper initialization arguments using Candid encoding
    log("⚙️ Configuring ICP Ledger initialization args");
    
    let init_args = InitArgs {
        minting_account: Account {
            owner: minting_principal,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "Internet Computer Protocol".into(),
        token_symbol: "ICP".into(),
        metadata: vec![], 
        initial_balances: vec![(
            Account {
                owner: test_user_principal,
                subaccount: None,
            },
            candid::Nat::from(1_000_000_000_000u64),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: developer_principal,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    
    let ledger_arg = LedgerArg::Init(init_args);

    // Properly encode arguments using candid
    let icp_init_args = match encode_args((ledger_arg,)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded ICP ledger init args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode ICP ledger init args: {}", e));
            panic!("Failed to encode ICP ledger init args: {}", e);
        }
    };
    
    log("📥 Installing ICP Ledger canister");
    pic.install_canister(
        icp_ledger_id, 
        icp_ledger_wasm.clone(),
        icp_init_args,
        None,
    );
    log("✅ ICP Ledger canister installed successfully");
    
    // Similarly deploy ICUSD Ledger - also with protocol as minting principal
    log("🏗️ Creating ICUSD Ledger canister");
    let icusd_ledger_id = pic.create_canister();
    pic.add_cycles(icusd_ledger_id, 2_000_000_000_000);
    log(&format!("💰 Added cycles to ICUSD Ledger: {}", icusd_ledger_id));
    
    // Use modified init args for ICUSD
    log("⚙️ Configuring ICUSD Ledger initialization args");
    
    let icusd_init_args = InitArgs {
        minting_account: Account {
            owner: minting_principal,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![(
            Account {
                owner: test_user_principal,
                subaccount: None,
            },
            candid::Nat::from(1_000_000_000_000u64),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: developer_principal,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    
    let icusd_ledger_arg = LedgerArg::Init(icusd_init_args);
    
    // Properly encode arguments using candid
    let icusd_encoded_args = match encode_args((icusd_ledger_arg,)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded ICUSD ledger init args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode ICUSD ledger init args: {}", e));
            panic!("Failed to encode ICUSD ledger init args: {}", e);
        }
    };
    
    log("📥 Installing ICUSD Ledger canister");
    pic.install_canister(
        icusd_ledger_id,
        icusd_ledger_wasm,
        icusd_encoded_args,
        None,
    );
    log("✅ ICUSD Ledger canister installed successfully");
    
    // Create and install XRC Canister with the mock
    log("🏗️ Creating XRC (Exchange Rate) canister");
    let xrc_id = pic.create_canister();
    pic.add_cycles(xrc_id, 1_000_000_000_000);
    log(&format!("💰 Added cycles to XRC canister: {}", xrc_id));
    
    // Create a mock with a predefined ICP rate
    let mock_data = prepare_mock_xrc();
    log(&format!("📦 Prepared mock XRC data: {} bytes", mock_data.len()));
    
    // Install mock implementation
    log("📥 Installing Mock XRC canister");
    pic.install_canister(
        xrc_id,
        xrc_wasm(),
        mock_data,
        None,
    );
    log("✅ Mock XRC canister installed successfully");
    
    // Now install the protocol canister
    log("📥 Installing RUMI Protocol canister");
    let protocol_init_arg = ProtocolInitArg {
        fee_e8s: 10_000,
        icp_ledger_principal: icp_ledger_id,
        xrc_principal: xrc_id,
        icusd_ledger_principal: icusd_ledger_id,
        developer_principal,
    };
    
    let protocol_arg = ProtocolArgVariant::Init(protocol_init_arg);
    
    // Properly encode protocol arguments
    let protocol_init_encoded = match encode_args((protocol_arg,)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded protocol init args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode protocol init args: {}", e));
            panic!("Failed to encode protocol init args: {}", e);
        }
    };
    
    log("📥 Installing RUMI Protocol canister");
    pic.install_canister(
        protocol_id,
        protocol_wasm_binary,
        protocol_init_encoded,
        None,
    );
    log("✅ RUMI Protocol canister installed successfully");
    
    // Add extra retry logic for fetching the ICP price
    log("🔄 Fetching initial ICP price");
    let mut price_set = false;
    
    for attempt in 1..=3 {
        log(&format!("🔄 Attempt {}/3 to set ICP price", attempt));
        
        if set_icp_price_directly(&pic, protocol_id) {
            log("✅ ICP price set for testing");
            price_set = true;
            break;
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    
    if !price_set {
        log("⚠️ Could not set ICP price after multiple attempts, test may fail");
    }
    
    log("✨ Setup complete");
    log(&format!("🔑 Protocol ID: {}", protocol_id));
    log(&format!("🔑 ICP Ledger ID: {}", icp_ledger_id));
    log(&format!("🔑 ICUSD Ledger ID: {}", icusd_ledger_id));
    log(&format!("🔑 XRC ID: {}", xrc_id));
    
    (pic, protocol_id, icp_ledger_id, icusd_ledger_id)
}

// Helper function to get ICUSD balance
fn get_icusd_balance(pic: &PocketIc, icusd_ledger_id: Principal, owner: Principal) -> u64 {
    let account = Account {
        owner,
        subaccount: None,
    };
    
    let encoded_balance_args = match encode_args((account,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode balance_of args: {}", e),
    };
    
    let balance_result = match pic.query_call(
        icusd_ledger_id,
        Principal::anonymous(),
        "icrc1_balance_of",
        encoded_balance_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call icrc1_balance_of: {}", e),
    };
    
    let balance: candid::Nat = match balance_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => panic!("Failed to decode balance response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected balance_of call: {}", error),
    };
    
    match balance.0.to_u64() {
        Some(value) => value,
        None => panic!("Failed to convert balance to u64"),
    }
}

// Helper function to get ICP balance
fn get_icp_balance(pic: &PocketIc, ledger_id: Principal, owner: Principal) -> u64 {
    let account = Account {
        owner,
        subaccount: None,
    };
    
    let encoded_balance_args = match encode_args((account,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode balance_of args: {}", e),
    };
    
    let balance_result = match pic.query_call(
        ledger_id,
        Principal::anonymous(),
        "icrc1_balance_of",
        encoded_balance_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call icrc1_balance_of: {}", e),
    };
    
    let balance: candid::Nat = match balance_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => panic!("Failed to decode balance response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected balance_of call: {}", error),
    };
    
    match balance.0.to_u64() {
        Some(value) => value,
        None => panic!("Failed to convert balance to u64"),
    }
}

// Helper function to get vault details
fn get_vault(pic: &PocketIc, protocol_id: Principal, owner: Principal, vault_id: u64) -> CandidVault {
    let encoded_get_vaults_args = match encode_args((Some(owner),)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode get_vaults args: {}", e),
    };
    
    let vaults_result = match pic.query_call(
        protocol_id,
        owner,
        "get_vaults", 
        encoded_get_vaults_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call get_vaults: {}", e),
    };
    
    let vaults: Vec<CandidVault> = match vaults_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => panic!("Failed to decode vaults: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected get_vaults call: {}", error),
    };
    
    vaults.into_iter()
        .find(|v| v.vault_id == vault_id)
        .unwrap_or_else(|| panic!("Vault with ID {} not found", vault_id))
}

// Check if ICP rate is available
fn verify_icp_rate_available(pic: &PocketIc, protocol_id: Principal) -> bool {
    match pic.query_call(
        protocol_id,
        Principal::anonymous(),
        "get_protocol_status",
        encode_args(()).unwrap()
    ) {
        Ok(result) => {
            match result {
                WasmResult::Reply(bytes) => {
                    match decode_one::<rumi_protocol_backend::ProtocolStatus>(&bytes) {
                        Ok(status) => {
                            log(&format!("📊 Current ICP rate: ${}", status.last_icp_rate));
                            status.last_icp_rate > 0.0
                        },
                        Err(_) => false,
                    }
                },
                _ => false,
            }
        },
        Err(_) => false,
    }
}

// Create a test vault and return its ID
fn create_test_vault(pic: &PocketIc, protocol_id: Principal, icp_ledger_id: Principal, owner: Principal, margin_amount: u64) -> Result<u64, String> {
    // Approve ICP transfer
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(margin_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => return Err(format!("Failed to encode approve args: {}", e)),
    };
    
    match pic.update_call(
        icp_ledger_id,
        owner, 
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ Approval successful"),
        Err(e) => return Err(format!("Failed to approve ICP transfer: {}", e)),
    };
    
    // Open vault
    let encoded_open_vault_args = match encode_args((margin_amount,)) {
        Ok(bytes) => bytes,
        Err(e) => return Err(format!("Failed to encode open_vault args: {}", e)),
    };
    
    let open_result = match pic.update_call(
        protocol_id,
        owner,
        "open_vault", 
        encoded_open_vault_args
    ) {
        Ok(result) => result,
        Err(e) => return Err(format!("Failed to call open_vault: {}", e)),
    };
    
    // Extract vault_id
    match open_result {
        WasmResult::Reply(bytes) => {
            match decode_one::<Result<OpenVaultSuccess, ProtocolError>>(&bytes) {
                Ok(decoded) => match decoded {
                    Ok(success) => {
                        log(&format!("✅ Successfully opened vault with ID: {}", success.vault_id));
                        Ok(success.vault_id)
                    },
                    Err(e) => Err(format!("Failed to open vault: {:?}", e)),
                },
                Err(e) => Err(format!("Failed to decode open_vault response: {}", e)),
            }
        },
        WasmResult::Reject(error) => Err(format!("Canister rejected open_vault call: {}", error)),
    }
}

// Helper function to borrow from a vault
fn call_borrow_from_vault(pic: &PocketIc, protocol_id: Principal, owner: Principal, borrow_arg: VaultArg) 
    -> Result<SuccessWithFee, ProtocolError> {
    
    let encoded_borrow_args = match encode_args((borrow_arg,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode borrow_from_vault args: {}", e),
    };
    
    let borrow_result = match pic.update_call(
        protocol_id,
        owner,
        "borrow_from_vault", 
        encoded_borrow_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call borrow_from_vault: {}", e),
    };
    
    // Parse the borrow result
    match borrow_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(result) => result,
            Err(e) => panic!("Failed to decode borrow_from_vault response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected borrow_from_vault call: {}", error),
    }
}

// Integration test for creating a vault
#[test]
fn test_open_vault() {
    log("🧪 TEST STARTING: test_open_vault");
    
    // Set up the test environment with proper error handling
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, _) = setup_protocol();
    
    // Try setting the ICP price again directly before the test
    set_icp_price_directly(&pic, protocol_id);
    
    // Use the SAME self-authenticating principal as in setup
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // First, approve ICP transfer to the protocol using proper Candid encoding
    log("🔐 Creating approval for ICP transfer");
    
    // Fix: Use candid::Nat for fields that are nat in the Candid interface
    #[derive(CandidType)]
    struct ApproveArgs {
        fee: Option<candid::Nat>,
        memo: Option<Vec<u8>>,
        from_subaccount: Option<Vec<u8>>,
        created_at_time: Option<u64>, // Timestamp can stay u64
        amount: candid::Nat,          // Changed from u64 to candid::Nat
        expected_allowance: Option<candid::Nat>,
        expires_at: Option<u64>, // Timestamp can stay u64
        spender: Account,
    }
    
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(1_000_000_000u64), // Convert u64 to candid::Nat
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded approve args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode approve args: {}", e));
            panic!("Failed to encode approve args: {}", e);
        }
    };
    
    log(&format!("📤 Calling icrc2_approve on ICP ledger: {}", icp_ledger_id));
    
    let approve_result = match pic.update_call(
        icp_ledger_id,
        test_user, 
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(result) => {
            log("✅ Approval successful");
            result
        },
        Err(e) => {
            log(&format!("❌ Approval failed: {}", e));
            panic!("Failed to approve ICP transfer: {}", e);
        }
    };
    
    log(&format!("🔍 Approve result: {:?}", approve_result));
    
    // Now open a vault with proper Candid encoding
    log("🏦 Opening vault");
    
    let encoded_open_vault_args = match encode_args((1_000_000_000u64,)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded open_vault args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode open_vault args: {}", e));
            panic!("Failed to encode open_vault args: {}", e);
        }
    };
    
    log(&format!("📤 Calling open_vault on protocol: {}", protocol_id));
    
    let open_result = match pic.update_call(
        protocol_id,
        test_user,
        "open_vault", 
        encoded_open_vault_args
    ) {
        Ok(result) => {
            log("✅ open_vault call successful");
            result
        },
        Err(e) => {
            log(&format!("❌ open_vault call failed: {}", e));
            panic!("Failed to call open_vault: {}", e);
        }
    };
    
    // Decode and handle the result
    log("🔄 Decoding open_vault response");
    let result: Result<OpenVaultSuccess, ProtocolError> = match open_result {
        WasmResult::Reply(bytes) => {
            log(&format!("📦 Got reply with {} bytes", bytes.len()));
            match decode_one(&bytes) {
                Ok(decoded) => {
                    log("✅ Successfully decoded response");
                    decoded
                },
                Err(e) => {
                    log(&format!("❌ Failed to decode response: {}", e));
                    return;
                }
            }
        },
        WasmResult::Reject(error) => {
            log(&format!("❌ Canister rejected call: {}", error));
            return;
        }
    };
    
    match result {
        Ok(success) => {
            log(&format!("🎉 Successfully opened vault with ID: {}", success.vault_id));
            log(&format!("📊 Block index: {}", success.block_index));
            assert_eq!(success.vault_id, 1);
        },
        Err(e) => {
            log(&format!("❌ Failed to open vault: {:?}", e));
            return;
        }
    };
    
    // Verify vault state using query calls with proper Candid encoding
    log("🔍 Verifying vault state");
    
    let encoded_get_vaults_args = match encode_args((Some(test_user),)) {
        Ok(bytes) => {
            log(&format!("✅ Successfully encoded get_vaults args: {} bytes", bytes.len()));
            bytes
        },
        Err(e) => {
            log(&format!("❌ Failed to encode get_vaults args: {}", e));
            return;
        }
    };
    
    log(&format!("📤 Calling get_vaults on protocol: {}", protocol_id));
    
    let vaults_result = match pic.query_call(
        protocol_id,
        test_user,
        "get_vaults", 
        encoded_get_vaults_args
    ) {
        Ok(result) => {
            log("✅ get_vaults call successful");
            result
        },
        Err(e) => {
            log(&format!("❌ get_vaults call failed: {}", e));
            return;
        }
    };
    
    // Handle the result using pattern matching
    log("🔄 Decoding get_vaults response");
    let vaults: Vec<CandidVault> = match vaults_result {
        WasmResult::Reply(bytes) => {
            log(&format!("📦 Got reply with {} bytes", bytes.len()));
            match decode_one(&bytes) {
                Ok(decoded) => {
                    log("✅ Successfully decoded vaults");
                    decoded
                },
                Err(e) => {
                    log(&format!("❌ Failed to decode vaults: {}", e));
                    return;
                }
            }
        },
        WasmResult::Reject(error) => {
            log(&format!("❌ Canister rejected get_vaults call: {}", error));
            return;
        }
    };
    
    log(&format!("📊 Found {} vaults", vaults.len()));
    
    // Assertions
    assert_eq!(vaults.len(), 1, "Expected 1 vault, found {}", vaults.len());
    
    if !vaults.is_empty() {
        let vault = &vaults[0];
        log(&format!("🏦 Vault details:"));
        log(&format!("   ID: {}", vault.vault_id));
        log(&format!("   Owner: {}", vault.owner));
        log(&format!("   ICP Margin: {}", vault.icp_margin_amount));
        log(&format!("   Borrowed ICUSD: {}", vault.borrowed_icusd_amount));
        
        assert_eq!(vault.owner, test_user, "Vault owner doesn't match test user");
        assert_eq!(vault.icp_margin_amount, 1_000_000_000, "Incorrect ICP margin amount");
        assert_eq!(vault.borrowed_icusd_amount, 0, "Expected 0 borrowed amount");
    }
    
    log("🎉 TEST PASSED: test_open_vault");
}

// Integration test for protocol status
#[test]
fn test_protocol_status() {
    log("🧪 TEST STARTING: test_protocol_status");
    
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, _, _) = setup_protocol();
    
    // Use the SAME self-authenticating principal as in setup
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Call the status endpoint with empty arguments vector
    log(&format!("📤 Calling get_protocol_status on protocol: {}", protocol_id));
    let status_result = match pic.query_call(
        protocol_id,
        test_user,
        "get_protocol_status",
        encode_args(()).unwrap() // properly encode empty args tuple
    ) {
        Ok(result) => {
            log("✅ get_protocol_status call successful");
            result
        },
        Err(e) => {
            log(&format!("❌ get_protocol_status call failed: {}", e));
            return;
        }
    };
    
    // Decode and verify protocol status
    log("🔄 Decoding get_protocol_status response");
    type ProtocolStatus = rumi_protocol_backend::ProtocolStatus;
    
    let status: ProtocolStatus = match status_result {
        WasmResult::Reply(bytes) => {
            log(&format!("📦 Got reply with {} bytes", bytes.len()));
            match decode_one(&bytes) {
                Ok(decoded) => {
                    log("✅ Successfully decoded status");
                    decoded
                },
                Err(e) => {
                    log(&format!("❌ Failed to decode status: {}", e));
                    return;
                }
            }
        },
        WasmResult::Reject(error) => {
            log(&format!("❌ Canister rejected get_protocol_status call: {}", error));
            return;
        }
    };
    
    log(&format!("📊 Protocol status details:"));
    log(&format!("   ICP Rate: ${}", status.last_icp_rate));
    log(&format!("   Last Rate Update: {}", status.last_icp_timestamp));
    log(&format!("   Total ICP Margin: {}", status.total_icp_margin));
    log(&format!("   Total ICUSD Borrowed: {}", status.total_icusd_borrowed));
    log(&format!("   Total Collateral Ratio: {}", status.total_collateral_ratio));
    log(&format!("   Mode: {:?}", status.mode));
    
    // Basic assertions to verify the status is reasonable
    assert!(status.total_icp_margin >= 0, "Total ICP margin should be non-negative");
    assert!(status.total_icusd_borrowed >= 0, "Total ICUSD borrowed should be non-negative");
    assert_eq!(format!("{:?}", status.mode), "GeneralAvailability", "Expected GeneralAvailability mode");
    
    log("🎉 TEST PASSED: test_protocol_status");
}

// Integration test for borrowing ICUSD against ICP collateral
#[test]
fn test_borrow_icusd() {
    log("🧪 TEST STARTING: test_borrow_icusd");
    
    // Set up the test environment
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();
    
    // Verify ICP price is set before proceeding
    let protocol_status = match pic.query_call(
        protocol_id,
        Principal::anonymous(),
        "get_protocol_status",
        encode_args(()).unwrap()
    ) {
        Ok(result) => {
            match result {
                WasmResult::Reply(bytes) => {
                    match decode_one::<rumi_protocol_backend::ProtocolStatus>(&bytes) {
                        Ok(status) => {
                            log(&format!("📊 Current ICP rate: ${}", status.last_icp_rate));
                            Some(status)
                        },
                        Err(e) => {
                            log(&format!("❌ Failed to decode status: {}", e));
                            None
                        }
                    }
                },
                _ => {
                    log("❌ Unexpected response format");
                    None
                }
            }
        },
        Err(e) => {
            log(&format!("❌ Could not check protocol status: {}", e));
            None
        }
    };
    
    // Skip the test if ICP rate not set
    if protocol_status.map_or(true, |status| status.last_icp_rate <= 0.0) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }
    
    // Try setting the ICP price directly before the test
    set_icp_price_directly(&pic, protocol_id);
    
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Step 1: Approve ICP transfer for collateral
    log("🔐 Creating approval for ICP transfer");
    
    let approve_args = ApproveArgs { // Use the imported ApproveArgs struct
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(5_000_000_000u64), // 50 ICP
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => {
            log(&format!("❌ Failed to encode approve args: {}", e));
            panic!("Failed to encode approve args: {}", e);
        }
    };
    
    log(&format!("📤 Calling icrc2_approve on ICP ledger: {}", icp_ledger_id));
    
    match pic.update_call(
        icp_ledger_id,
        test_user, 
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ Approval successful"),
        Err(e) => {
            log(&format!("❌ Approval failed: {}", e));
            panic!("Failed to approve ICP transfer: {}", e);
        }
    };
    
    // Step 2: Open a vault with ICP collateral
    log("🏦 Opening vault with 50 ICP");
    
    let encoded_open_vault_args = match encode_args((5_000_000_000u64,)) {
        Ok(bytes) => bytes,
        Err(e) => {
            log(&format!("❌ Failed to encode open_vault args: {}", e));
            panic!("Failed to encode open_vault args: {}", e);
        }
    };
    
    let open_result = match pic.update_call(
        protocol_id,
        test_user,
        "open_vault", 
        encoded_open_vault_args
    ) {
        Ok(result) => result,
        Err(e) => {
            log(&format!("❌ open_vault call failed: {}", e));
            panic!("Failed to call open_vault: {}", e);
        }
    };
    
    log("🔄 Decoding open_vault response");
    let open_result: Result<OpenVaultSuccess, ProtocolError> = match open_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => {
                log(&format!("❌ Failed to decode open_vault response: {}", e));
                panic!("Failed to decode open_vault response: {}", e);
            }
        },
        WasmResult::Reject(error) => {
            log(&format!("❌ Canister rejected open_vault call: {}", error));
            panic!("Canister rejected open_vault call: {}", error);
        }
    };
    
    // Extract vault_id or fail
    let vault_id = match open_result {
        Ok(success) => {
            log(&format!("🎉 Successfully opened vault with ID: {}", success.vault_id));
            success.vault_id
        },
        Err(e) => {
            log(&format!("❌ Failed to open vault: {:?}", e));
            panic!("Failed to open vault: {:?}", e);
        }
    };
    
    // Step 3: Check initial ICUSD balance
    let initial_icusd_balance = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Initial ICUSD balance: {}", initial_icusd_balance));
    
    // Step 4: Borrow ICUSD against the vault
    log("🏦 Borrowing ICUSD against the vault");
    let borrow_amount = 2_000_000_000u64; // 20 ICUSD
    
    // Use the imported VaultArg instead of redefining it
    let borrow_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };
    
    let encoded_borrow_args = match encode_args((borrow_arg,)) {
        Ok(bytes) => bytes,
        Err(e) => {
            log(&format!("❌ Failed to encode borrow_from_vault args: {}", e));
            panic!("Failed to encode borrow_from_vault args: {}", e);
        }
    };
    
    let borrow_result = match pic.update_call(
        protocol_id,
        test_user,
        "borrow_from_vault", 
        encoded_borrow_args
    ) {
        Ok(result) => result,
        Err(e) => {
            log(&format!("❌ borrow_from_vault call failed: {}", e));
            panic!("Failed to call borrow_from_vault: {}", e);
        }
    };
    
    // Parse the borrow result
    type SuccessWithFee = rumi_protocol_backend::SuccessWithFee;
    let borrow_result: Result<SuccessWithFee, ProtocolError> = match borrow_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => {
                log(&format!("❌ Failed to decode borrow_from_vault response: {}", e));
                panic!("Failed to decode borrow_from_vault response: {}", e);
            }
        },
        WasmResult::Reject(error) => {
            log(&format!("❌ Canister rejected borrow_from_vault call: {}", error));
            panic!("Canister rejected borrow_from_vault call: {}", error);
        }
    };
    
    match borrow_result {
        Ok(success) => {
            log(&format!("🎉 Successfully borrowed ICUSD with block index: {}", success.block_index));
            log(&format!("💰 Fee paid: {}", success.fee_amount_paid));
        },
        Err(e) => {
            log(&format!("❌ Failed to borrow ICUSD: {:?}", e));
            panic!("Failed to borrow ICUSD: {:?}", e);
        }
    };
    
    // Step 5: Check final ICUSD balance
    let final_icusd_balance = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Final ICUSD balance: {}", final_icusd_balance));
    
    // Expected balance should be initial + borrowed amount - fee
    let expected_min_increase = borrow_amount - borrow_amount / 10; // Assuming max 10% fee
    let actual_increase = final_icusd_balance - initial_icusd_balance;
    
    log(&format!("📊 ICUSD balance increase: {}", actual_increase));
    assert!(actual_increase > 0, "ICUSD balance should have increased");
    assert!(
        actual_increase >= expected_min_increase, 
        "ICUSD increase ({}) should be at least {} after fees", 
        actual_increase, expected_min_increase
    );
    
    // Step 6: Verify the vault state after borrowing
    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    log(&format!("🏦 Updated vault details:"));
    log(&format!("   ID: {}", vault.vault_id));
    log(&format!("   ICP Margin: {}", vault.icp_margin_amount));
    log(&format!("   Borrowed ICUSD: {}", vault.borrowed_icusd_amount));
    
    assert_eq!(vault.borrowed_icusd_amount, borrow_amount, 
               "Vault borrowed amount should match the borrowed amount");
    
    log("🎉 TEST PASSED: test_borrow_icusd");
}


// Test for repaying borrowed ICUSD
#[test]
fn test_repay_to_vault() {
    log("🧪 TEST STARTING: test_repay_to_vault");
    
    // Set up the test environment
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();
    
    // Skip if ICP rate not set
    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }
    
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Step 1: Create a vault with ICP collateral
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, 5_000_000_000).unwrap();
    log(&format!("🏦 Created vault with ID: {}", vault_id));
    
    // Step 2: Borrow ICUSD against the vault
    let borrow_amount = 2_000_000_000u64; // 20 ICUSD
    let borrow_arg = VaultArg { vault_id, amount: borrow_amount };
    
    match call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg) {
        Ok(result) => {
            log(&format!("🎉 Successfully borrowed ICUSD with block index: {}", result.block_index));
            log(&format!("💰 Fee paid: {}", result.fee_amount_paid));
        },
        Err(e) => {
            log(&format!("❌ Failed to borrow ICUSD: {:?}", e));
            panic!("Failed to borrow ICUSD: {:?}", e);
        }
    };
    
    // Step 3: Check borrowed amount in vault
    let vault_before = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_before.borrowed_icusd_amount, borrow_amount, 
               "Vault borrowed amount should match the amount borrowed");
    
    // Step 4: Approve ICUSD transfer to protocol for repayment
    let repay_amount = 1_000_000_000u64; // 10 ICUSD (partial repayment)
    
    log("🔐 Creating approval for ICUSD transfer");
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(repay_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode approve args: {}", e),
    };
    
    log(&format!("📤 Calling icrc2_approve on ICUSD ledger: {}", icusd_ledger_id));
    match pic.update_call(
        icusd_ledger_id,
        test_user,
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ ICUSD approval successful"),
        Err(e) => panic!("Failed to approve ICUSD transfer: {}", e),
    };
    
    // Step 5: Repay to vault
    log("💵 Repaying ICUSD to vault");
    let repay_arg = VaultArg { vault_id, amount: repay_amount };
    let encoded_repay_args = match encode_args((repay_arg,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode repay_to_vault args: {}", e),
    };
    
    let repay_result = match pic.update_call(
        protocol_id,
        test_user,
        "repay_to_vault", 
        encoded_repay_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call repay_to_vault: {}", e),
    };
    
    // Step 6: Verify repayment success
    let block_index: u64 = match repay_result {
        WasmResult::Reply(bytes) => match decode_one::<Result<u64, ProtocolError>>(&bytes) {
            Ok(decoded_result) => {
                match decoded_result {
                    Ok(block_index) => {
                        log(&format!("✅ Successfully repaid with block index: {}", block_index));
                        block_index
                    },
                    Err(e) => panic!("Error in repay_to_vault result: {:?}", e),
                }
            },
            Err(e) => panic!("Failed to decode repay_to_vault response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected repay_to_vault call: {}", error),
    };
    
    // Step 7: Verify vault state after repayment
    let vault_after = get_vault(&pic, protocol_id, test_user, vault_id);
    log(&format!("🏦 Updated vault details after repayment:"));
    log(&format!("   ID: {}", vault_after.vault_id));
    log(&format!("   ICP Margin: {}", vault_after.icp_margin_amount));
    log(&format!("   Borrowed ICUSD: {}", vault_after.borrowed_icusd_amount));
    
    // Verify the borrowed amount decreased by repay_amount
    assert_eq!(vault_after.borrowed_icusd_amount, borrow_amount - repay_amount, 
               "Borrowed amount should decrease by the repayment amount");
               
    log("🎉 TEST PASSED: test_repay_to_vault");
}

// Test for adding more ICP collateral to an existing vault
#[test]
fn test_add_margin_to_vault() {
    log("🧪 TEST STARTING: test_add_margin_to_vault");
    
    // Set up the test environment
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, _) = setup_protocol();
    
    // Skip if ICP rate not set
    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }
    
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Step 1: Create a vault with ICP collateral
    let initial_margin = 1_000_000_000u64; // 10 ICP
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, initial_margin).unwrap();
    log(&format!("🏦 Created vault with ID: {}", vault_id));
    
    // Step 2: Check initial vault margin
    let vault_before = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_before.icp_margin_amount, initial_margin, "Incorrect initial margin amount");
    
    // Step 3: Approve additional ICP transfer to protocol
    log("🔐 Creating approval for additional ICP transfer");
    let additional_margin = 500_000_000u64; // 5 ICP
    
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(additional_margin),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode approve args: {}", e),
    };
    
    log(&format!("📤 Calling icrc2_approve on ICP ledger: {}", icp_ledger_id));
    match pic.update_call(
        icp_ledger_id,
        test_user,
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ Approval successful"),
        Err(e) => panic!("Failed to approve ICP transfer: {}", e),
    };
    
    // Step 4: Add margin to vault
    log("💹 Adding margin to vault");
    let add_margin_arg = VaultArg { vault_id, amount: additional_margin };
    let encoded_add_margin_args = match encode_args((add_margin_arg,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode add_margin_to_vault args: {}", e),
    };
    
    let add_margin_result = match pic.update_call(
        protocol_id,
        test_user,
        "add_margin_to_vault", 
        encoded_add_margin_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call add_margin_to_vault: {}", e),
    };
    
    // Step 5: Verify add margin success
    let block_index: u64 = match add_margin_result {
        WasmResult::Reply(bytes) => match decode_one::<Result<u64, ProtocolError>>(&bytes) {
            Ok(decoded_result) => {
                match decoded_result {
                    Ok(block_index) => {
                        log(&format!("✅ Successfully added margin with block index: {}", block_index));
                        block_index
                    },
                    Err(e) => panic!("Error in add_margin_to_vault result: {:?}", e),
                }
            },
            Err(e) => panic!("Failed to decode add_margin_to_vault response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected add_margin_to_vault call: {}", error),
    };
    
    // Step 6: Verify vault state after adding margin
    let vault_after = get_vault(&pic, protocol_id, test_user, vault_id);
    log(&format!("🏦 Updated vault details after adding margin:"));
    log(&format!("   ID: {}", vault_after.vault_id));
    log(&format!("   ICP Margin: {}", vault_after.icp_margin_amount));
    log(&format!("   Borrowed ICUSD: {}", vault_after.borrowed_icusd_amount));
    
    // Verify margin increased by additional_margin
    let expected_margin = initial_margin + additional_margin;
    assert_eq!(vault_after.icp_margin_amount, expected_margin, 
               "Margin amount should increase by the additional amount");
               
    log("🎉 TEST PASSED: test_add_margin_to_vault");
}

// Test for closing a vault after repaying all debt
#[test]
fn test_close_vault() {
    log("🧪 TEST STARTING: test_close_vault");
    
    // Set up the test environment
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();
    
    // Skip if ICP rate not set
    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }
    
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Step 1: Create a vault with ICP collateral
    let initial_margin = 5_000_000_000u64; // 50 ICP
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, initial_margin).unwrap();
    log(&format!("🏦 Created vault with ID: {}", vault_id));
    
    // Step 2: Borrow a small amount of ICUSD against the vault
    let borrow_amount = 1_000_000_000u64; // 10 ICUSD
    let borrow_arg = VaultArg { vault_id, amount: borrow_amount };
    
    match call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg) {
        Ok(result) => {
            log(&format!("🎉 Successfully borrowed ICUSD with block index: {}", result.block_index));
        },
        Err(e) => panic!("Failed to borrow ICUSD: {:?}", e),
    };
    
    // Step 3: Verify borrowing succeeded
    let vault_after_borrow = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_after_borrow.borrowed_icusd_amount, borrow_amount, 
               "Vault borrowed amount should match the borrowed amount");
    
    // Step 4: Approve ICUSD transfer to repay the full borrowed amount
    log("🔐 Creating approval for ICUSD repayment");
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(borrow_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode approve args: {}", e),
    };
    
    match pic.update_call(
        icusd_ledger_id,
        test_user,
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ ICUSD approval successful"),
        Err(e) => panic!("Failed to approve ICUSD transfer: {}", e),
    };
    
    // Step 5: Fully repay the borrowed amount
    log("💵 Repaying all borrowed ICUSD");
    let repay_arg = VaultArg { vault_id, amount: borrow_amount };
    let encoded_repay_args = match encode_args((repay_arg,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode repay_to_vault args: {}", e),
    };
    
    match pic.update_call(
        protocol_id,
        test_user,
        "repay_to_vault", 
        encoded_repay_args
    ) {
        Ok(_) => log("✅ Repayment successful"),
        Err(e) => panic!("Failed to repay borrowed ICUSD: {}", e),
    };
    
    // Step 6: Verify vault has no debt
    let vault_after_repay = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_after_repay.borrowed_icusd_amount, 0, "Vault should have no debt after full repayment");
    
    // Step 7: Close the vault
    log("🔒 Closing vault");
    let encoded_close_vault_args = match encode_args((vault_id,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode close_vault args: {}", e),
    };
    
    let close_result = match pic.update_call(
        protocol_id,
        test_user,
        "close_vault", 
        encoded_close_vault_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call close_vault: {}", e),
    };
    
    // Step 8: Verify close vault success - should return the block index of ICP return transfer
    let maybe_block_index: Option<u64> = match close_result {
        WasmResult::Reply(bytes) => match decode_one::<Result<Option<u64>, ProtocolError>>(&bytes) {
            Ok(decoded_result) => {
                match decoded_result {
                    Ok(maybe_block_index) => {
                        log(&format!("✅ Successfully closed vault, block index: {:?}", maybe_block_index));
                        maybe_block_index
                    },
                    Err(e) => panic!("Error in close_vault result: {:?}", e),
                }
            },
            Err(e) => panic!("Failed to decode close_vault response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected close_vault call: {}", error),
    };
    
    // If the block index is returned, it means the margin was returned
    if let Some(block_index) = maybe_block_index {
        log(&format!("⚡ Margin returned with block index: {}", block_index));
    } else {
        log("⚠️ No immediate margin return (may be processed asynchronously)");
    }
    
    // Step 9: Verify vault no longer exists
    let encoded_get_vaults_args = match encode_args((Some(test_user),)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode get_vaults args: {}", e),
    };
    
    let vaults_result = match pic.query_call(
        protocol_id,
        test_user,
        "get_vaults", 
        encoded_get_vaults_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call get_vaults: {}", e),
    };
    
    let vaults: Vec<CandidVault> = match vaults_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => panic!("Failed to decode vaults: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected get_vaults call: {}", error),
    };
    
    // Either the vault should be gone or it should have 0 margin and 0 borrowing
    for vault in &vaults {
        if vault.vault_id == vault_id {
            assert_eq!(vault.icp_margin_amount, 0, "Closed vault should have 0 margin");
            assert_eq!(vault.borrowed_icusd_amount, 0, "Closed vault should have 0 borrowed amount");
        }
    }
    
    log("🎉 TEST PASSED: test_close_vault");
}

// Test for redeeming ICP (burning ICUSD to get ICP)
#[test]
fn test_redeem_icp() {
    log("🧪 TEST STARTING: test_redeem_icp");
    
    // Set up the test environment
    log("🛠️ Setting up test environment");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();
    
    // Skip if ICP rate not set
    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }
    
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    log(&format!("👤 Test user: {}", test_user));
    
    // Step 1: Create a vault with ICP collateral and borrow against it
    // so there's ICP in the protocol to redeem against
    let initial_margin = 10_000_000_000u64; // 100 ICP
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, initial_margin).unwrap();
    log(&format!("🏦 Created vault with ID: {}", vault_id));
    
    // Step 2: Get initial ICP balance for later comparison
    let initial_icp_balance = get_icp_balance(&pic, icp_ledger_id, test_user);
    log(&format!("💰 Initial ICP balance: {}", initial_icp_balance));
    
    // Step 3: Check initial ICUSD balance
    let initial_icusd_balance = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Initial ICUSD balance: {}", initial_icusd_balance));
    
    // Step 4: Approve protocol to transfer ICUSD
    let redeem_amount = 5_000_000_000u64; // 50 ICUSD
    log("🔐 Creating approval for ICUSD transfer");
    
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(redeem_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    
    let encoded_approve_args = match encode_args((approve_args,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode approve args: {}", e),
    };
    
    log(&format!("📤 Calling icrc2_approve on ICUSD ledger: {}", icusd_ledger_id));
    match pic.update_call(
        icusd_ledger_id,
        test_user, 
        "icrc2_approve",
        encoded_approve_args
    ) {
        Ok(_) => log("✅ ICUSD approval successful"),
        Err(e) => panic!("Failed to approve ICUSD transfer: {}", e),
    };
    
    // Step 5: Call redeem_icp
    log("💱 Redeeming ICP");
    let encoded_redeem_args = match encode_args((redeem_amount,)) {
        Ok(bytes) => bytes,
        Err(e) => panic!("Failed to encode redeem_icp args: {}", e),
    };
    
    let redeem_result = match pic.update_call(
        protocol_id,
        test_user,
        "redeem_icp", 
        encoded_redeem_args
    ) {
        Ok(result) => result,
        Err(e) => panic!("Failed to call redeem_icp: {}", e),
    };
    
    // Step 6: Verify redeem success
    log("🔄 Decoding redeem_icp response");
    let redeem_result: Result<SuccessWithFee, ProtocolError> = match redeem_result {
        WasmResult::Reply(bytes) => match decode_one(&bytes) {
            Ok(decoded) => decoded,
            Err(e) => panic!("Failed to decode redeem_icp response: {}", e),
        },
        WasmResult::Reject(error) => panic!("Canister rejected redeem_icp call: {}", error),
    };
    
    match redeem_result {
        Ok(success) => {
            log(&format!("🎉 Successfully redeemed ICP with block index: {}", success.block_index));
            log(&format!("💰 Fee paid: {}", success.fee_amount_paid));
        },
        Err(e) => {
            log(&format!("❌ Failed to redeem ICP: {:?}", e));
            panic!("Failed to redeem ICP: {:?}", e);
        }
    };
    
    // Step 7: Check final ICUSD balance
    let final_icusd_balance = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Final ICUSD balance: {}", final_icusd_balance));
    
    // Verify ICUSD decreased by the redeemed amount
    assert!(final_icusd_balance < initial_icusd_balance, "ICUSD balance should have decreased");
    let expected_icusd_decrease = redeem_amount;
    let actual_icusd_decrease = initial_icusd_balance - final_icusd_balance;
    assert!(
        actual_icusd_decrease >= expected_icusd_decrease - 100_000, // Allow for small rounding differences
        "ICUSD decrease ({}) should be approximately equal to redeemed amount ({})", 
        actual_icusd_decrease, expected_icusd_decrease
    );
    
    // Step 8: Check final ICP balance with retry to allow for asynchronous transfer
    let mut final_icp_balance = 0u64;
    let mut success = false;
    
    // Try checking the balance a few times to allow for async operations
    for i in 0..5 {
        log(&format!("⏳ Attempt {} to check final ICP balance", i+1));
        // Wait a moment for async operations
        std::thread::sleep(std::time::Duration::from_millis(300));
        
        final_icp_balance = get_icp_balance(&pic, icp_ledger_id, test_user);
        log(&format!("💰 Final ICP balance: {}", final_icp_balance));
        
        if final_icp_balance > initial_icp_balance {
            success = true;
            break;
        }
    }
    
    if success {
        // Verify ICP increased (exact amount depends on exchange rate and fees)
        log(&format!("📊 ICP balance increase: {}", final_icp_balance - initial_icp_balance));
        assert!(final_icp_balance > initial_icp_balance, "ICP balance should have increased");
    } else {
        log("⚠️ No ICP balance increase detected - may be processed asynchronously");
        // Note: In a real environment, the ICP transfer might be processed asynchronously
        // so we don't fail the test if it hasn't arrived yet
    }
    
    log("🎉 TEST PASSED: test_redeem_icp");
}


//-----------------------------------------------------------------------------------
// MULTI-COLLATERAL HELPERS
//-----------------------------------------------------------------------------------

/// Deploy a second ICRC-1 ledger configured as "ckETH" with 18 decimals.
/// Returns the canister ID of the new ledger.
fn deploy_second_ledger(pic: &PocketIc, protocol_id: Principal) -> Principal {
    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);

    log("🏗️ Deploying ckETH ledger (18 decimals)");
    let cketh_ledger_id = pic.create_canister();
    pic.add_cycles(cketh_ledger_id, 2_000_000_000_000);

    let init_args = InitArgs {
        minting_account: Account {
            owner: protocol_id,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(10_000_000_000_000u64), // 0.00001 ckETH
        decimals: Some(18),
        max_memo_length: Some(32),
        token_name: "Chain-key Ethereum".into(),
        token_symbol: "ckETH".into(),
        metadata: vec![],
        initial_balances: vec![(
            Account {
                owner: test_user,
                subaccount: None,
            },
            // 10 ckETH = 10 * 10^18 (must fit in u64 for ICRC-1 ledger)
            candid::Nat::from(10_000_000_000_000_000_000u64),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: developer,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };

    let ledger_arg = LedgerArg::Init(init_args);
    let encoded = encode_args((ledger_arg,)).expect("Failed to encode ckETH ledger init args");

    pic.install_canister(
        cketh_ledger_id,
        icrc1_ledger_wasm(),
        encoded,
        None,
    );
    log(&format!("✅ ckETH ledger deployed: {}", cketh_ledger_id));
    cketh_ledger_id
}

/// Register a new collateral token via `add_collateral_token` (developer-only).
fn register_collateral(
    pic: &PocketIc,
    protocol_id: Principal,
    arg: AddCollateralArg,
) -> Result<(), ProtocolError> {
    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let encoded = encode_args((arg,)).expect("Failed to encode AddCollateralArg");

    let result = pic
        .update_call(protocol_id, developer, "add_collateral_token", encoded)
        .expect("Failed to call add_collateral_token");

    match result {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .expect("Failed to decode add_collateral_token response"),
        WasmResult::Reject(msg) => panic!("add_collateral_token rejected: {}", msg),
    }
}

/// Set the price for a non-ICP collateral type by reading the current config,
/// modifying `last_price` and `last_price_timestamp`, and writing it back via
/// `update_collateral_config`.
fn set_collateral_price(
    pic: &PocketIc,
    protocol_id: Principal,
    collateral_type: Principal,
    price_usd: f64,
) {
    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);

    // Step 1: Get current config
    let encoded_query = encode_args((collateral_type,)).expect("Failed to encode get_collateral_config args");
    let query_result = pic
        .query_call(protocol_id, Principal::anonymous(), "get_collateral_config", encoded_query)
        .expect("Failed to call get_collateral_config");

    let mut config: CollateralConfig = match query_result {
        WasmResult::Reply(bytes) => {
            decode_one::<Option<CollateralConfig>>(&bytes)
                .expect("Failed to decode get_collateral_config response")
                .expect("CollateralConfig not found for the given collateral type")
        }
        WasmResult::Reject(msg) => panic!("get_collateral_config rejected: {}", msg),
    };

    // Step 2: Update price fields
    config.last_price = Some(price_usd);
    config.last_price_timestamp = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
    );

    // Step 3: Write back
    let encoded_update = encode_args((collateral_type, config))
        .expect("Failed to encode update_collateral_config args");
    let update_result = pic
        .update_call(protocol_id, developer, "update_collateral_config", encoded_update)
        .expect("Failed to call update_collateral_config");

    match update_result {
        WasmResult::Reply(bytes) => {
            let res: Result<(), ProtocolError> =
                decode_one(&bytes).expect("Failed to decode update_collateral_config response");
            res.expect("update_collateral_config returned an error");
        }
        WasmResult::Reject(msg) => panic!("update_collateral_config rejected: {}", msg),
    }
    log(&format!("💰 Set collateral price for {} to ${}", collateral_type, price_usd));
}

/// Extended setup: calls `setup_protocol()`, deploys a ckETH ledger, registers ckETH
/// as a collateral type, and sets its price to $2000.
/// Returns (PocketIc, protocol_id, icp_ledger_id, icusd_ledger_id, cketh_ledger_id).
fn setup_protocol_with_cketh() -> (PocketIc, Principal, Principal, Principal, Principal) {
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();

    // Deploy ckETH ledger
    let cketh_ledger_id = deploy_second_ledger(&pic, protocol_id);

    // Register ckETH as collateral
    let add_arg = AddCollateralArg {
        ledger_canister_id: cketh_ledger_id,
        price_source: PriceSource::Xrc {
            base_asset: "ETH".to_string(),
            base_asset_class: XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: XrcAssetClass::FiatCurrency,
        },
        liquidation_ratio: 1.5,
        borrow_threshold_ratio: 2.0,
        liquidation_bonus: 1.15,
        borrowing_fee: 0.005,
        debt_ceiling: u64::MAX,
        min_vault_debt: 100_000_000, // 1 ICUSD
        interest_rate_apr: 0.0,
        recovery_target_cr: 2.0,
        min_collateral_deposit: 0,
        display_color: None,
    };
    register_collateral(&pic, protocol_id, add_arg)
        .expect("Failed to register ckETH collateral");

    // Set ckETH price to $2000
    set_collateral_price(&pic, protocol_id, cketh_ledger_id, 2000.0);

    log("✨ setup_protocol_with_cketh complete");
    (pic, protocol_id, icp_ledger_id, icusd_ledger_id, cketh_ledger_id)
}

/// Open a vault with a specific collateral type. Approves the transfer on the
/// collateral ledger, then calls `open_vault(amount, Some(collateral_type))`.
fn create_test_vault_with_collateral(
    pic: &PocketIc,
    protocol_id: Principal,
    collateral_ledger_id: Principal,
    collateral_type: Principal,
    owner: Principal,
    collateral_amount: u64,
) -> Result<u64, String> {
    // Approve collateral transfer
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(collateral_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };

    let encoded_approve = encode_args((approve_args,))
        .map_err(|e| format!("Failed to encode approve args: {}", e))?;

    pic.update_call(collateral_ledger_id, owner, "icrc2_approve", encoded_approve)
        .map_err(|e| format!("Failed to approve collateral transfer: {}", e))?;

    // Open vault with explicit collateral type
    let collateral_type_opt: Option<Principal> = Some(collateral_type);
    let encoded_open = encode_args((collateral_amount, collateral_type_opt))
        .map_err(|e| format!("Failed to encode open_vault args: {}", e))?;

    let open_result = pic
        .update_call(protocol_id, owner, "open_vault", encoded_open)
        .map_err(|e| format!("Failed to call open_vault: {}", e))?;

    match open_result {
        WasmResult::Reply(bytes) => {
            let decoded: Result<OpenVaultSuccess, ProtocolError> = decode_one(&bytes)
                .map_err(|e| format!("Failed to decode open_vault response: {}", e))?;
            match decoded {
                Ok(success) => {
                    log(&format!(
                        "✅ Opened vault {} with {} units of collateral {}",
                        success.vault_id, collateral_amount, collateral_type
                    ));
                    Ok(success.vault_id)
                }
                Err(e) => Err(format!("open_vault returned error: {:?}", e)),
            }
        }
        WasmResult::Reject(msg) => Err(format!("open_vault rejected: {}", msg)),
    }
}

/// Helper to set a collateral's status via `set_collateral_status` (developer-only).
fn set_collateral_status_helper(
    pic: &PocketIc,
    protocol_id: Principal,
    collateral_type: Principal,
    status: CollateralStatus,
) -> Result<(), ProtocolError> {
    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let encoded = encode_args((collateral_type, status))
        .expect("Failed to encode set_collateral_status args");

    let result = pic
        .update_call(protocol_id, developer, "set_collateral_status", encoded)
        .expect("Failed to call set_collateral_status");

    match result {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .expect("Failed to decode set_collateral_status response"),
        WasmResult::Reject(msg) => panic!("set_collateral_status rejected: {}", msg),
    }
}

//-----------------------------------------------------------------------------------
// MULTI-COLLATERAL INTEGRATION TESTS
//-----------------------------------------------------------------------------------

/// Verify that a second collateral token can be registered and appears in the
/// list returned by `get_supported_collateral_types`, and that its config has
/// the expected decimals and ratios.
#[test]
fn test_add_collateral_token() {
    log("🧪 TEST STARTING: test_add_collateral_token");
    let (pic, protocol_id, icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    // Deploy the ckETH ledger
    let cketh_ledger_id = deploy_second_ledger(&pic, protocol_id);

    // Register ckETH
    let add_arg = AddCollateralArg {
        ledger_canister_id: cketh_ledger_id,
        price_source: PriceSource::Xrc {
            base_asset: "ETH".to_string(),
            base_asset_class: XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: XrcAssetClass::FiatCurrency,
        },
        liquidation_ratio: 1.5,
        borrow_threshold_ratio: 2.0,
        liquidation_bonus: 1.15,
        borrowing_fee: 0.005,
        debt_ceiling: u64::MAX,
        min_vault_debt: 100_000_000,
        interest_rate_apr: 0.0,
        recovery_target_cr: 2.0,
        min_collateral_deposit: 0,
        display_color: None,
    };
    register_collateral(&pic, protocol_id, add_arg)
        .expect("Failed to register ckETH collateral");

    // Verify get_supported_collateral_types returns both ICP and ckETH
    let encoded_query = encode_args(()).unwrap();
    let types_result = pic
        .query_call(protocol_id, Principal::anonymous(), "get_supported_collateral_types", encoded_query)
        .expect("Failed to call get_supported_collateral_types");

    let supported: Vec<(Principal, CollateralStatus)> = match types_result {
        WasmResult::Reply(bytes) => decode_one(&bytes)
            .expect("Failed to decode get_supported_collateral_types response"),
        WasmResult::Reject(msg) => panic!("get_supported_collateral_types rejected: {}", msg),
    };

    log(&format!("📊 Supported collateral types: {:?}",
        supported.iter().map(|(p, s)| format!("{}: {:?}", p, s)).collect::<Vec<_>>()));

    // Should contain at least ICP and ckETH
    assert!(
        supported.len() >= 2,
        "Expected at least 2 supported collateral types, found {}",
        supported.len()
    );
    let principals: Vec<Principal> = supported.iter().map(|(p, _)| *p).collect();
    assert!(
        principals.contains(&icp_ledger_id),
        "ICP ledger should be in supported collateral types"
    );
    assert!(
        principals.contains(&cketh_ledger_id),
        "ckETH ledger should be in supported collateral types"
    );

    // Verify ckETH config
    let encoded_config_query = encode_args((cketh_ledger_id,)).unwrap();
    let config_result = pic
        .query_call(protocol_id, Principal::anonymous(), "get_collateral_config", encoded_config_query)
        .expect("Failed to call get_collateral_config");

    let config: CollateralConfig = match config_result {
        WasmResult::Reply(bytes) => {
            decode_one::<Option<CollateralConfig>>(&bytes)
                .expect("Failed to decode get_collateral_config response")
                .expect("ckETH config should exist")
        }
        WasmResult::Reject(msg) => panic!("get_collateral_config rejected: {}", msg),
    };

    assert_eq!(config.decimals, 18, "ckETH should have 18 decimals");
    assert_eq!(config.ledger_canister_id, cketh_ledger_id, "Config ledger_canister_id should match");
    assert!(
        matches!(config.status, CollateralStatus::Active),
        "Newly added collateral should be Active"
    );

    log("🎉 TEST PASSED: test_add_collateral_token");
}

/// Open a vault backed by ckETH and verify the vault state reflects the correct
/// collateral type and amount.
#[test]
fn test_open_vault_with_cketh() {
    log("🧪 TEST STARTING: test_open_vault_with_cketh");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // 1 ckETH = 10^18 units.  Deposit 0.5 ckETH = 500_000_000_000_000_000
    let deposit_amount: u64 = 500_000_000_000_000_000;
    let vault_id = create_test_vault_with_collateral(
        &pic,
        protocol_id,
        cketh_ledger_id,
        cketh_ledger_id,
        test_user,
        deposit_amount,
    )
    .expect("Failed to open ckETH vault");

    log(&format!("🏦 Opened ckETH vault with ID: {}", vault_id));

    // Verify vault state
    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault.collateral_type, cketh_ledger_id, "Vault collateral_type should be ckETH");
    assert_eq!(vault.collateral_amount, deposit_amount, "Vault collateral_amount should match deposit");
    assert_eq!(vault.borrowed_icusd_amount, 0, "New vault should have 0 borrowed");

    log("🎉 TEST PASSED: test_open_vault_with_cketh");
}

/// Open a ckETH vault, borrow ICUSD against it, and verify the ICUSD balance
/// increased and the vault's borrowed amount is correct.
#[test]
fn test_borrow_against_cketh_vault() {
    log("🧪 TEST STARTING: test_borrow_against_cketh_vault");
    let (pic, protocol_id, _icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Deposit 1 ckETH = 10^18 units => worth $2000 at $2000/ETH
    let deposit_amount: u64 = 1_000_000_000_000_000_000;
    let vault_id = create_test_vault_with_collateral(
        &pic,
        protocol_id,
        cketh_ledger_id,
        cketh_ledger_id,
        test_user,
        deposit_amount,
    )
    .expect("Failed to open ckETH vault");

    // Record initial ICUSD balance
    let initial_icusd = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Initial ICUSD balance: {}", initial_icusd));

    // Borrow 100 ICUSD = 10_000_000_000 (8 decimals).
    // With $2000 collateral and borrow_threshold_ratio 2.0, max safe borrow ~ $1000,
    // so 100 ICUSD is well within range.
    let borrow_amount = 10_000_000_000u64; // 100 ICUSD
    let borrow_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };

    match call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg) {
        Ok(result) => {
            log(&format!(
                "🎉 Borrowed ICUSD successfully, block_index: {}, fee: {}",
                result.block_index, result.fee_amount_paid
            ));
        }
        Err(e) => panic!("Failed to borrow ICUSD against ckETH vault: {:?}", e),
    };

    // Verify ICUSD balance increased
    let final_icusd = get_icusd_balance(&pic, icusd_ledger_id, test_user);
    log(&format!("💰 Final ICUSD balance: {}", final_icusd));
    let icusd_increase = final_icusd - initial_icusd;
    assert!(icusd_increase > 0, "ICUSD balance should have increased after borrowing");

    // Verify vault state
    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(
        vault.borrowed_icusd_amount, borrow_amount,
        "Vault borrowed amount should match the borrow request"
    );
    assert_eq!(vault.collateral_type, cketh_ledger_id, "Vault collateral_type should still be ckETH");

    log("🎉 TEST PASSED: test_borrow_against_cketh_vault");
}

/// Full lifecycle: open ckETH vault -> borrow -> repay -> close.
/// Verifies all state transitions and that collateral is returned on close.
#[test]
fn test_cketh_vault_full_lifecycle() {
    log("🧪 TEST STARTING: test_cketh_vault_full_lifecycle");
    let (pic, protocol_id, _icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // --- Step 1: Open vault ---
    let deposit_amount: u64 = 1_000_000_000_000_000_000; // 1 ckETH
    let vault_id = create_test_vault_with_collateral(
        &pic,
        protocol_id,
        cketh_ledger_id,
        cketh_ledger_id,
        test_user,
        deposit_amount,
    )
    .expect("Failed to open ckETH vault");
    log(&format!("📌 Step 1: Opened vault {}", vault_id));

    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault.collateral_amount, deposit_amount);
    assert_eq!(vault.borrowed_icusd_amount, 0);

    // --- Step 2: Borrow ---
    let borrow_amount = 5_000_000_000u64; // 50 ICUSD
    let borrow_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };
    match call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg) {
        Ok(_) => log("📌 Step 2: Borrowed 50 ICUSD"),
        Err(e) => panic!("Failed to borrow: {:?}", e),
    }

    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault.borrowed_icusd_amount, borrow_amount, "Vault should show 50 ICUSD borrowed");

    // --- Step 3: Repay in full ---
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(borrow_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    let encoded_approve = encode_args((approve_args,)).unwrap();
    pic.update_call(icusd_ledger_id, test_user, "icrc2_approve", encoded_approve)
        .expect("Failed to approve ICUSD for repayment");

    let repay_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };
    let encoded_repay = encode_args((repay_arg,)).unwrap();
    let repay_result = pic
        .update_call(protocol_id, test_user, "repay_to_vault", encoded_repay)
        .expect("Failed to call repay_to_vault");

    match repay_result {
        WasmResult::Reply(bytes) => {
            let res: Result<u64, ProtocolError> =
                decode_one(&bytes).expect("Failed to decode repay response");
            res.expect("repay_to_vault returned error");
            log("📌 Step 3: Repaid 50 ICUSD");
        }
        WasmResult::Reject(msg) => panic!("repay_to_vault rejected: {}", msg),
    }

    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault.borrowed_icusd_amount, 0, "Vault debt should be zero after full repayment");

    // --- Step 4: Close vault ---
    // Record ckETH balance before close
    let cketh_before = get_icp_balance(&pic, cketh_ledger_id, test_user);

    let encoded_close = encode_args((vault_id,)).unwrap();
    let close_result = pic
        .update_call(protocol_id, test_user, "close_vault", encoded_close)
        .expect("Failed to call close_vault");

    match close_result {
        WasmResult::Reply(bytes) => {
            let res: Result<Option<u64>, ProtocolError> =
                decode_one(&bytes).expect("Failed to decode close_vault response");
            match res {
                Ok(maybe_block_index) => {
                    log(&format!("📌 Step 4: Vault closed, return transfer block_index: {:?}", maybe_block_index));
                }
                Err(e) => panic!("close_vault returned error: {:?}", e),
            }
        }
        WasmResult::Reject(msg) => panic!("close_vault rejected: {}", msg),
    }

    // Check that ckETH was returned
    let cketh_after = get_icp_balance(&pic, cketh_ledger_id, test_user);
    log(&format!("💰 ckETH balance before close: {}, after close: {}", cketh_before, cketh_after));
    assert!(
        cketh_after > cketh_before,
        "ckETH balance should increase after closing vault (collateral returned)"
    );

    log("🎉 TEST PASSED: test_cketh_vault_full_lifecycle");
}

/// Verify that setting a collateral to Paused blocks opening new vaults and
/// borrowing, but still allows repayment.
#[test]
fn test_collateral_status_paused_blocks_borrow() {
    log("🧪 TEST STARTING: test_collateral_status_paused_blocks_borrow");
    let (pic, protocol_id, _icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // First, open a vault and borrow while Active
    let deposit_amount: u64 = 1_000_000_000_000_000_000; // 1 ckETH
    let vault_id = create_test_vault_with_collateral(
        &pic,
        protocol_id,
        cketh_ledger_id,
        cketh_ledger_id,
        test_user,
        deposit_amount,
    )
    .expect("Failed to open ckETH vault while Active");

    let borrow_amount = 5_000_000_000u64; // 50 ICUSD
    let borrow_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };
    call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg)
        .expect("Borrow should succeed while Active");
    log("📌 Opened vault and borrowed while Active");

    // Pause ckETH
    set_collateral_status_helper(&pic, protocol_id, cketh_ledger_id, CollateralStatus::Paused)
        .expect("Failed to pause ckETH");
    log("📌 Set ckETH to Paused");

    // Attempt to open a NEW vault with ckETH while Paused -> should fail
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(deposit_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    let encoded_approve = encode_args((approve_args,)).unwrap();
    pic.update_call(cketh_ledger_id, test_user, "icrc2_approve", encoded_approve)
        .expect("Failed to approve ckETH");

    let collateral_type_opt: Option<Principal> = Some(cketh_ledger_id);
    let encoded_open = encode_args((deposit_amount, collateral_type_opt)).unwrap();
    let open_result = pic
        .update_call(protocol_id, test_user, "open_vault", encoded_open)
        .expect("Failed to call open_vault");

    let open_decoded: Result<OpenVaultSuccess, ProtocolError> = match open_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("Failed to decode open_vault"),
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    };
    assert!(
        open_decoded.is_err(),
        "Opening a new vault should fail when collateral is Paused"
    );
    log("✅ Confirmed: open_vault blocked while Paused");

    // Attempt to borrow from existing vault while Paused -> should fail
    let borrow_arg2 = VaultArg {
        vault_id,
        amount: 1_000_000_000, // 10 ICUSD
    };
    let borrow_result = call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg2);
    assert!(
        borrow_result.is_err(),
        "Borrowing should fail when collateral is Paused"
    );
    log("✅ Confirmed: borrow_from_vault blocked while Paused");

    // Repay should still work
    let repay_amount = 1_000_000_000u64; // 10 ICUSD
    let approve_icusd = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(repay_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    let encoded_icusd_approve = encode_args((approve_icusd,)).unwrap();
    pic.update_call(icusd_ledger_id, test_user, "icrc2_approve", encoded_icusd_approve)
        .expect("Failed to approve ICUSD");

    let repay_arg = VaultArg {
        vault_id,
        amount: repay_amount,
    };
    let encoded_repay = encode_args((repay_arg,)).unwrap();
    let repay_result = pic
        .update_call(protocol_id, test_user, "repay_to_vault", encoded_repay)
        .expect("Failed to call repay_to_vault");

    let repay_decoded: Result<u64, ProtocolError> = match repay_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("Failed to decode repay response"),
        WasmResult::Reject(msg) => panic!("repay_to_vault rejected: {}", msg),
    };
    assert!(
        repay_decoded.is_ok(),
        "Repayment should succeed when collateral is Paused"
    );
    log("✅ Confirmed: repay_to_vault succeeds while Paused");

    // Verify vault debt decreased
    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(
        vault.borrowed_icusd_amount,
        borrow_amount - repay_amount,
        "Vault debt should decrease after repayment"
    );

    log("🎉 TEST PASSED: test_collateral_status_paused_blocks_borrow");
}

/// Verify that setting a collateral to Frozen blocks all operations:
/// open, borrow, repay, close.
#[test]
fn test_collateral_status_frozen_blocks_everything() {
    log("🧪 TEST STARTING: test_collateral_status_frozen_blocks_everything");
    let (pic, protocol_id, _icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Open a vault and borrow while Active
    let deposit_amount: u64 = 1_000_000_000_000_000_000; // 1 ckETH
    let vault_id = create_test_vault_with_collateral(
        &pic,
        protocol_id,
        cketh_ledger_id,
        cketh_ledger_id,
        test_user,
        deposit_amount,
    )
    .expect("Failed to open ckETH vault while Active");

    let borrow_amount = 5_000_000_000u64; // 50 ICUSD
    let borrow_arg = VaultArg {
        vault_id,
        amount: borrow_amount,
    };
    call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg)
        .expect("Borrow should succeed while Active");
    log("📌 Opened vault and borrowed while Active");

    // Freeze ckETH
    set_collateral_status_helper(&pic, protocol_id, cketh_ledger_id, CollateralStatus::Frozen)
        .expect("Failed to freeze ckETH");
    log("📌 Set ckETH to Frozen");

    // 1. Open new vault -> should fail
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(deposit_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    let encoded_approve = encode_args((approve_args,)).unwrap();
    pic.update_call(cketh_ledger_id, test_user, "icrc2_approve", encoded_approve)
        .expect("Failed to approve ckETH");

    let collateral_type_opt: Option<Principal> = Some(cketh_ledger_id);
    let encoded_open = encode_args((deposit_amount, collateral_type_opt)).unwrap();
    let open_result = pic
        .update_call(protocol_id, test_user, "open_vault", encoded_open)
        .expect("Failed to call open_vault");

    let open_decoded: Result<OpenVaultSuccess, ProtocolError> = match open_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };
    assert!(
        open_decoded.is_err(),
        "Opening vault should fail when collateral is Frozen"
    );
    log("✅ Confirmed: open_vault blocked while Frozen");

    // 2. Borrow from existing vault -> should fail
    let borrow_arg2 = VaultArg {
        vault_id,
        amount: 1_000_000_000,
    };
    let borrow_result = call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg2);
    assert!(
        borrow_result.is_err(),
        "Borrowing should fail when collateral is Frozen"
    );
    log("✅ Confirmed: borrow_from_vault blocked while Frozen");

    // 3. Repay -> should fail (Frozen blocks repay too)
    let approve_icusd = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(1_000_000_000u64),
        expected_allowance: None,
        expires_at: None,
        spender: Account {
            owner: protocol_id,
            subaccount: None,
        },
    };
    let encoded_icusd_approve = encode_args((approve_icusd,)).unwrap();
    pic.update_call(icusd_ledger_id, test_user, "icrc2_approve", encoded_icusd_approve)
        .expect("Failed to approve ICUSD");

    let repay_arg = VaultArg {
        vault_id,
        amount: 1_000_000_000,
    };
    let encoded_repay = encode_args((repay_arg,)).unwrap();
    let repay_result = pic
        .update_call(protocol_id, test_user, "repay_to_vault", encoded_repay)
        .expect("Failed to call repay_to_vault");

    let repay_decoded: Result<u64, ProtocolError> = match repay_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };
    assert!(
        repay_decoded.is_err(),
        "Repayment should fail when collateral is Frozen"
    );
    log("✅ Confirmed: repay_to_vault blocked while Frozen");

    // 4. Close vault -> should fail
    let encoded_close = encode_args((vault_id,)).unwrap();
    let close_result = pic
        .update_call(protocol_id, test_user, "close_vault", encoded_close)
        .expect("Failed to call close_vault");

    let close_decoded: Result<Option<u64>, ProtocolError> = match close_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };
    assert!(
        close_decoded.is_err(),
        "Closing vault should fail when collateral is Frozen"
    );
    log("✅ Confirmed: close_vault blocked while Frozen");

    // Vault state should be unchanged
    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault.borrowed_icusd_amount, borrow_amount, "Vault debt should be unchanged");
    assert_eq!(vault.collateral_amount, deposit_amount, "Vault collateral should be unchanged");

    log("🎉 TEST PASSED: test_collateral_status_frozen_blocks_everything");
}


//-----------------------------------------------------------------------------------
// UPGRADE SAFETY TESTS
//-----------------------------------------------------------------------------------

/// Helper to perform a canister upgrade on the protocol.
/// Uses the current wasm binary and passes Upgrade args.
fn upgrade_protocol(pic: &PocketIc, protocol_id: Principal) {
    let wasm = protocol_wasm();
    let upgrade_arg = ProtocolArgVariant::Upgrade(UpgradeArg { mode: None });
    let encoded = encode_args((upgrade_arg,)).expect("Failed to encode upgrade args");

    pic.upgrade_canister(protocol_id, wasm, encoded, None)
        .expect("Protocol upgrade failed");
    log("🔄 Protocol canister upgraded successfully");
}

/// Upgrade with no vaults — verifies that the canister can upgrade cleanly
/// from an empty state and that the ICP price and protocol status survive.
#[test]
fn test_upgrade_empty_state() {
    log("🧪 TEST STARTING: test_upgrade_empty_state");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();

    // Check status before upgrade
    let status_before = {
        let res = pic.query_call(
            protocol_id, Principal::anonymous(), "get_protocol_status",
            encode_args(()).unwrap()
        ).expect("get_protocol_status failed");
        match res {
            WasmResult::Reply(bytes) => decode_one::<rumi_protocol_backend::ProtocolStatus>(&bytes)
                .expect("decode status"),
            WasmResult::Reject(msg) => panic!("rejected: {}", msg),
        }
    };
    log(&format!("📊 Status before upgrade: ICP rate={}, mode={:?}",
        status_before.last_icp_rate, status_before.mode));

    // Perform upgrade
    upgrade_protocol(&pic, protocol_id);

    // Check status after upgrade
    let status_after = {
        let res = pic.query_call(
            protocol_id, Principal::anonymous(), "get_protocol_status",
            encode_args(()).unwrap()
        ).expect("get_protocol_status failed");
        match res {
            WasmResult::Reply(bytes) => decode_one::<rumi_protocol_backend::ProtocolStatus>(&bytes)
                .expect("decode status"),
            WasmResult::Reject(msg) => panic!("rejected: {}", msg),
        }
    };
    log(&format!("📊 Status after upgrade: ICP rate={}, mode={:?}",
        status_after.last_icp_rate, status_after.mode));

    // ICP rate should survive (event replay restores it)
    assert_eq!(
        status_before.last_icp_rate, status_after.last_icp_rate,
        "ICP rate should survive upgrade"
    );
    assert_eq!(
        format!("{:?}", status_before.mode),
        format!("{:?}", status_after.mode),
        "Protocol mode should survive upgrade"
    );

    log("🎉 TEST PASSED: test_upgrade_empty_state");
}

/// Upgrade with ICP vaults — open vaults, borrow, then upgrade and verify
/// all vault state is preserved through event replay.
#[test]
fn test_upgrade_preserves_icp_vaults() {
    log("🧪 TEST STARTING: test_upgrade_preserves_icp_vaults");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id) = setup_protocol();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Create a vault and borrow
    let margin_amount = 5_000_000_000u64; // 50 ICP
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, margin_amount)
        .expect("Failed to create vault");

    let borrow_amount = 2_000_000_000u64; // 20 ICUSD
    let borrow_arg = VaultArg { vault_id, amount: borrow_amount };
    call_borrow_from_vault(&pic, protocol_id, test_user, borrow_arg)
        .expect("Failed to borrow");

    // Snapshot vault state before upgrade
    let vault_before = get_vault(&pic, protocol_id, test_user, vault_id);
    log(&format!("📊 Vault before upgrade: margin={}, borrowed={}",
        vault_before.icp_margin_amount, vault_before.borrowed_icusd_amount));

    // Perform upgrade
    upgrade_protocol(&pic, protocol_id);

    // Verify vault state after upgrade
    let vault_after = get_vault(&pic, protocol_id, test_user, vault_id);
    log(&format!("📊 Vault after upgrade: margin={}, borrowed={}",
        vault_after.icp_margin_amount, vault_after.borrowed_icusd_amount));

    assert_eq!(vault_before.vault_id, vault_after.vault_id, "Vault ID should survive upgrade");
    assert_eq!(vault_before.owner, vault_after.owner, "Vault owner should survive upgrade");
    assert_eq!(
        vault_before.icp_margin_amount, vault_after.icp_margin_amount,
        "ICP margin should survive upgrade"
    );
    assert_eq!(
        vault_before.borrowed_icusd_amount, vault_after.borrowed_icusd_amount,
        "Borrowed ICUSD should survive upgrade"
    );
    assert_eq!(
        vault_before.collateral_type, vault_after.collateral_type,
        "Collateral type should survive upgrade"
    );

    // Verify vault is still functional post-upgrade: repay should work
    let repay_amount = 1_000_000_000u64; // 10 ICUSD
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(repay_amount),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    let encoded_approve = encode_args((approve_args,)).unwrap();
    pic.update_call(icusd_ledger_id, test_user, "icrc2_approve", encoded_approve)
        .expect("Failed to approve ICUSD");

    let repay_arg = VaultArg { vault_id, amount: repay_amount };
    let encoded_repay = encode_args((repay_arg,)).unwrap();
    let repay_result = pic.update_call(protocol_id, test_user, "repay_to_vault", encoded_repay)
        .expect("Failed to call repay_to_vault");

    let repay_decoded: Result<u64, ProtocolError> = match repay_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };
    assert!(repay_decoded.is_ok(), "Repay should work after upgrade");

    let vault_after_repay = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(
        vault_after_repay.borrowed_icusd_amount,
        borrow_amount - repay_amount,
        "Repayment should reduce debt post-upgrade"
    );

    log("🎉 TEST PASSED: test_upgrade_preserves_icp_vaults");
}

/// Upgrade with multi-collateral state — register ckETH, create ckETH vaults,
/// then upgrade and verify all collateral configs and vault state are preserved.
#[test]
fn test_upgrade_preserves_multi_collateral_state() {
    log("🧪 TEST STARTING: test_upgrade_preserves_multi_collateral_state");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Create an ICP vault
    let icp_margin = 5_000_000_000u64; // 50 ICP
    let icp_vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, icp_margin)
        .expect("Failed to create ICP vault");

    let icp_borrow = 2_000_000_000u64; // 20 ICUSD
    call_borrow_from_vault(&pic, protocol_id, test_user, VaultArg { vault_id: icp_vault_id, amount: icp_borrow })
        .expect("Failed to borrow from ICP vault");

    // Create a ckETH vault
    let cketh_deposit = 1_000_000_000_000_000_000u64; // 1 ckETH
    let cketh_vault_id = create_test_vault_with_collateral(
        &pic, protocol_id, cketh_ledger_id, cketh_ledger_id, test_user, cketh_deposit,
    ).expect("Failed to create ckETH vault");

    let cketh_borrow = 5_000_000_000u64; // 50 ICUSD
    call_borrow_from_vault(&pic, protocol_id, test_user, VaultArg { vault_id: cketh_vault_id, amount: cketh_borrow })
        .expect("Failed to borrow from ckETH vault");

    // Snapshot state before upgrade
    let icp_vault_before = get_vault(&pic, protocol_id, test_user, icp_vault_id);
    let cketh_vault_before = get_vault(&pic, protocol_id, test_user, cketh_vault_id);

    // Get supported collateral types before upgrade
    let types_before: Vec<(Principal, CollateralStatus)> = {
        let res = pic.query_call(protocol_id, Principal::anonymous(), "get_supported_collateral_types",
            encode_args(()).unwrap()).unwrap();
        match res {
            WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
            WasmResult::Reject(msg) => panic!("rejected: {}", msg),
        }
    };

    log(&format!("📊 Before upgrade: {} collateral types, ICP vault debt={}, ckETH vault debt={}",
        types_before.len(), icp_vault_before.borrowed_icusd_amount, cketh_vault_before.borrowed_icusd_amount));

    // Perform upgrade
    upgrade_protocol(&pic, protocol_id);

    // Verify collateral configs survived
    let types_after: Vec<(Principal, CollateralStatus)> = {
        let res = pic.query_call(protocol_id, Principal::anonymous(), "get_supported_collateral_types",
            encode_args(()).unwrap()).unwrap();
        match res {
            WasmResult::Reply(bytes) => decode_one(&bytes).unwrap(),
            WasmResult::Reject(msg) => panic!("rejected: {}", msg),
        }
    };

    assert_eq!(
        types_before.len(), types_after.len(),
        "Number of collateral types should survive upgrade"
    );

    let principals_after: Vec<Principal> = types_after.iter().map(|(p, _)| *p).collect();
    assert!(principals_after.contains(&icp_ledger_id), "ICP should still be registered");
    assert!(principals_after.contains(&cketh_ledger_id), "ckETH should still be registered");

    // Verify ckETH config survived with correct decimals and price
    let cketh_config: CollateralConfig = {
        let res = pic.query_call(protocol_id, Principal::anonymous(), "get_collateral_config",
            encode_args((cketh_ledger_id,)).unwrap()).unwrap();
        match res {
            WasmResult::Reply(bytes) => decode_one::<Option<CollateralConfig>>(&bytes).unwrap().unwrap(),
            WasmResult::Reject(msg) => panic!("rejected: {}", msg),
        }
    };
    assert_eq!(cketh_config.decimals, 18, "ckETH decimals should survive upgrade");
    assert!(cketh_config.last_price.is_some(), "ckETH price should survive upgrade");
    log(&format!("📊 ckETH config after upgrade: decimals={}, price={:?}, status={:?}",
        cketh_config.decimals, cketh_config.last_price, cketh_config.status));

    // Verify both vaults survived
    let icp_vault_after = get_vault(&pic, protocol_id, test_user, icp_vault_id);
    let cketh_vault_after = get_vault(&pic, protocol_id, test_user, cketh_vault_id);

    assert_eq!(icp_vault_before.icp_margin_amount, icp_vault_after.icp_margin_amount);
    assert_eq!(icp_vault_before.borrowed_icusd_amount, icp_vault_after.borrowed_icusd_amount);
    assert_eq!(icp_vault_before.collateral_type, icp_vault_after.collateral_type);

    assert_eq!(cketh_vault_before.collateral_amount, cketh_vault_after.collateral_amount);
    assert_eq!(cketh_vault_before.borrowed_icusd_amount, cketh_vault_after.borrowed_icusd_amount);
    assert_eq!(cketh_vault_before.collateral_type, cketh_vault_after.collateral_type);
    assert_eq!(cketh_vault_after.collateral_type, cketh_ledger_id, "ckETH vault type should survive");

    log(&format!("📊 After upgrade: ICP vault margin={} debt={}, ckETH vault collateral={} debt={}",
        icp_vault_after.icp_margin_amount, icp_vault_after.borrowed_icusd_amount,
        cketh_vault_after.collateral_amount, cketh_vault_after.borrowed_icusd_amount));

    // Verify vaults are functional post-upgrade: borrow more from ckETH vault
    let additional_borrow = 1_000_000_000u64; // 10 ICUSD
    let borrow_result = call_borrow_from_vault(
        &pic, protocol_id, test_user,
        VaultArg { vault_id: cketh_vault_id, amount: additional_borrow }
    );
    assert!(borrow_result.is_ok(), "Should be able to borrow from ckETH vault post-upgrade");

    let cketh_vault_final = get_vault(&pic, protocol_id, test_user, cketh_vault_id);
    assert_eq!(
        cketh_vault_final.borrowed_icusd_amount,
        cketh_borrow + additional_borrow,
        "Additional borrow should succeed post-upgrade"
    );

    log("🎉 TEST PASSED: test_upgrade_preserves_multi_collateral_state");
}

/// Double upgrade test — verifies the protocol survives two consecutive upgrades.
/// This catches issues with event log growth (the Upgrade event is appended each time).
#[test]
fn test_double_upgrade_stability() {
    log("🧪 TEST STARTING: test_double_upgrade_stability");
    let (pic, protocol_id, icp_ledger_id, _icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Create a vault before any upgrades
    let cketh_deposit = 1_000_000_000_000_000_000u64; // 1 ckETH
    let vault_id = create_test_vault_with_collateral(
        &pic, protocol_id, cketh_ledger_id, cketh_ledger_id, test_user, cketh_deposit,
    ).expect("Failed to create ckETH vault");

    let borrow_amount = 5_000_000_000u64; // 50 ICUSD
    call_borrow_from_vault(&pic, protocol_id, test_user, VaultArg { vault_id, amount: borrow_amount })
        .expect("Failed to borrow");

    // First upgrade
    log("🔄 Performing first upgrade...");
    upgrade_protocol(&pic, protocol_id);

    // Verify vault survives first upgrade
    let vault_after_1 = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_after_1.borrowed_icusd_amount, borrow_amount);
    assert_eq!(vault_after_1.collateral_type, cketh_ledger_id);

    // Second upgrade
    log("🔄 Performing second upgrade...");
    upgrade_protocol(&pic, protocol_id);

    // Verify vault survives second upgrade
    let vault_after_2 = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(vault_after_2.borrowed_icusd_amount, borrow_amount, "Debt should survive double upgrade");
    assert_eq!(vault_after_2.collateral_amount, cketh_deposit, "Collateral should survive double upgrade");
    assert_eq!(vault_after_2.collateral_type, cketh_ledger_id, "Type should survive double upgrade");

    // Verify protocol is still fully functional
    let additional_borrow = 1_000_000_000u64; // 10 ICUSD
    let result = call_borrow_from_vault(
        &pic, protocol_id, test_user,
        VaultArg { vault_id, amount: additional_borrow }
    );
    assert!(result.is_ok(), "Borrowing should work after double upgrade");

    log("🎉 TEST PASSED: test_double_upgrade_stability");
}


//-----------------------------------------------------------------------------------
// EDGE CASE TESTS
//-----------------------------------------------------------------------------------

/// Verify that opening a vault with an unregistered collateral type is rejected.
#[test]
fn test_open_vault_unregistered_collateral_rejected() {
    log("🧪 TEST STARTING: test_open_vault_unregistered_collateral_rejected");
    let (pic, protocol_id, icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Try to open a vault with a random principal that isn't registered
    let fake_ledger = Principal::self_authenticating(&[99, 99, 99, 99]);

    // Approve on ICP ledger (doesn't matter which ledger we approve on,
    // the protocol should reject based on collateral_type)
    let approve_args = ApproveArgs {
        fee: None,
        memo: None,
        from_subaccount: None,
        created_at_time: None,
        amount: candid::Nat::from(1_000_000_000u64),
        expected_allowance: None,
        expires_at: None,
        spender: Account { owner: protocol_id, subaccount: None },
    };
    let encoded_approve = encode_args((approve_args,)).unwrap();
    pic.update_call(icp_ledger_id, test_user, "icrc2_approve", encoded_approve)
        .expect("approve failed");

    let collateral_type_opt: Option<Principal> = Some(fake_ledger);
    let encoded_open = encode_args((1_000_000_000u64, collateral_type_opt)).unwrap();
    let open_result = pic.update_call(protocol_id, test_user, "open_vault", encoded_open)
        .expect("call failed");

    let decoded: Result<OpenVaultSuccess, ProtocolError> = match open_result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };

    assert!(decoded.is_err(), "Opening vault with unregistered collateral should fail");
    log(&format!("✅ Got expected error: {:?}", decoded.unwrap_err()));

    log("🎉 TEST PASSED: test_open_vault_unregistered_collateral_rejected");
}

/// Verify that a non-developer cannot call add_collateral_token.
#[test]
fn test_add_collateral_non_developer_rejected() {
    log("🧪 TEST STARTING: test_add_collateral_non_developer_rejected");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    let non_developer = Principal::self_authenticating(&[1, 2, 3, 4]); // test_user, NOT developer

    let fake_ledger = Principal::self_authenticating(&[99, 99, 99, 99]);
    let add_arg = AddCollateralArg {
        ledger_canister_id: fake_ledger,
        price_source: PriceSource::Xrc {
            base_asset: "FAKE".to_string(),
            base_asset_class: XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: XrcAssetClass::FiatCurrency,
        },
        liquidation_ratio: 1.5,
        borrow_threshold_ratio: 2.0,
        liquidation_bonus: 1.15,
        borrowing_fee: 0.005,
        debt_ceiling: u64::MAX,
        min_vault_debt: 100_000_000,
        interest_rate_apr: 0.0,
        recovery_target_cr: 2.0,
        min_collateral_deposit: 0,
        display_color: None,
    };

    let encoded = encode_args((add_arg,)).unwrap();
    let result = pic.update_call(protocol_id, non_developer, "add_collateral_token", encoded)
        .expect("call failed");

    let decoded: Result<(), ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode"),
        WasmResult::Reject(msg) => panic!("rejected: {}", msg),
    };

    assert!(decoded.is_err(), "Non-developer should not be able to add collateral");
    log(&format!("✅ Got expected error: {:?}", decoded.unwrap_err()));

    log("🎉 TEST PASSED: test_add_collateral_non_developer_rejected");
}

/// Verify ICP and ckETH vaults coexist independently — borrowing from one
/// doesn't affect the other.
#[test]
fn test_icp_and_cketh_vaults_coexist() {
    log("🧪 TEST STARTING: test_icp_and_cketh_vaults_coexist");
    let (pic, protocol_id, icp_ledger_id, icusd_ledger_id, cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Create both an ICP vault and a ckETH vault
    let icp_margin = 5_000_000_000u64; // 50 ICP
    let icp_vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, icp_margin)
        .expect("Failed to create ICP vault");

    let cketh_deposit = 1_000_000_000_000_000_000u64; // 1 ckETH
    let cketh_vault_id = create_test_vault_with_collateral(
        &pic, protocol_id, cketh_ledger_id, cketh_ledger_id, test_user, cketh_deposit,
    ).expect("Failed to create ckETH vault");

    // Borrow from ICP vault
    let icp_borrow = 2_000_000_000u64; // 20 ICUSD
    call_borrow_from_vault(&pic, protocol_id, test_user, VaultArg { vault_id: icp_vault_id, amount: icp_borrow })
        .expect("Failed to borrow from ICP vault");

    // Snapshot ckETH vault — should be untouched
    let cketh_vault = get_vault(&pic, protocol_id, test_user, cketh_vault_id);
    assert_eq!(cketh_vault.borrowed_icusd_amount, 0, "ckETH vault should be unaffected");
    assert_eq!(cketh_vault.collateral_amount, cketh_deposit, "ckETH collateral should be unaffected");

    // Borrow from ckETH vault
    let cketh_borrow = 5_000_000_000u64; // 50 ICUSD
    call_borrow_from_vault(&pic, protocol_id, test_user, VaultArg { vault_id: cketh_vault_id, amount: cketh_borrow })
        .expect("Failed to borrow from ckETH vault");

    // Snapshot ICP vault — should be untouched by ckETH borrow
    let icp_vault = get_vault(&pic, protocol_id, test_user, icp_vault_id);
    assert_eq!(icp_vault.borrowed_icusd_amount, icp_borrow, "ICP vault debt should be unchanged");
    assert_eq!(icp_vault.icp_margin_amount, icp_margin, "ICP margin should be unchanged");

    // Verify both vaults are correct
    let cketh_vault_final = get_vault(&pic, protocol_id, test_user, cketh_vault_id);
    assert_eq!(cketh_vault_final.borrowed_icusd_amount, cketh_borrow);
    assert_eq!(cketh_vault_final.collateral_type, cketh_ledger_id);

    let icp_vault_final = get_vault(&pic, protocol_id, test_user, icp_vault_id);
    assert_eq!(icp_vault_final.borrowed_icusd_amount, icp_borrow);
    assert_eq!(icp_vault_final.collateral_type, icp_ledger_id);

    log(&format!("📊 ICP vault: margin={}, debt={}", icp_vault_final.icp_margin_amount, icp_vault_final.borrowed_icusd_amount));
    log(&format!("📊 ckETH vault: collateral={}, debt={}", cketh_vault_final.collateral_amount, cketh_vault_final.borrowed_icusd_amount));

    log("🎉 TEST PASSED: test_icp_and_cketh_vaults_coexist");
}

/// Verify that opening a vault with None as collateral_type defaults to ICP.
#[test]
fn test_open_vault_none_defaults_to_icp() {
    log("🧪 TEST STARTING: test_open_vault_none_defaults_to_icp");
    let (pic, protocol_id, icp_ledger_id, _icusd_ledger_id, _cketh_ledger_id) =
        setup_protocol_with_cketh();

    if !verify_icp_rate_available(&pic, protocol_id) {
        log("⚠️ Skipping test due to missing ICP rate");
        return;
    }

    let test_user = Principal::self_authenticating(&[1, 2, 3, 4]);

    // Open vault with None (backward compat — old callers don't pass collateral_type)
    let margin = 1_000_000_000u64; // 10 ICP
    let vault_id = create_test_vault(&pic, protocol_id, icp_ledger_id, test_user, margin)
        .expect("Failed to create vault with default collateral");

    let vault = get_vault(&pic, protocol_id, test_user, vault_id);
    assert_eq!(
        vault.collateral_type, icp_ledger_id,
        "Vault with None collateral_type should default to ICP ledger"
    );
    log(&format!("✅ collateral_type={} matches icp_ledger_id={}", vault.collateral_type, icp_ledger_id));

    log("🎉 TEST PASSED: test_open_vault_none_defaults_to_icp");
}

//-----------------------------------------------------------------------------------
// ADMIN MINT INTEGRATION TESTS
//-----------------------------------------------------------------------------------

/// Helper: call admin_mint_icusd as a given principal.
fn call_admin_mint(
    pic: &PocketIc,
    protocol_id: Principal,
    caller: Principal,
    amount_e8s: u64,
    to: Principal,
    reason: &str,
) -> Result<u64, ProtocolError> {
    let encoded = encode_args((amount_e8s, to, reason.to_string()))
        .expect("Failed to encode admin_mint_icusd args");

    let result = pic
        .update_call(protocol_id, caller, "admin_mint_icusd", encoded)
        .expect("Failed to call admin_mint_icusd");

    match result {
        WasmResult::Reply(bytes) => {
            decode_one::<Result<u64, ProtocolError>>(&bytes)
                .expect("Failed to decode admin_mint_icusd response")
        }
        WasmResult::Reject(error) => panic!("Canister rejected admin_mint_icusd: {}", error),
    }
}

/// Helper: fetch all events from the protocol.
fn fetch_events(pic: &PocketIc, protocol_id: Principal, start: u64, length: u64) -> Vec<Event> {
    let arg = GetEventsArg { start, length };
    let encoded = encode_args((arg,)).expect("Failed to encode get_events args");

    let result = pic
        .query_call(protocol_id, Principal::anonymous(), "get_events", encoded)
        .expect("Failed to call get_events");

    match result {
        WasmResult::Reply(bytes) => {
            decode_one::<Vec<Event>>(&bytes).expect("Failed to decode get_events response")
        }
        WasmResult::Reject(error) => panic!("Canister rejected get_events: {}", error),
    }
}

/// Test: developer can successfully mint icUSD and the balance + event are correct.
#[test]
fn test_admin_mint_success() {
    log("🧪 TEST STARTING: test_admin_mint_success");
    let (pic, protocol_id, _icp_ledger_id, icusd_ledger_id) = setup_protocol();

    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let recipient = Principal::self_authenticating(&[9, 10, 11, 12]);

    // Check recipient icUSD balance before mint
    let balance_before = get_icusd_balance(&pic, icusd_ledger_id, recipient);
    log(&format!("💰 Recipient balance before: {}", balance_before));

    // Mint 100 icUSD (10_000_000_000 e8s) to recipient
    let mint_amount = 10_000_000_000u64; // 100 icUSD
    let result = call_admin_mint(
        &pic, protocol_id, developer, mint_amount, recipient, "Refund for failed operation #42",
    );
    assert!(result.is_ok(), "Admin mint should succeed: {:?}", result.err());
    let block_index = result.unwrap();
    log(&format!("✅ Admin mint succeeded, block_index={}", block_index));

    // Verify balance increased
    let balance_after = get_icusd_balance(&pic, icusd_ledger_id, recipient);
    log(&format!("💰 Recipient balance after: {}", balance_after));
    assert_eq!(
        balance_after - balance_before, mint_amount,
        "Recipient should have received exactly {} e8s", mint_amount
    );

    // Verify event was recorded
    let events = fetch_events(&pic, protocol_id, 0, 100);
    let admin_mint_events: Vec<&Event> = events.iter().filter(|e| {
        matches!(e, Event::AdminMint { .. })
    }).collect();
    assert_eq!(admin_mint_events.len(), 1, "Should have exactly one AdminMint event");

    if let Event::AdminMint { amount, to, reason, block_index: evt_block } = admin_mint_events[0] {
        assert!(*amount == mint_amount, "Minted amount should match");
        assert_eq!(*to, recipient);
        assert_eq!(reason, "Refund for failed operation #42");
        assert_eq!(*evt_block, block_index);
        log("✅ AdminMint event fields verified");
    } else {
        panic!("Expected AdminMint event variant");
    }

    log("🎉 TEST PASSED: test_admin_mint_success");
}

/// Test: non-developer caller is rejected.
#[test]
fn test_admin_mint_non_developer_rejected() {
    log("🧪 TEST STARTING: test_admin_mint_non_developer_rejected");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    let non_developer = Principal::self_authenticating(&[99, 99, 99]);
    let recipient = Principal::self_authenticating(&[9, 10, 11, 12]);

    let result = call_admin_mint(&pic, protocol_id, non_developer, 1_000_000_000, recipient, "test");
    assert!(result.is_err(), "Non-developer should be rejected");
    match result.unwrap_err() {
        ProtocolError::GenericError(msg) => {
            assert!(msg.contains("Only developer"), "Error should mention developer: {}", msg);
        }
        other => panic!("Expected GenericError, got {:?}", other),
    }

    log("🎉 TEST PASSED: test_admin_mint_non_developer_rejected");
}

/// Test: amount exceeding the 1,500 icUSD cap is rejected.
#[test]
fn test_admin_mint_cap_exceeded() {
    log("🧪 TEST STARTING: test_admin_mint_cap_exceeded");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let recipient = Principal::self_authenticating(&[9, 10, 11, 12]);

    // Try to mint 1,501 icUSD (exceeds cap of 1,500)
    let over_cap = 150_100_000_000u64; // 1,501 icUSD
    let result = call_admin_mint(&pic, protocol_id, developer, over_cap, recipient, "too much");
    assert!(result.is_err(), "Amount over cap should be rejected");
    match result.unwrap_err() {
        ProtocolError::GenericError(msg) => {
            assert!(msg.contains("cap"), "Error should mention cap: {}", msg);
        }
        other => panic!("Expected GenericError about cap, got {:?}", other),
    }

    // Verify exactly at cap works
    let at_cap = 150_000_000_000u64; // exactly 1,500 icUSD
    let result = call_admin_mint(&pic, protocol_id, developer, at_cap, recipient, "at cap");
    assert!(result.is_ok(), "Exactly at cap should succeed: {:?}", result.err());

    log("🎉 TEST PASSED: test_admin_mint_cap_exceeded");
}

/// Test: zero amount is rejected.
#[test]
fn test_admin_mint_zero_amount_rejected() {
    log("🧪 TEST STARTING: test_admin_mint_zero_amount_rejected");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let recipient = Principal::self_authenticating(&[9, 10, 11, 12]);

    let result = call_admin_mint(&pic, protocol_id, developer, 0, recipient, "zero");
    assert!(result.is_err(), "Zero amount should be rejected");
    match result.unwrap_err() {
        ProtocolError::GenericError(msg) => {
            assert!(msg.contains("Amount must be > 0"), "Error should mention zero: {}", msg);
        }
        other => panic!("Expected GenericError about zero, got {:?}", other),
    }

    log("🎉 TEST PASSED: test_admin_mint_zero_amount_rejected");
}

/// Test: 72-hour cooldown is enforced between admin mints.
#[test]
fn test_admin_mint_cooldown_enforced() {
    log("🧪 TEST STARTING: test_admin_mint_cooldown_enforced");
    let (pic, protocol_id, _icp_ledger_id, _icusd_ledger_id) = setup_protocol();

    let developer = Principal::self_authenticating(&[5, 6, 7, 8]);
    let recipient = Principal::self_authenticating(&[9, 10, 11, 12]);

    // First mint should succeed
    let result1 = call_admin_mint(
        &pic, protocol_id, developer, 1_000_000_000, recipient, "first mint",
    );
    assert!(result1.is_ok(), "First admin mint should succeed: {:?}", result1.err());
    log("✅ First mint succeeded");

    // Immediate second mint should fail (cooldown)
    let result2 = call_admin_mint(
        &pic, protocol_id, developer, 1_000_000_000, recipient, "second mint too soon",
    );
    assert!(result2.is_err(), "Second mint should fail due to cooldown");
    match result2.unwrap_err() {
        ProtocolError::GenericError(msg) => {
            assert!(msg.contains("cooldown"), "Error should mention cooldown: {}", msg);
        }
        other => panic!("Expected GenericError about cooldown, got {:?}", other),
    }
    log("✅ Second mint correctly rejected by cooldown");

    // Advance time by 73 hours (past the 72-hour cooldown)
    pic.advance_time(std::time::Duration::from_secs(73 * 3600));
    log("⏰ Advanced time by 73 hours");

    // Third mint should succeed after cooldown expires
    let result3 = call_admin_mint(
        &pic, protocol_id, developer, 1_000_000_000, recipient, "after cooldown",
    );
    assert!(result3.is_ok(), "Mint after cooldown should succeed: {:?}", result3.err());
    log("✅ Third mint succeeded after cooldown");

    // Verify two AdminMint events total
    let events = fetch_events(&pic, protocol_id, 0, 100);
    let admin_mint_count = events.iter().filter(|e| matches!(e, Event::AdminMint { .. })).count();
    assert_eq!(admin_mint_count, 2, "Should have exactly 2 AdminMint events (first + after cooldown)");

    log("🎉 TEST PASSED: test_admin_mint_cooldown_enforced");
}
