// ICRC-21 Consent Message Support for Oisy Wallet Integration
// This module implements the ICRC-21 standard for human-readable consent messages

use candid::{CandidType, Decode, Deserialize, Principal};
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

/// Helper to format icUSD amount from e8s
fn format_icusd_amount(e8s: u64) -> String {
    let icusd = e8s as f64 / 100_000_000.0;
    format!("{:.2} icUSD", icusd)
}

/// Human-readable label for a collateral whose symbol is unknown (not yet
/// backfilled, or a fetch failure). We deliberately do NOT default to "ICP" —
/// that is the exact bug this module is fixing.
const UNKNOWN_COLLATERAL_LABEL: &str = "collateral";

/// Resolve `(symbol, decimals)` for a collateral from protocol state, so consent
/// messages name the ACTUAL locked token instead of assuming ICP.
///
/// `collateral_type == None` means "the caller omitted the optional collateral
/// type", which the vault methods treat as the default ICP collateral — so we
/// resolve it to the ICP config. A registered collateral whose `symbol` has not
/// been backfilled yet falls back to a generic label, never to "ICP". Decimals
/// are always taken from the collateral's own config (they are stored for every
/// collateral), so amounts are scaled correctly even for non-8-decimal tokens
/// such as ckETH (18) or XRP (6).
fn resolve_collateral_display(collateral_type: Option<Principal>) -> (String, u8) {
    crate::state::read_state(|s| {
        let ct = collateral_type.unwrap_or_else(|| s.icp_collateral_type());
        match s.get_collateral_config(&ct) {
            Some(cfg) => (
                cfg.symbol
                    .clone()
                    .unwrap_or_else(|| UNKNOWN_COLLATERAL_LABEL.to_string()),
                cfg.decimals,
            ),
            None => (UNKNOWN_COLLATERAL_LABEL.to_string(), 8),
        }
    })
}

/// Resolve `(symbol, decimals)` for the collateral backing a specific vault.
/// Used for methods (`add_margin_to_vault`, `withdraw_collateral`, ...) whose
/// argument carries only a `vault_id` and no collateral identity. Returns the
/// generic fallback if the vault is unknown (e.g. Oisy probing before submit).
fn resolve_collateral_for_vault(vault_id: u64) -> (String, u8) {
    let ct = crate::state::read_state(|s| {
        s.vault_id_to_vaults.get(&vault_id).map(|v| v.collateral_type)
    });
    match ct {
        Some(ct) => resolve_collateral_display(Some(ct)),
        None => (UNKNOWN_COLLATERAL_LABEL.to_string(), 8),
    }
}

/// Format a raw token amount (in the token's smallest unit) using the token's
/// own decimals and symbol. Trailing zeros are trimmed for readability, so e.g.
/// 400_000 drops of XRP (6 decimals) renders "0.4 XRP" and 4_000_000_000_000_000
/// wei of ckETH (18 decimals) renders "0.004 ckETH".
fn format_collateral_amount(raw: u64, decimals: u8, symbol: &str) -> String {
    let amount = raw as f64 / 10f64.powi(decimals as i32);
    // Show up to 8 fractional digits, then trim trailing zeros (and a bare dot).
    let mut s = format!("{:.8}", amount);
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{} {}", s, symbol)
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

/// Try to decode (principal, u64) for redeem_collateral — the collateral type
/// being redeemed for, and the icUSD amount in e8s.
fn try_decode_principal_u64(
    arg: &[u8],
    _method_name: &str,
) -> Result<Option<(Principal, u64)>, String> {
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    match Decode!(arg, Principal, u64) {
        Ok((ct, amount)) => Ok(Some((ct, amount))),
        Err(_) => Ok(None),
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

/// Try to decode (u64, opt principal) for open_vault — collateral amount and the
/// optional collateral type. The collateral type is what lets us name the actual
/// locked token instead of assuming ICP.
fn try_decode_u64_opt_principal(
    arg: &[u8],
    _method_name: &str,
) -> Result<Option<(u64, Option<Principal>)>, String> {
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    match Decode!(arg, u64, Option<Principal>) {
        Ok((amount, ct)) => Ok(Some((amount, ct))),
        // Fall back to a bare u64 (e.g. an older client that omits the optional).
        Err(_) => match Decode!(arg, u64) {
            Ok(amount) => Ok(Some((amount, None))),
            Err(_) => Ok(None),
        },
    }
}

/// Try to decode (u64, u64, opt principal) for open_vault_and_borrow —
/// collateral amount, borrow amount, and the optional collateral type. The
/// collateral type is preserved so the consent message names the real token.
fn try_decode_u64_u64_opt_principal(
    arg: &[u8],
    _method_name: &str,
) -> Result<Option<(u64, u64, Option<Principal>)>, String> {
    if arg.is_empty() || arg.len() < 6 {
        return Ok(None);
    }
    match Decode!(arg, u64, u64, Option<Principal>) {
        Ok((collateral, borrow, ct)) => Ok(Some((collateral, borrow, ct))),
        // Fall back to just decoding two u64s (e.g. if Oisy omits the optional).
        Err(_) => match Decode!(arg, u64, u64) {
            Ok((collateral, borrow)) => Ok(Some((collateral, borrow, None))),
            Err(_) => Ok(None),
        },
    }
}

/// Generate consent message for a specific method and arguments
fn generate_consent_message(method: &str, arg: &[u8]) -> Result<String, String> {
    match method {
        "open_vault" => {
            // Decode argument: (nat64, opt principal) — collateral amount in the
            // token's smallest unit, plus the optional collateral type.
            match try_decode_u64_opt_principal(arg, "open_vault")? {
                Some((amount, collateral_type)) => {
                    let (symbol, decimals) = resolve_collateral_display(collateral_type);
                    Ok(format!(
                        "## Create New Vault\n\n\
                        You are creating a new vault with **{}** as collateral.\n\n\
                        This will:\n\
                        - Lock your {} in the Rumi Protocol\n\
                        - Create a new vault that you can borrow icUSD against\n\n\
                        *Minimum collateral ratio: 150%*",
                        format_collateral_amount(amount, decimals, &symbol),
                        symbol
                    ))
                }
                None => Ok(
                    "## Create New Vault\n\n\
                    You are creating a new vault in the Rumi Protocol.\n\n\
                    This will:\n\
                    - Lock your chosen collateral in the Rumi Protocol\n\
                    - Create a new vault that you can borrow icUSD against\n\n\
                    *Minimum collateral ratio: 150%*".to_string()
                ),
            }
        }

        "open_vault_and_borrow" => {
            // Decode argument: (nat64, nat64, opt principal) — collateral amount,
            // borrow amount in icUSD e8s, and the optional collateral type.
            match try_decode_u64_u64_opt_principal(arg, "open_vault_and_borrow")? {
                Some((collateral, borrow, collateral_type)) if borrow > 0 => {
                    let (symbol, decimals) = resolve_collateral_display(collateral_type);
                    Ok(format!(
                        "## Create Vault & Borrow\n\n\
                        You are creating a new vault with **{}** as collateral \
                        and borrowing **{}**.\n\n\
                        This will:\n\
                        - Lock your {} in the Rumi Protocol\n\
                        - Create a new vault\n\
                        - Borrow icUSD to your wallet\n\n\
                        *A small borrowing fee will be applied. Minimum collateral ratio: 150%*",
                        format_collateral_amount(collateral, decimals, &symbol),
                        format_icusd_amount(borrow),
                        symbol
                    ))
                }
                Some((collateral, _, collateral_type)) => {
                    let (symbol, decimals) = resolve_collateral_display(collateral_type);
                    Ok(format!(
                        "## Create New Vault\n\n\
                        You are creating a new vault with **{}** as collateral.\n\n\
                        This will:\n\
                        - Lock your {} in the Rumi Protocol\n\
                        - Create a new vault that you can borrow icUSD against\n\n\
                        *Minimum collateral ratio: 150%*",
                        format_collateral_amount(collateral, decimals, &symbol),
                        symbol
                    ))
                }
                None => Ok(
                    "## Create Vault & Borrow\n\n\
                    You are creating a new vault and borrowing icUSD.\n\n\
                    This will:\n\
                    - Lock your chosen collateral in the Rumi Protocol\n\
                    - Create a new vault\n\
                    - Borrow icUSD to your wallet\n\n\
                    *A small borrowing fee will be applied. Minimum collateral ratio: 150%*".to_string()
                ),
            }
        }

        "add_margin_to_vault" => {
            match try_decode_vault_arg(arg, "add_margin_to_vault")? {
                Some(vault_arg) => {
                    let (symbol, decimals) = resolve_collateral_for_vault(vault_arg.vault_id);
                    Ok(format!(
                        "## Add Collateral to Vault\n\n\
                        You are adding **{}** to vault #{}.\n\n\
                        This will increase your collateral ratio and reduce liquidation risk.",
                        format_collateral_amount(vault_arg.amount, decimals, &symbol),
                        vault_arg.vault_id
                    ))
                }
                None => Ok(
                    "## Add Collateral to Vault\n\n\
                    You are adding collateral to your vault.\n\n\
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

        "repay_and_close_vault" => {
            match try_decode_vault_arg(arg, "repay_and_close_vault")? {
                Some(vault_arg) => Ok(format!(
                    "## Repay and Close Vault\n\n\
                    You are repaying **{}** to vault #{} and closing it.\n\n\
                    This will:\n\
                    - Burn the icUSD from your balance\n\
                    - Return all remaining collateral to your wallet\n\
                    - Remove the vault from the protocol",
                    format_icusd_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Repay and Close Vault\n\n\
                    You are repaying icUSD to your vault and closing it.\n\n\
                    This will:\n\
                    - Burn the icUSD from your balance\n\
                    - Return all remaining collateral to your wallet\n\
                    - Remove the vault from the protocol".to_string()
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
            // Argument is the VAULT ID (nat64), not an amount — this endpoint
            // withdraws all excess collateral and computes the amount itself, so
            // the consent message references the vault and its collateral token
            // rather than a (nonexistent) amount.
            match try_decode_u64(arg, "withdraw_collateral")? {
                Some(vault_id) => {
                    let (symbol, _decimals) = resolve_collateral_for_vault(vault_id);
                    Ok(format!(
                        "## Withdraw Collateral\n\n\
                        You are withdrawing excess **{}** collateral from vault #{}.\n\n\
                        Only collateral above the minimum ratio can be withdrawn.",
                        symbol,
                        vault_id
                    ))
                }
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
        
        "redeem_collateral" => {
            // Argument: (principal, nat64) — the collateral type to receive and
            // the icUSD amount to redeem. Generic, collateral-aware redemption.
            match try_decode_principal_u64(arg, "redeem_collateral")? {
                Some((collateral_type, amount)) => {
                    let (symbol, _decimals) = resolve_collateral_display(Some(collateral_type));
                    Ok(format!(
                        "## Redeem icUSD for {}\n\n\
                        You are redeeming **{}** for {}.\n\n\
                        This will:\n\
                        - Burn your icUSD\n\
                        - Transfer {} to your wallet at the current oracle rate\n\n\
                        *A small redemption fee may apply.*",
                        symbol,
                        format_icusd_amount(amount),
                        symbol,
                        symbol
                    ))
                }
                None => Ok(
                    "## Redeem icUSD for Collateral\n\n\
                    You are redeeming icUSD for collateral.\n\n\
                    This will:\n\
                    - Burn your icUSD\n\
                    - Transfer collateral to your wallet at the current oracle rate\n\n\
                    *A small redemption fee may apply.*".to_string()
                ),
            }
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
            "https://app.rumiprotocol.com".to_string(),
            "https://app.rumiprotocol.xyz".to_string(),
            "https://rumiprotocol.io".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Encode;

    // A stand-in ledger principal for decode round-trip tests (ckBTC ledger).
    fn sample_ct() -> Principal {
        Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap()
    }

    #[test]
    fn format_collateral_amount_respects_decimals_and_symbol() {
        // 8-decimal ICP
        assert_eq!(format_collateral_amount(400_000, 8, "ICP"), "0.004 ICP");
        // 6-decimal XRP (drops)
        assert_eq!(format_collateral_amount(400_000, 6, "XRP"), "0.4 XRP");
        // 18-decimal ckETH — the fixed /1e8 divisor would have understated this
        // by 10 orders of magnitude and labeled it ICP.
        assert_eq!(
            format_collateral_amount(4_000_000_000_000_000, 18, "ckETH"),
            "0.004 ckETH"
        );
        // Whole number trims the trailing dot.
        assert_eq!(format_collateral_amount(500_000_000, 8, "ICP"), "5 ICP");
        // Zero.
        assert_eq!(format_collateral_amount(0, 8, "ckXAUT"), "0 ckXAUT");
    }

    #[test]
    fn decode_open_vault_preserves_collateral_type() {
        let ct = sample_ct();
        let arg = Encode!(&1_000_000u64, &Some(ct)).unwrap();
        assert_eq!(
            try_decode_u64_opt_principal(&arg, "open_vault").unwrap(),
            Some((1_000_000u64, Some(ct)))
        );
    }

    #[test]
    fn decode_open_vault_none_collateral_type() {
        let none: Option<Principal> = None;
        let arg = Encode!(&2_000_000u64, &none).unwrap();
        assert_eq!(
            try_decode_u64_opt_principal(&arg, "open_vault").unwrap(),
            Some((2_000_000u64, None))
        );
    }

    #[test]
    fn decode_open_vault_and_borrow_preserves_collateral_type() {
        let ct = sample_ct();
        let arg = Encode!(&1_000_000u64, &500_000u64, &Some(ct)).unwrap();
        assert_eq!(
            try_decode_u64_u64_opt_principal(&arg, "open_vault_and_borrow").unwrap(),
            Some((1_000_000u64, 500_000u64, Some(ct)))
        );
    }

    #[test]
    fn decode_redeem_collateral() {
        let ct = sample_ct();
        let arg = Encode!(&ct, &750_000u64).unwrap();
        assert_eq!(
            try_decode_principal_u64(&arg, "redeem_collateral").unwrap(),
            Some((ct, 750_000u64))
        );
    }

    // The generic (empty-arg) fallbacks are what Oisy renders while the user is
    // still typing. They must never claim "ICP" for what could be any collateral
    // — that is the exact bug this module fixes.
    #[test]
    fn generic_collateral_messages_never_hardcode_icp() {
        for method in ["open_vault", "open_vault_and_borrow", "add_margin_to_vault"] {
            let msg = generate_consent_message(method, &[]).unwrap();
            assert!(
                !msg.contains("ICP"),
                "generic {method} consent message must not hardcode ICP: {msg}"
            );
        }
    }
}
