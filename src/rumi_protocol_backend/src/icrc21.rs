// ICRC-21 Consent Message Support for Oisy Wallet Integration
// This module implements the ICRC-21 standard for human-readable consent messages

use candid::{CandidType, Decode, Deserialize};
use crate::vault::VaultArg;

/// Metadata about the consent message request
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageMetadata {
    /// Language tag (BCP-47) for the message, e.g., "en"
    pub language: String,
    /// Optional UTC offset in minutes for displaying timestamps
    pub utc_offset_minutes: Option<i16>,
}

/// Device specification for formatting consent messages
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum DeviceSpec {
    /// For devices with generic displays
    GenericDisplay,
    /// For devices with line-based displays (like hardware wallets)
    LineDisplay {
        /// Number of characters per line
        characters_per_line: u16,
        /// Number of lines on the display
        lines_per_page: u16,
    },
}

/// Preferences for how the consent message should be formatted
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageSpec {
    /// Metadata about the consent message request
    pub metadata: ConsentMessageMetadata,
    /// Optional device specification for formatting
    pub device_spec: Option<DeviceSpec>,
}

/// Request for a consent message
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageRequest {
    /// The name of the canister method being called
    pub method: String,
    /// The encoded arguments for the method
    pub arg: Vec<u8>,
    /// User preferences for the consent message
    pub user_preferences: ConsentMessageSpec,
}

/// A page of text for line-based displays
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct LineDisplayPage {
    /// Lines of text for this page
    pub lines: Vec<String>,
}

/// The consent message content
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsentMessage {
    /// A generic text message (Markdown supported)
    GenericDisplayMessage(String),
    /// A message formatted for line-based displays
    LineDisplayMessage { pages: Vec<LineDisplayPage> },
}

/// Successful consent message response
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentInfo {
    /// Metadata about the consent message
    pub metadata: ConsentMessageMetadata,
    /// The consent message content
    pub consent_message: ConsentMessage,
}

/// Error information for consent message failures
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ErrorInfo {
    /// Human-readable error description
    pub description: String,
}

/// Supported standards declaration
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc28TrustedOriginsResponse {
    pub trusted_origins: Vec<String>,
}

/// Result type for consent message requests
pub type Icrc21ConsentMessageResult = Result<ConsentInfo, Icrc21Error>;

/// Error types for ICRC-21
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum Icrc21Error {
    /// Generic error
    GenericError { error_code: u64, description: String },
    /// Unsupported canister call
    UnsupportedCanisterCall(ErrorInfo),
    /// Consent message unavailable
    ConsentMessageUnavailable(ErrorInfo),
}

/// Helper to format ICP amount from e8s
fn format_icp_amount(e8s: u64) -> String {
    let icp = e8s as f64 / 100_000_000.0;
    format!("{:.4} ICP", icp)
}

/// Helper to format icUSD amount from e8s
fn format_icusd_amount(e8s: u64) -> String {
    let icusd = e8s as f64 / 100_000_000.0;
    format!("{:.2} icUSD", icusd)
}

/// Helper to convert bytes to hex string for debugging
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
}

/// Try to decode a u64 from Candid bytes, handling empty args gracefully
fn try_decode_u64(arg: &[u8], _method_name: &str) -> Result<Option<u64>, String> {
    // Handle empty or minimal args - Oisy may call this before user enters a value
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    
    // Check for DIDL magic bytes - if invalid, fall back gracefully
    if arg.len() >= 4 && &arg[0..4] != b"DIDL" {
        return Ok(None);
    }
    
    // Try standard decoding - fall back to None on failure
    match Decode!(arg, u64) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(None), // Graceful fallback - return generic message
    }
}

/// Try to decode VaultArg from Candid bytes - returns None for graceful fallback
fn try_decode_vault_arg(arg: &[u8], _method_name: &str) -> Result<Option<VaultArg>, String> {
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    
    match Decode!(arg, VaultArg) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(None), // Graceful fallback - return generic message
    }
}

/// Try to decode two u64 values from Candid bytes
fn try_decode_u64_pair(arg: &[u8], _method_name: &str) -> Result<Option<(u64, u64)>, String> {
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    
    match Decode!(arg, u64, u64) {
        Ok(values) => Ok(Some(values)),
        Err(_) => Ok(None), // Graceful fallback - return generic message
    }
}

/// Generate consent message for a specific method and arguments
fn generate_consent_message(method: &str, arg: &[u8]) -> Result<String, String> {
    match method {
        "open_vault" => {
            // Decode argument: (nat64) - ICP amount in e8s
            match try_decode_u64(arg, "open_vault")? {
                Some(amount) => Ok(format!(
                    "## Create New Vault\n\n\
                    You are creating a new vault with **{}** as collateral.\n\n\
                    This will:\n\
                    - Lock your ICP in the Rumi Protocol\n\
                    - Create a new vault that you can borrow icUSD against\n\n\
                    *Minimum collateral ratio: 150%*",
                    format_icp_amount(amount)
                )),
                None => Ok(
                    "## Create New Vault\n\n\
                    You are creating a new vault in the Rumi Protocol.\n\n\
                    This will:\n\
                    - Lock your ICP as collateral\n\
                    - Create a new vault that you can borrow icUSD against\n\n\
                    *Minimum collateral ratio: 150%*".to_string()
                ),
            }
        }
        
        "add_margin_to_vault" => {
            match try_decode_vault_arg(arg, "add_margin_to_vault")? {
                Some(vault_arg) => Ok(format!(
                    "## Add Collateral to Vault\n\n\
                    You are adding **{}** to vault #{}.\n\n\
                    This will increase your collateral ratio and reduce liquidation risk.",
                    format_icp_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Add Collateral to Vault\n\n\
                    You are adding ICP collateral to your vault.\n\n\
                    This will increase your collateral ratio and reduce liquidation risk.".to_string()
                ),
            }
        }
        
        "borrow_from_vault" => {
            match try_decode_vault_arg(arg, "borrow_from_vault")? {
                Some(vault_arg) => Ok(format!(
                    "## Borrow icUSD\n\n\
                    You are borrowing **{}** from vault #{}.\n\n\
                    This will:\n\
                    - Transfer icUSD to your wallet\n\
                    - Decrease your collateral ratio\n\n\
                    *A small borrowing fee will be applied.*",
                    format_icusd_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Borrow icUSD\n\n\
                    You are borrowing icUSD from your vault.\n\n\
                    This will:\n\
                    - Transfer icUSD to your wallet\n\
                    - Decrease your collateral ratio\n\n\
                    *A small borrowing fee will be applied.*".to_string()
                ),
            }
        }
        
        "repay_to_vault" => {
            match try_decode_vault_arg(arg, "repay_to_vault")? {
                Some(vault_arg) => Ok(format!(
                    "## Repay icUSD\n\n\
                    You are repaying **{}** to vault #{}.\n\n\
                    This will:\n\
                    - Burn the icUSD from your balance\n\
                    - Increase your collateral ratio",
                    format_icusd_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Repay icUSD\n\n\
                    You are repaying icUSD to your vault.\n\n\
                    This will:\n\
                    - Burn the icUSD from your balance\n\
                    - Increase your collateral ratio".to_string()
                ),
            }
        }
        
        "close_vault" => {
            match try_decode_u64(arg, "close_vault")? {
                Some(vault_id) => Ok(format!(
                    "## Close Vault\n\n\
                    You are closing vault #{}.\n\n\
                    **Requirements:**\n\
                    - All borrowed icUSD must be repaid first\n\n\
                    Your remaining collateral will be returned to your wallet.",
                    vault_id
                )),
                None => Ok(
                    "## Close Vault\n\n\
                    You are closing your vault.\n\n\
                    **Requirements:**\n\
                    - All borrowed icUSD must be repaid first\n\n\
                    Your remaining collateral will be returned to your wallet.".to_string()
                ),
            }
        }
        
        "withdraw_collateral" => {
            match try_decode_u64(arg, "withdraw_collateral")? {
                Some(amount) => Ok(format!(
                    "## Withdraw Collateral\n\n\
                    You are withdrawing **{}** from your vault.\n\n\
                    Only collateral above the minimum ratio can be withdrawn.",
                    format_icp_amount(amount)
                )),
                None => Ok(
                    "## Withdraw Collateral\n\n\
                    You are withdrawing excess collateral from your vault.\n\n\
                    Only collateral above the minimum ratio can be withdrawn.".to_string()
                ),
            }
        }
        
        "withdraw_and_close_vault" => {
            match try_decode_u64(arg, "withdraw_and_close_vault")? {
                Some(vault_id) => Ok(format!(
                    "## Withdraw and Close Vault\n\n\
                    You are withdrawing all collateral and closing vault #{}.\n\n\
                    **Requirements:**\n\
                    - All borrowed icUSD must be repaid first\n\n\
                    All collateral will be returned to your wallet.",
                    vault_id
                )),
                None => Ok(
                    "## Withdraw and Close Vault\n\n\
                    You are withdrawing all collateral and closing your vault.\n\n\
                    **Requirements:**\n\
                    - All borrowed icUSD must be repaid first\n\n\
                    All collateral will be returned to your wallet.".to_string()
                ),
            }
        }
        
        "liquidate_vault" => {
            match try_decode_u64(arg, "liquidate_vault")? {
                Some(vault_id) => Ok(format!(
                    "## Liquidate Vault\n\n\
                    You are liquidating vault #{} which is undercollateralized.\n\n\
                    This will:\n\
                    - Use icUSD from the stability pool to cover the debt\n\
                    - Transfer the vault's collateral to liquidators\n\n\
                    *You will receive a liquidation reward.*",
                    vault_id
                )),
                None => Ok(
                    "## Liquidate Vault\n\n\
                    You are liquidating an undercollateralized vault.\n\n\
                    This will:\n\
                    - Use icUSD from the stability pool to cover the debt\n\
                    - Transfer the vault's collateral to liquidators\n\n\
                    *You will receive a liquidation reward.*".to_string()
                ),
            }
        }
        
        "liquidate_vault_partial" => {
            match try_decode_u64_pair(arg, "liquidate_vault_partial")? {
                Some((vault_id, amount)) => Ok(format!(
                    "## Partial Liquidation\n\n\
                    You are partially liquidating vault #{} for **{}**.\n\n\
                    This will:\n\
                    - Repay part of the vault's debt\n\
                    - Transfer proportional collateral to you at a discount\n\n\
                    *You will receive the collateral at a discount to market rate.*",
                    vault_id,
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Partial Liquidation\n\n\
                    You are partially liquidating an undercollateralized vault.\n\n\
                    This will:\n\
                    - Repay part of the vault's debt\n\
                    - Transfer proportional collateral to you at a discount\n\n\
                    *You will receive the collateral at a discount to market rate.*".to_string()
                ),
            }
        }
        
        "provide_liquidity" => {
            match try_decode_u64(arg, "provide_liquidity")? {
                Some(amount) => Ok(format!(
                    "## Provide Liquidity to Stability Pool\n\n\
                    You are depositing **{}** to the stability pool.\n\n\
                    Benefits:\n\
                    - Earn rewards from liquidations\n\
                    - Support the protocol's stability\n\n\
                    *You can withdraw your liquidity at any time.*",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Provide Liquidity to Stability Pool\n\n\
                    You are depositing icUSD to the stability pool.\n\n\
                    Benefits:\n\
                    - Earn rewards from liquidations\n\
                    - Support the protocol's stability\n\n\
                    *You can withdraw your liquidity at any time.*".to_string()
                ),
            }
        }
        
        "withdraw_liquidity" => {
            match try_decode_u64(arg, "withdraw_liquidity")? {
                Some(amount) => Ok(format!(
                    "## Withdraw from Stability Pool\n\n\
                    You are withdrawing **{}** from the stability pool.\n\n\
                    Your icUSD will be returned to your wallet.",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Withdraw from Stability Pool\n\n\
                    You are withdrawing icUSD from the stability pool.\n\n\
                    Your icUSD will be returned to your wallet.".to_string()
                ),
            }
        }
        
        "claim_liquidity_returns" => {
            Ok("## Claim Liquidation Rewards\n\n\
                You are claiming your accumulated liquidation rewards.\n\n\
                This will transfer all earned ICP collateral to your wallet.".to_string())
        }
        
        "redeem_icp" => {
            match try_decode_u64(arg, "redeem_icp")? {
                Some(amount) => Ok(format!(
                    "## Redeem icUSD for ICP\n\n\
                    You are redeeming **{}** for ICP.\n\n\
                    This will:\n\
                    - Burn your icUSD\n\
                    - Transfer ICP to your wallet at the current oracle rate\n\n\
                    *A small redemption fee may apply.*",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Redeem icUSD for ICP\n\n\
                    You are redeeming icUSD for ICP.\n\n\
                    This will:\n\
                    - Burn your icUSD\n\
                    - Transfer ICP to your wallet at the current oracle rate\n\n\
                    *A small redemption fee may apply.*".to_string()
                ),
            }
        }
        
        // ─── Push-deposit methods (Oisy wallet integration) ───
        "open_vault_with_deposit" => {
            match try_decode_u64(arg, "open_vault_with_deposit")? {
                Some(borrow_amount) if borrow_amount > 0 => Ok(format!(
                    "## Create Vault (Push-Deposit)\n\n\
                    You are creating a new vault using collateral you deposited to your deposit account.\n\n\
                    Requested initial borrow: **{}**\n\n\
                    This will:\n\
                    - Sweep deposited collateral into the protocol\n\
                    - Create a new vault\n\
                    - Borrow the requested icUSD amount\n\n\
                    *Minimum collateral ratio: 150%*",
                    format_icusd_amount(borrow_amount)
                )),
                _ => Ok(
                    "## Create Vault (Push-Deposit)\n\n\
                    You are creating a new vault using collateral you deposited to your deposit account.\n\n\
                    This will:\n\
                    - Sweep deposited collateral into the protocol\n\
                    - Create a new vault that you can borrow icUSD against\n\n\
                    *Minimum collateral ratio: 150%*".to_string()
                ),
            }
        }

        "add_margin_with_deposit" => {
            match try_decode_u64(arg, "add_margin_with_deposit")? {
                Some(vault_id) => Ok(format!(
                    "## Add Collateral (Push-Deposit)\n\n\
                    You are adding collateral to vault #{} using funds from your deposit account.\n\n\
                    This will sweep your deposited collateral and increase your vault's collateral ratio.",
                    vault_id
                )),
                None => Ok(
                    "## Add Collateral (Push-Deposit)\n\n\
                    You are adding collateral to your vault using funds from your deposit account.\n\n\
                    This will increase your collateral ratio and reduce liquidation risk.".to_string()
                ),
            }
        }

        "get_deposit_account" => {
            Ok("## Get Deposit Account\n\n\
                This is a read-only query that returns your deposit account address.\n\
                No funds will be moved.".to_string())
        }

        // Query methods don't need consent messages, but we handle them gracefully
        "get_fees" | "get_liquidity_status" | "get_protocol_status" |
        "get_vaults" | "get_vault_history" | "get_events" |
        "get_redemption_rate" | "get_liquidatable_vaults" | "http_request" => {
            Ok(format!(
                "## Query: {}\n\n\
                This is a read-only query that does not modify any state.",
                method
            ))
        }
        
        _ => {
            // Unknown method - provide a generic message
            Ok(format!(
                "## Rumi Protocol Action\n\n\
                You are calling the **{}** method on the Rumi Protocol.\n\n\
                *Please verify this action before approving.*",
                method
            ))
        }
    }
}

/// ICRC-21: Get consent message for a canister call
pub fn icrc21_canister_call_consent_message(
    request: ConsentMessageRequest,
) -> Icrc21ConsentMessageResult {
    // Log the incoming request for debugging
    ic_cdk::println!(
        "[ICRC21] Consent message request - method: {}, arg_len: {}, arg_hex: {}, language: {}",
        request.method,
        request.arg.len(),
        bytes_to_hex(&request.arg),
        request.user_preferences.metadata.language
    );
    
    let message = match generate_consent_message(&request.method, &request.arg) {
        Ok(msg) => {
            ic_cdk::println!("[ICRC21] Generated message successfully for method: {}", request.method);
            msg
        },
        Err(description) => {
            ic_cdk::println!("[ICRC21] Error generating message: {}", description);
            return Err(Icrc21Error::ConsentMessageUnavailable(ErrorInfo {
                description,
            }));
        }
    };

    let consent_message = match &request.user_preferences.device_spec {
        Some(DeviceSpec::LineDisplay { characters_per_line, lines_per_page }) => {
            // Format for line displays (hardware wallets)
            let chars = *characters_per_line as usize;
            let lines = *lines_per_page as usize;
            
            // Simple line breaking - split by newlines first, then wrap long lines
            let all_lines: Vec<String> = message
                .lines()
                .flat_map(|line| {
                    // Remove markdown formatting for line displays
                    let clean_line = line
                        .replace("##", "")
                        .replace("**", "")
                        .replace("*", "")
                        .trim()
                        .to_string();
                    
                    if clean_line.is_empty() {
                        vec![]
                    } else if clean_line.len() <= chars {
                        vec![clean_line]
                    } else {
                        // Word wrap
                        let mut wrapped = Vec::new();
                        let mut current_line = String::new();
                        for word in clean_line.split_whitespace() {
                            if current_line.is_empty() {
                                current_line = word.to_string();
                            } else if current_line.len() + 1 + word.len() <= chars {
                                current_line.push(' ');
                                current_line.push_str(word);
                            } else {
                                wrapped.push(current_line);
                                current_line = word.to_string();
                            }
                        }
                        if !current_line.is_empty() {
                            wrapped.push(current_line);
                        }
                        wrapped
                    }
                })
                .collect();
            
            // Split into pages
            let pages: Vec<LineDisplayPage> = all_lines
                .chunks(lines)
                .map(|chunk| LineDisplayPage {
                    lines: chunk.to_vec(),
                })
                .collect();
            
            ConsentMessage::LineDisplayMessage { pages }
        }
        _ => {
            // Generic display - use markdown
            ConsentMessage::GenericDisplayMessage(message)
        }
    };

    Ok(ConsentInfo {
        metadata: ConsentMessageMetadata {
            language: request.user_preferences.metadata.language,
            utc_offset_minutes: request.user_preferences.metadata.utc_offset_minutes,
        },
        consent_message,
    })
}

/// ICRC-28: Return trusted origins for this canister
/// This allows signers to verify which frontends are trusted
pub fn icrc28_trusted_origins() -> Icrc28TrustedOriginsResponse {
    Icrc28TrustedOriginsResponse {
        trusted_origins: vec![
            "https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io".to_string(),
            "https://tcfua-yaaaa-aaaap-qrd7q-cai.raw.icp0.io".to_string(),
            "https://rumi.finance".to_string(),
            "https://www.rumi.finance".to_string(),
            "https://rumiprotocol.io".to_string(),
            "https://www.rumiprotocol.io".to_string(),
        ],
    }
}

/// ICRC-10: Return supported standards
pub fn icrc10_supported_standards() -> Vec<StandardRecord> {
    vec![
        StandardRecord {
            name: "ICRC-21".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string(),
        },
        StandardRecord {
            name: "ICRC-28".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-28/ICRC-28.md".to_string(),
        },
    ]
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StandardRecord {
    pub name: String,
    pub url: String,
}
