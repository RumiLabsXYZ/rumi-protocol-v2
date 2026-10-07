// ICRC-21 Consent Message Support for Oisy Wallet Integration
// This module implements the ICRC-21 standard for human-readable consent messages

use crate::vault::VaultArg;
use candid::{CandidType, Decode, Deserialize, Principal};

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
    GenericError {
        error_code: u64,
        description: String,
    },
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

fn format_token_amount_exact(raw: u64, decimals: u8, symbol: &str) -> String {
    if decimals > 38 {
        return format!("{} raw units ({} decimals) {}", raw, decimals, symbol);
    }
    let scale = 10u128.pow(u32::from(decimals));
    let whole = u128::from(raw) / scale;
    let fraction = u128::from(raw) % scale;
    if decimals == 0 || fraction == 0 {
        return format!("{} {}", whole, symbol);
    }
    let fractional = format!("{:0width$}", fraction, width = usize::from(decimals));
    format!("{}.{} {}", whole, fractional.trim_end_matches('0'), symbol)
}

fn format_icusd_amount_exact(e8s: u64) -> String {
    format_token_amount_exact(e8s, 8, "icUSD")
}

fn safe_token_symbol(symbol: &str) -> String {
    let mut safe = String::new();
    for ch in symbol.chars().take(16) {
        safe.push(if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            ch
        } else {
            '_'
        });
    }
    if safe.is_empty() {
        UNKNOWN_COLLATERAL_LABEL.to_string()
    } else {
        safe
    }
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
                safe_token_symbol(cfg.symbol.as_deref().unwrap_or(UNKNOWN_COLLATERAL_LABEL)),
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
        s.vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_type)
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

const MAX_CONSENT_METHOD_DISPLAY_CHARS: usize = 64;

fn safe_method_display(method: &str) -> String {
    let truncated = method.chars().count() > MAX_CONSENT_METHOD_DISPLAY_CHARS;
    let mut display: String = method
        .chars()
        .take(MAX_CONSENT_METHOD_DISPLAY_CHARS)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if truncated {
        display.push_str("...");
    }
    display
}

const MAX_LINE_DISPLAY_CHARS_PER_LINE: u16 = 256;
const MAX_LINE_DISPLAY_LINES_PER_PAGE: u16 = 256;
const MAX_LINE_DISPLAY_PAGES: usize = 256;
const MAX_CONSENT_VALUE_CHARS: usize = 64;

fn safe_consent_value(value: &str) -> String {
    let mut safe = String::new();
    let mut truncated = false;
    for ch in value.chars() {
        if safe.chars().count() >= MAX_CONSENT_VALUE_CHARS {
            truncated = true;
            break;
        }
        if ch.is_control()
            || is_bidi_format_control(ch)
            || matches!(ch, '*' | '`' | '_' | '[' | ']' | '(' | ')' | '<' | '>')
        {
            safe.push('_');
        } else {
            safe.push(ch);
        }
    }
    if truncated {
        safe.push_str("...");
    }
    safe
}

fn is_bidi_format_control(ch: char) -> bool {
    matches!(
        ch as u32,
        0x061C | 0x200E..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFEFF
    )
}

fn line_display_message(
    message: &str,
    requested_characters_per_line: u16,
    requested_lines_per_page: u16,
) -> Option<ConsentMessage> {
    let chars = usize::from(requested_characters_per_line.clamp(1, MAX_LINE_DISPLAY_CHARS_PER_LINE));
    let lines = usize::from(requested_lines_per_page.clamp(1, MAX_LINE_DISPLAY_LINES_PER_PAGE));

    let all_lines: Vec<String> = message
        .lines()
        .flat_map(|line| {
            // Remove markdown formatting for line displays.
            let clean_line = line
                .replace("##", "")
                .replace("**", "")
                .replace('*', "")
                .trim()
                .to_string();

            if clean_line.is_empty() {
                vec![]
            } else if clean_line.chars().count() <= chars {
                vec![clean_line]
            } else {
                let mut wrapped = Vec::new();
                let mut current_line = String::new();
                for word in clean_line.split_whitespace() {
                    if current_line.is_empty() {
                        current_line = String::new();
                    } else if current_line.chars().count() + 1 + word.chars().count() <= chars {
                        current_line.push(' ');
                    } else if !current_line.is_empty() {
                        wrapped.push(current_line);
                        current_line = String::new();
                    }
                    for ch in word.chars() {
                        if current_line.chars().count() >= chars {
                            wrapped.push(current_line);
                            current_line = String::new();
                        }
                        current_line.push(ch);
                    }
                }
                if !current_line.is_empty() {
                    wrapped.push(current_line);
                }
                wrapped
            }
        })
        .collect();

    if all_lines.len() > lines * MAX_LINE_DISPLAY_PAGES {
        return None;
    }
    let pages: Vec<LineDisplayPage> = all_lines
        .chunks(lines)
        .map(|chunk| LineDisplayPage {
            lines: chunk.to_vec(),
        })
        .collect();
    Some(ConsentMessage::LineDisplayMessage { pages })
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
                    The legacy repayment endpoint for vault #{} is disabled. The requested amount is **{}**, but this call will not pull or burn icUSD.",
                    vault_arg.vault_id,
                    format_icusd_amount(vault_arg.amount),
                )),
                None => Ok(
                    "## Repay icUSD\n\n\
                    The legacy repayment endpoint is disabled and will not pull or burn icUSD. Use the request-ID repayment endpoint.".to_string()
                ),
            }
        }

        "repay_and_close_vault" => {
            match try_decode_vault_arg(arg, "repay_and_close_vault")? {
                Some(vault_arg) => Ok(format!(
                    "## Repay and Close Vault\n\n\
                    This legacy endpoint is disabled. It would request **{}** icUSD for vault #{}, but it will not pull or burn tokens or close the vault.",
                    format_icusd_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Repay and Close Vault\n\n\
                    This legacy endpoint is disabled and will not pull or burn tokens or close the vault.".to_string()
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
                    Closing this vault removes it from the protocol without transferring collateral. All debt must be repaid and collateral withdrawn first.",
                    vault_id
                )),
                None => Ok(
                    "## Close Vault\n\n\
                    You are closing your vault.\n\n\
                    **Requirements:**\n\
                    - All borrowed icUSD must be repaid first\n\n\
                    Closing this vault removes it from the protocol without transferring collateral. All debt must be repaid and collateral withdrawn first.".to_string()
                ),
            }
        }
        
        "withdraw_collateral" => {
            // Argument is the VAULT ID (nat64), not an amount — this endpoint
            // withdraws all collateral after debt reaches zero and computes the amount itself, so
            // the consent message references the vault and its collateral token
            // rather than a (nonexistent) amount.
            match try_decode_u64(arg, "withdraw_collateral")? {
                Some(vault_id) => {
                    let (symbol, _decimals) = resolve_collateral_for_vault(vault_id);
                    Ok(format!(
                        "## Withdraw Collateral\n\n\
                        You are withdrawing all **{}** collateral from vault #{} after repaying all debt.",
                        symbol,
                        vault_id
                    ))
                }
                None => Ok(
                    "## Withdraw Collateral\n\n\
                    This call withdraws all collateral from your vault after all debt is repaid.".to_string()
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
                    The legacy liquidation endpoint for vault #{} is currently unavailable. No icUSD will be pulled and no collateral will be transferred.",
                    vault_id
                )),
                None => Ok(
                    "## Liquidate Vault\n\n\
                    This legacy liquidation endpoint is currently unavailable. No icUSD will be pulled and no collateral will be transferred.".to_string()
                ),
            }
        }
        
        "liquidate_vault_partial" => {
            match try_decode_vault_arg(arg, "liquidate_vault_partial")? {
                Some(vault_arg) => Ok(format!(
                    "## Partial liquidation\n\n\
                    This legacy endpoint is unavailable. The requested amount is **{}** icUSD for vault #{}, but no funds will be pulled or transferred.",
                    format_icusd_amount(vault_arg.amount),
                    vault_arg.vault_id
                )),
                None => Ok(
                    "## Partial Liquidation\n\n\
                    This legacy endpoint is unavailable and will not pull or transfer funds.".to_string()
                ),
            }
        }

        "partial_repay_to_vault" => match try_decode_vault_arg(arg, "partial_repay_to_vault")? {
            Some(vault_arg) => Ok(format!(
                "## Partial icUSD repayment\n\n\
                This legacy endpoint for vault #{} is disabled. The requested amount is **{}**, but this call will not pull or burn icUSD. Use the request-ID repayment endpoint.",
                vault_arg.vault_id,
                format_icusd_amount(vault_arg.amount)
            )),
            None => Ok(
                "## Partial icUSD repayment\n\n\
                This legacy endpoint is disabled and will not pull or burn icUSD. Use the request-ID repayment endpoint."
                    .to_string(),
            ),
        },

        "partial_liquidate_vault" => match try_decode_vault_arg(arg, "partial_liquidate_vault")? {
            Some(vault_arg) => Ok(format!(
                "## Partial liquidation\n\n\
                This legacy endpoint for vault #{} is unavailable. The requested amount is **{}** icUSD, but this call will not transfer funds.",
                vault_arg.vault_id,
                format_icusd_amount(vault_arg.amount)
            )),
            None => Ok(
                "## Partial liquidation\n\n\
                This legacy endpoint is unavailable and will not transfer funds."
                    .to_string(),
            ),
        },

        "withdraw_partial_collateral" => match try_decode_vault_arg(arg, "withdraw_partial_collateral")? {
            Some(vault_arg) => {
                let (symbol, decimals) = resolve_collateral_for_vault(vault_arg.vault_id);
                Ok(format!(
                    "## Withdraw collateral\n\n\
                    You are requesting withdrawal of **{}** from vault #{}. The collateral will be sent to your wallet if the request passes protocol checks.",
                    format_collateral_amount(vault_arg.amount, decimals, &symbol),
                    vault_arg.vault_id
                ))
            }
            None => Ok(
                "## Withdraw collateral\n\n\
                This call requests a partial withdrawal of collateral from your vault to your wallet."
                    .to_string(),
            ),
        },

        "repay_to_vault_with_stable" => match Decode!(arg, crate::VaultArgWithToken) {
            Ok(vault_arg) => {
                let token = match vault_arg.token_type {
                    crate::StableTokenType::CKUSDT => "ckUSDT",
                    crate::StableTokenType::CKUSDC => "ckUSDC",
                };
                Ok(format!(
                    "## Repay vault with {}\n\n\
                    This legacy endpoint is disabled. It would request {} raw units of {} from your balance for vault #{}, but this call will not pull or burn tokens.",
                    token,
                    vault_arg.amount,
                    token,
                    vault_arg.vault_id
                ))
            }
            Err(_) => Ok(
                "## Repay vault with stablecoin\n\n\
                This legacy endpoint is disabled and will not pull or burn tokens."
                    .to_string(),
            ),
        },

        "liquidate_vault_partial_with_stable" => match Decode!(arg, crate::VaultArgWithToken) {
            Ok(vault_arg) => {
                let token = match vault_arg.token_type {
                    crate::StableTokenType::CKUSDT => "ckUSDT",
                    crate::StableTokenType::CKUSDC => "ckUSDC",
                };
                Ok(format!(
                    "## Stablecoin liquidation\n\n\
                    This legacy endpoint is unavailable. It would request {} raw units of {} from your balance for vault #{}, but this call will not pull tokens.",
                    vault_arg.amount,
                    token,
                    vault_arg.vault_id
                ))
            }
            Err(_) => Ok(
                "## Stablecoin liquidation\n\n\
                This legacy endpoint is unavailable and will not pull tokens."
                    .to_string(),
            ),
        },
        
        "provide_liquidity" => {
            match try_decode_u64(arg, "provide_liquidity")? {
                Some(amount) => Ok(format!(
                    "## Deposit icUSD\n\n\
                    The legacy liquidity endpoint is disabled. It would request **{}** icUSD, but this call will not pull tokens.",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Deposit icUSD\n\n\
                    The legacy liquidity endpoint is disabled and will not pull icUSD.".to_string()
                ),
            }
        }
        
        "withdraw_liquidity" => {
            match try_decode_u64(arg, "withdraw_liquidity")? {
                Some(amount) => Ok(format!(
                    "## Withdraw icUSD\n\n\
                    The legacy liquidity endpoint is disabled. It would request **{}** icUSD, but this call will not mint or transfer tokens.",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Withdraw icUSD\n\n\
                    The legacy liquidity endpoint is disabled and will not mint or transfer icUSD.".to_string()
                ),
            }
        }
        
        "claim_liquidity_returns" => {
            Ok("## Claim Liquidity Returns\n\n\
                The legacy liquidity endpoint is disabled and will not transfer rewards.".to_string())
        }
        
        "redeem_collateral" => {
            // Argument: (principal, nat64) — the collateral type to receive and
            // the icUSD amount to redeem. Generic, collateral-aware redemption.
            match try_decode_principal_u64(arg, "redeem_collateral")? {
                Some((collateral_type, amount)) => {
                    let (symbol, _decimals) = resolve_collateral_display(Some(collateral_type));
                    Ok(format!(
                        "## Redeem icUSD for {}\n\n\
                        Redemptions are currently paused. This call will not pull or burn **{}** icUSD or transfer {}.",
                        symbol,
                        format_icusd_amount(amount),
                        symbol
                    ))
                }
                None => Ok(
                    "## Redeem icUSD for Collateral\n\n\
                    Redemptions are currently paused. This call will not pull or burn icUSD or transfer collateral.".to_string()
                ),
            }
        }

        "redeem_icp" => {
            match try_decode_u64(arg, "redeem_icp")? {
                Some(amount) => Ok(format!(
                    "## Redeem icUSD for ICP\n\n\
                    Redemptions are currently paused. This call will not pull or burn **{}** icUSD or transfer ICP.",
                    format_icusd_amount(amount)
                )),
                None => Ok(
                    "## Redeem icUSD for ICP\n\n\
                    Redemptions are currently paused. This call will not pull or burn icUSD or transfer ICP.".to_string()
                ),
            }
        }

        "redeem_quoted" => match Decode!(arg, crate::RedeemQuotedRequest) {
            Ok(request) => Ok(format!(
                "## Redeem icUSD for collateral\n\n\
                Redemptions are currently paused. This call will not pull **{}** icUSD or transfer collateral to {}. The requested minimum output is {} raw units.",
                format_icusd_amount(request.amount_e8s),
                resolve_collateral_display(Some(request.expected_collateral_type)).0,
                request.min_net_collateral_raw
            )),
            Err(_) => Ok(
                "## Redeem icUSD for collateral\n\n\
                Redemptions are currently paused. This call will not pull icUSD or transfer collateral."
                    .to_string(),
            ),
        },

        "redeem_quoted_v2" => match Decode!(arg, crate::state::RedemptionV2Request) {
            Ok(request) => {
                let (symbol, decimals) =
                    resolve_collateral_display(Some(request.expected_collateral_type));
                Ok(format!(
                    "## Redeem icUSD for collateral\n\n\
                    Request **{}** asks to pull and burn **{}** icUSD for a minimum payout of **{}**.\n\n\
                    If accepted, payout is queued to the caller's account in {}. Verify the request ID, amounts, and collateral asset.",
                    request.request_id,
                    format_icusd_amount_exact(request.amount_e8s),
                    format_token_amount_exact(
                        request.min_net_collateral_raw,
                        decimals,
                        &symbol
                    ),
                    symbol
                ))
            }
            Err(_) => Ok(
                "## Redeem icUSD for collateral\n\n\
                This request-ID redemption can pull and burn icUSD and queue collateral payout to your caller account. Verify the request ID and decoded amounts in your wallet before approving."
                    .to_string(),
            ),
        },

        "redeem_reserves" => match Decode!(arg, u64, Option<Principal>) {
            Ok((amount, preferred_token)) => {
                let token = preferred_token
                    .map(|ct| resolve_collateral_display(Some(ct)).0)
                    .unwrap_or_else(|| "an eligible reserve asset".to_string());
                Ok(format!(
                    "## Redeem reserves\n\n\
                    You are requesting redemption of **{}** icUSD for {}.\n\n\
                    This endpoint is currently paused; no icUSD will be pulled while it is unavailable.",
                    format_icusd_amount(amount),
                    token
                ))
            }
            Err(_) => Ok(
                "## Redeem reserves\n\n\
                This endpoint is currently paused; no icUSD will be pulled while it is unavailable."
                    .to_string(),
            ),
        },

        "settle_xrp_claim" => match Decode!(arg, u64, String) {
            Ok((claim_id, destination)) => Ok(format!(
                "## Settle XRP claim\n\n\
                You are requesting payout of XRP claim #{} to XRPL address **{}**. Verify this destination carefully.",
                claim_id, safe_consent_value(&destination)
            )),
            Err(_) => Ok(
                "## Settle XRP claim\n\n\
                This call sends an XRP claim to the XRPL destination supplied in the arguments. Verify the destination in your wallet before approving."
                    .to_string(),
            ),
        },

        "settle_xrp_claim_with_tag" => match Decode!(arg, u64, String, u32) {
            Ok((claim_id, destination, tag)) => Ok(format!(
                "## Settle XRP claim\n\n\
                You are requesting payout of XRP claim #{} to XRPL address **{}** with destination tag **{}**. Verify both values carefully.",
                claim_id, safe_consent_value(&destination), tag
            )),
            Err(_) => Ok(
                "## Settle XRP claim\n\n\
                This call sends an XRP claim to the XRPL destination and tag supplied in the arguments. Verify both in your wallet before approving."
                    .to_string(),
            ),
        },

        "open_xrp_vault" => Ok(
            "## Open XRP vault\n\n\
            This requests XRP vault setup and returns deposit information if the route is enabled. It does not transfer XRP or credit collateral; any deposit and confirmation are separate steps."
                .to_string(),
        ),

        "open_chain_vault_evm"
        | "borrow_chain_vault_evm"
        | "withdraw_chain_collateral_evm"
        | "close_chain_vault_evm" => {
            type Intent = crate::chains::evm::eip712::VaultIntent;
            match Decode!(arg, Intent, Vec<u8>) {
                Ok((intent, _signature)) => {
                    let action = match method {
                        "open_chain_vault_evm" => "open a vault",
                        "borrow_chain_vault_evm" => "borrow icUSD",
                        "withdraw_chain_collateral_evm" => "withdraw collateral",
                        _ => "close a vault",
                    };
                    Ok(format!(
                        "## EVM signed intent: {}\n\n\
                        This signed request will {} on chain {} for vault #{}. Collateral: {} raw units; debt: {} e8s. Recipient: {}. Verify the signed intent and recipient in your wallet.",
                        method,
                        action,
                        intent.chain_id,
                        intent.vault_id,
                        intent.collateral_wei,
                        intent.debt_e8s,
                        intent.recipient
                    ))
                }
                Err(_) => Ok(format!(
                    "## EVM signed intent: {}\n\n\
                    This call submits a signed EVM vault action. Verify its action, amounts, chain, vault, and recipient in your wallet before approving.",
                    method
                )),
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
                "## Rumi Protocol Action: {}\n\n\
                Review the method name and arguments in your wallet before approving. The effect could not be decoded here.",
                safe_method_display(method)
            ))
        }
    }
}

/// ICRC-21: Get consent message for a canister call
pub fn icrc21_canister_call_consent_message(
    request: ConsentMessageRequest,
) -> Icrc21ConsentMessageResult {
    // This endpoint accepts anonymous ingress. Keep caller-controlled method,
    // language, and argument data out of logs.
    ic_cdk::println!("[ICRC21] Consent message request received");

    let message = match generate_consent_message(&request.method, &request.arg) {
        Ok(msg) => msg,
        Err(description) => {
            ic_cdk::println!("[ICRC21] Consent message generation unavailable");
            return Err(Icrc21Error::ConsentMessageUnavailable(ErrorInfo {
                description,
            }));
        }
    };

    let consent_message = match &request.user_preferences.device_spec {
        Some(DeviceSpec::LineDisplay {
            characters_per_line,
            lines_per_page,
        }) => {
            line_display_message(&message, *characters_per_line, *lines_per_page)
                // ICRC-21 prefers a fallback message when requested formatting is
                // unsupported. This also bounds the number of generated pages.
                .unwrap_or_else(|| ConsentMessage::GenericDisplayMessage(message))
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

    #[test]
    fn consent_messages_describe_financial_effects_and_destinations() {
        crate::state::replace_state(crate::state::State::default());

        let liquidation =
            generate_consent_message("liquidate_vault", &Encode!(&7u64).unwrap()).unwrap();
        assert!(liquidation.contains("currently unavailable"));
        assert!(liquidation.contains("No icUSD will be pulled"));
        assert!(!liquidation.contains("stability pool"));

        let close = generate_consent_message("close_vault", &Encode!(&7u64).unwrap()).unwrap();
        assert!(close.contains("without transferring collateral"));

        let withdrawal =
            generate_consent_message("withdraw_collateral", &Encode!(&7u64).unwrap()).unwrap();
        assert!(withdrawal.contains("all"));
        assert!(withdrawal.contains("after repaying all debt"));

        let deposit =
            generate_consent_message("provide_liquidity", &Encode!(&125_000_000u64).unwrap())
                .unwrap();
        assert!(deposit.contains("disabled"));
        assert!(deposit.contains("will not pull tokens"));
        assert!(!deposit.contains("Earn rewards"));

        let withdrawal =
            generate_consent_message("withdraw_liquidity", &Encode!(&125_000_000u64).unwrap())
                .unwrap();
        assert!(withdrawal.contains("disabled"));
        assert!(withdrawal.contains("will not mint or transfer"));

        let destination = "rDestination123".to_string();
        let settlement =
            generate_consent_message("settle_xrp_claim", &Encode!(&9u64, &destination).unwrap())
                .unwrap();
        assert!(settlement.contains(&destination));

        let tagged_settlement = generate_consent_message(
            "settle_xrp_claim_with_tag",
            &Encode!(&9u64, &destination, &42u32).unwrap(),
        )
        .unwrap();
        assert!(tagged_settlement.contains(&destination));
        assert!(tagged_settlement.contains("42"));
    }

    #[test]
    fn generic_method_display_is_printable_and_bounded() {
        let method = format!("bad\n**{}💣", "x".repeat(100));
        let display = safe_method_display(&method);
        assert!(display.len() <= MAX_CONSENT_METHOD_DISPLAY_CHARS + 3);
        assert!(!display.contains('\n'));
        assert!(!display.contains('*'));
        assert!(display.ends_with("..."));

        let message = generate_consent_message(&method, &[]).unwrap();
        assert!(message.len() < 256);
        assert!(!message.contains("bad\n"));
        assert!(!message.contains("💣"));
    }

    #[test]
    fn anonymous_consent_with_zero_line_dimensions_does_not_trap() {
        let request = ConsentMessageRequest {
            method: "unrecognized_method".to_string(),
            arg: vec![],
            user_preferences: ConsentMessageSpec {
                metadata: ConsentMessageMetadata {
                    language: "en".to_string(),
                    utc_offset_minutes: None,
                },
                device_spec: Some(DeviceSpec::LineDisplay {
                    characters_per_line: 0,
                    lines_per_page: 0,
                }),
            },
        };

        let response = icrc21_canister_call_consent_message(request).unwrap();
        match response.consent_message {
            ConsentMessage::LineDisplayMessage { pages } => {
                assert!(!pages.is_empty());
                assert!(pages.len() <= MAX_LINE_DISPLAY_PAGES);
                assert!(pages.iter().all(|page| !page.lines.is_empty()));
            }
            ConsentMessage::GenericDisplayMessage(_) => {
                panic!("small zero-dimension request should normalize and render as line display")
            }
        }
    }

    #[test]
    fn line_display_bounds_extreme_dimensions_and_page_count() {
        let message = "word ".repeat(MAX_LINE_DISPLAY_PAGES + 1);
        assert!(line_display_message(&message, 0, 0).is_none());

        let message = "short text";
        let rendered =
            line_display_message(&message, u16::MAX, u16::MAX).expect("bounded display output");
        match rendered {
            ConsentMessage::LineDisplayMessage { pages } => {
                assert_eq!(pages.len(), 1);
                assert!(pages[0].lines.len() <= usize::from(MAX_LINE_DISPLAY_LINES_PER_PAGE));
                assert!(pages[0]
                    .lines
                    .iter()
                    .all(|line| line.len() <= usize::from(MAX_LINE_DISPLAY_CHARS_PER_LINE)));
            }
            ConsentMessage::GenericDisplayMessage(_) => panic!("expected bounded line display"),
        }
    }

    #[test]
    fn quoted_v2_consent_identifies_request_amount_asset_and_minimum() {
        crate::state::replace_state(crate::state::State::default());
        let request = crate::state::RedemptionV2Request {
            request_id: u128::MAX,
            amount_e8s: 100_000_001,
            expected_collateral_type: Principal::anonymous(),
            min_net_collateral_raw: 400_000,
        };
        let message = generate_consent_message(
            "redeem_quoted_v2",
            &Encode!(&request).expect("encode request"),
        )
        .expect("consent message");

        assert!(message.contains(&u128::MAX.to_string()));
        assert!(message.contains("1.00000001 icUSD"));
        assert!(message.contains("minimum payout"));
        assert!(message.contains("caller's account"));
    }

    #[test]
    fn consent_values_and_line_display_bound_hostile_long_content() {
        let hostile_destination = format!("r\n**{}\u{202e}💥", "x".repeat(10_000));
        let message = generate_consent_message(
            "settle_xrp_claim",
            &Encode!(&1u64, &hostile_destination).expect("encode claim"),
        )
        .expect("consent message");
        assert!(message.len() < 512);
        assert!(message.contains("..."));
        let escaped = safe_consent_value(&hostile_destination);
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains("**"));
        assert!(!escaped.contains('\u{202e}'));

        let long_word = "界".repeat(1_000);
        let rendered = line_display_message(&long_word, 7, 2).expect("bounded pages");
        match rendered {
            ConsentMessage::LineDisplayMessage { pages } => {
                assert!(pages.len() <= MAX_LINE_DISPLAY_PAGES);
                assert!(pages.iter().flat_map(|page| &page.lines).all(|line| {
                    line.chars().count() <= 7 && line.len() <= "界".len() * 7
                }));
            }
            ConsentMessage::GenericDisplayMessage(_) => panic!("expected line display"),
        }
    }

    #[test]
    fn exact_consent_amounts_preserve_smallest_units() {
        assert_eq!(
            format_icusd_amount_exact(100_000_001),
            "1.00000001 icUSD"
        );
        assert_eq!(
            format_token_amount_exact(1, 18, "ckETH"),
            "0.000000000000000001 ckETH"
        );
        assert_eq!(safe_token_symbol("ckETH\u{202e}**"), "ckETH___");
        assert_eq!(
            format_token_amount_exact(u64::MAX, 255, "token"),
            "18446744073709551615 raw units (255 decimals) token"
        );
    }
}
