// ICRC-21 Consent Message Support for Oisy Wallet Integration
// Implements human-readable consent messages for the Rumi AMM canister

use candid::{CandidType, Decode, Deserialize, Principal};

// ─── Types ───

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum DeviceSpec {
    GenericDisplay,
    LineDisplay {
        characters_per_line: u16,
        lines_per_page: u16,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageSpec {
    pub metadata: ConsentMessageMetadata,
    pub device_spec: Option<DeviceSpec>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageRequest {
    pub method: String,
    pub arg: Vec<u8>,
    pub user_preferences: ConsentMessageSpec,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct LineDisplayPage {
    pub lines: Vec<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsentMessage {
    GenericDisplayMessage(String),
    LineDisplayMessage { pages: Vec<LineDisplayPage> },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentInfo {
    pub metadata: ConsentMessageMetadata,
    pub consent_message: ConsentMessage,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ErrorInfo {
    pub description: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum Icrc21Error {
    GenericError {
        error_code: u64,
        description: String,
    },
    UnsupportedCanisterCall(ErrorInfo),
    ConsentMessageUnavailable(ErrorInfo),
}

pub type Icrc21ConsentMessageResult = Result<ConsentInfo, Icrc21Error>;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc28TrustedOriginsResponse {
    pub trusted_origins: Vec<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StandardRecord {
    pub name: String,
    pub url: String,
}

// ─── Consent message generation ───

fn generate_consent_message(method: &str) -> String {
    match method {
        "swap" => "## Swap Tokens\n\n\
            You are swapping tokens in the Rumi AMM.\n\n\
            This will:\n\
            - Transfer your input token to the pool\n\
            - Receive the output token in return\n\n\
            *A swap fee applies (set by the pool).*"
            .to_string(),

        "add_liquidity" => "## Add Liquidity\n\n\
            You are adding liquidity to a Rumi AMM pool.\n\n\
            This will:\n\
            - Transfer both tokens to the pool\n\
            - Mint LP shares representing your position\n\n\
            *You can withdraw your liquidity at any time.*"
            .to_string(),

        "remove_liquidity" => "## Remove Liquidity\n\n\
            You are withdrawing liquidity from a Rumi AMM pool.\n\n\
            This will:\n\
            - Burn your LP shares\n\
            - Return a proportional share of both tokens\n\n\
            *No fee for withdrawal.*"
            .to_string(),

        "claim_pending" => "## Claim Pending Tokens\n\n\
            You are retrying a previously failed token transfer.\n\n\
            This will:\n\
            - Transfer tokens owed to you from a prior operation\n\n\
            *No additional fee applies.*"
            .to_string(),

        "claim_rewards" => "## Claim AMM Rewards\n\n\
            You are claiming accrued rewards from a Rumi AMM pool.\n\n\
            This will:\n\
            - Deduct the icUSD ledger fee from your settled pool rewards\n\
            - Transfer the net icUSD amount to your wallet\n\n\
            *The exact amount is determined when the pool settles your rewards.*"
            .to_string(),

        // Query methods
        "health"
        | "get_pool"
        | "get_pools"
        | "get_quote"
        | "get_lp_balance"
        | "is_pool_creation_open"
        | "is_maintenance_mode"
        | "get_pending_claims" => {
            format!(
                "## Query: {}\n\n\
                This is a read-only query that does not modify any state.",
                method
            )
        }

        // Admin methods
        "create_pool"
        | "set_fee"
        | "set_protocol_fee"
        | "withdraw_protocol_fees"
        | "pause_pool"
        | "unpause_pool"
        | "set_pool_creation_open"
        | "set_maintenance_mode"
        | "resolve_pending_claim" => {
            format!(
                "## Admin: {}\n\n\
                You are calling an admin method on the Rumi AMM.\n\n\
                *Only the AMM admin can execute this.*",
                method
            )
        }

        _ => {
            format!(
                "## Rumi AMM Action\n\n\
                You are calling the **{}** method on the Rumi AMM.\n\n\
                *Please verify this action before approving.*",
                method
            )
        }
    }
}

fn token_label(token: Principal) -> String {
    if Principal::from_text(crate::ICUSD_LEDGER).ok() == Some(token) {
        "icUSD".to_string()
    } else if Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").ok() == Some(token) {
        "ICP".to_string()
    } else {
        format!("token ledger {}", token)
    }
}

fn generate_remove_liquidity_consent(arg: &[u8]) -> Result<String, String> {
    let (pool_id, lp_shares, min_amount_a, min_amount_b) =
        Decode!(arg, String, u128, u128, u128)
            .map_err(|_| "Invalid remove_liquidity arguments; consent preview unavailable".to_string())?;
    let pool = crate::state::read_state(|state| {
        state.pools.get(&pool_id).map(|pool| (
            pool.token_a,
            pool.token_b,
            pool.reserve_a,
            pool.reserve_b,
            pool.total_lp_shares,
        ))
    }).ok_or_else(|| format!("Pool {} is unavailable; consent preview cannot identify its output assets", pool_id))?;
    let (token_a, token_b, reserve_a, reserve_b, total_shares) = pool;
    let (gross_a, gross_b) = crate::compute_remove_liquidity(
        lp_shares, reserve_a, reserve_b, total_shares,
    ).map_err(|_| "Current pool state cannot produce a safe removal preview".to_string())?;
    let asset_a = token_label(token_a);
    let asset_b = token_label(token_b);
    Ok(format!(
        "## Remove Liquidity from {}\n\nYou authorize burning **{} LP shares**. At the current pool state, the proportional gross outputs are **{} raw units of {}** and **{} raw units of {}**; these are a preview, not guaranteed final amounts. The output minimums are **{} raw units of {} net** and **{} raw units of {} net**. Each payout deducts that token ledger's current transfer fee, so received amounts are gross outputs less the respective ledger fees and the call fails if either net output is below its floor. Pool reserves and your LP position are committed before the two external payouts. A failed or ambiguous leg remains held under its original payout identity for recovery, so one leg can arrive before the other; it is never blindly resent.",
        pool_id, lp_shares, gross_a, asset_a, gross_b, asset_b,
        min_amount_a, asset_a, min_amount_b, asset_b,
    ))
}

// ─── Public API ───

pub fn icrc21_canister_call_consent_message(
    request: ConsentMessageRequest,
) -> Icrc21ConsentMessageResult {
    let message = if request.method == "remove_liquidity" {
        match generate_remove_liquidity_consent(&request.arg) {
            Ok(message) => message,
            Err(description) => return Err(Icrc21Error::ConsentMessageUnavailable(ErrorInfo { description })),
        }
    } else {
        generate_consent_message(&request.method)
    };

    let consent_message = match &request.user_preferences.device_spec {
        Some(DeviceSpec::LineDisplay {
            characters_per_line,
            lines_per_page,
        }) => {
            let chars = *characters_per_line as usize;
            let lines = *lines_per_page as usize;

            let all_lines: Vec<String> = message
                .lines()
                .flat_map(|line| {
                    let clean = line
                        .replace("##", "")
                        .replace("**", "")
                        .replace("*", "")
                        .trim()
                        .to_string();

                    if clean.is_empty() {
                        vec![]
                    } else if clean.len() <= chars {
                        vec![clean]
                    } else {
                        let mut wrapped = Vec::new();
                        let mut current = String::new();
                        for word in clean.split_whitespace() {
                            if current.is_empty() {
                                current = word.to_string();
                            } else if current.len() + 1 + word.len() <= chars {
                                current.push(' ');
                                current.push_str(word);
                            } else {
                                wrapped.push(current);
                                current = word.to_string();
                            }
                        }
                        if !current.is_empty() {
                            wrapped.push(current);
                        }
                        wrapped
                    }
                })
                .collect();

            let pages: Vec<LineDisplayPage> = all_lines
                .chunks(lines)
                .map(|chunk| LineDisplayPage {
                    lines: chunk.to_vec(),
                })
                .collect();

            ConsentMessage::LineDisplayMessage { pages }
        }
        _ => ConsentMessage::GenericDisplayMessage(message),
    };

    Ok(ConsentInfo {
        metadata: ConsentMessageMetadata {
            language: request.user_preferences.metadata.language,
            utc_offset_minutes: request.user_preferences.metadata.utc_offset_minutes,
        },
        consent_message,
    })
}

pub fn icrc28_trusted_origins() -> Icrc28TrustedOriginsResponse {
    Icrc28TrustedOriginsResponse {
        trusted_origins: vec![
            "https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io".to_string(),
            "https://tcfua-yaaaa-aaaap-qrd7q-cai.raw.icp0.io".to_string(),
            "https://app.rumiprotocol.com".to_string(),
            "https://app.rumiprotocol.xyz".to_string(),
            "https://rumiprotocol.io".to_string(),
        ],
    }
}

pub fn icrc10_supported_standards() -> Vec<StandardRecord> {
    vec![
        StandardRecord {
            name: "ICRC-10".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md".to_string(),
        },
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

#[cfg(test)]
mod tests {
    use super::{generate_consent_message, generate_remove_liquidity_consent};
    use candid::{Encode, Principal};
    use std::collections::BTreeMap;

    fn install_test_pool() {
        let token_a = Principal::from_text(crate::ICUSD_LEDGER).unwrap();
        let token_b = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        let mut state = crate::state::AmmState::default();
        state.pools.insert("icUSD_ICP".to_string(), crate::types::Pool {
            token_a,
            token_b,
            reserve_a: 1_000_000,
            reserve_b: 3_000_000,
            fee_bps: 30,
            protocol_fee_bps: 5,
            curve: crate::types::CurveType::ConstantProduct,
            lp_shares: BTreeMap::new(),
            total_lp_shares: 10_000,
            protocol_fees_a: 0,
            protocol_fees_b: 0,
            paused: false,
            subaccount_a: [0; 32],
            subaccount_b: [0; 32],
            lp_rewards: BTreeMap::new(),
            acc_reward_per_share: 0,
            pending_no_lp: 0,
            total_rewards_distributed: 0,
            processed_donation_nonces: Default::default(),
            reward_balance_snapshot: 0,
        });
        crate::state::replace_state(state);
    }

    #[test]
    fn claim_rewards_has_method_specific_money_movement_consent() {
        let message = generate_consent_message("claim_rewards");
        assert!(message.contains("Claim AMM Rewards"));
        assert!(message.contains("Deduct the icUSD ledger fee"));
        assert!(message.contains("Transfer the net icUSD amount"));
        assert!(!message.contains("Rumi AMM Action"));
    }

    #[test]
    fn unknown_method_keeps_generic_fallback() {
        let message = generate_consent_message("future_method");
        assert!(message.contains("Rumi AMM Action"));
        assert!(message.contains("future_method"));
    }

    #[test]
    fn remove_liquidity_discloses_burn_assets_gross_preview_and_net_floors() {
        install_test_pool();
        let arg = Encode!(
            &"icUSD_ICP".to_string(),
            &5_000u128,
            &490_000u128,
            &1_490_000u128
        ).unwrap();
        let message = generate_remove_liquidity_consent(&arg).unwrap();
        assert!(message.contains("5000 LP shares"));
        assert!(message.contains("500000 raw units of icUSD"));
        assert!(message.contains("1500000 raw units of ICP"));
        assert!(message.contains("490000 raw units of icUSD net"));
        assert!(message.contains("1490000 raw units of ICP net"));
        assert!(message.contains("current transfer fee"));
        assert!(message.contains("preview, not guaranteed final amounts"));
        assert!(message.contains("committed before the two external payouts"));
        assert!(message.contains("one leg can arrive before the other"));
        assert!(message.contains("never blindly resent"));
    }

    #[test]
    fn remove_liquidity_consent_fails_closed_for_bad_or_unresolvable_args() {
        assert!(generate_remove_liquidity_consent(&[]).is_err());
        let arg = Encode!(&"missing".to_string(), &1u128, &0u128, &0u128).unwrap();
        assert!(generate_remove_liquidity_consent(&arg).is_err());
    }
}
