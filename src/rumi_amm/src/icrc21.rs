// ICRC-21 Consent Message Support for Oisy Wallet Integration
// Implements human-readable consent messages for the Rumi AMM canister

use candid::{CandidType, Deserialize};

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
    GenericError { error_code: u64, description: String },
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
        "swap" => {
            "## Swap Tokens\n\n\
            You are swapping tokens in the Rumi AMM.\n\n\
            This will:\n\
            - Transfer your input token to the pool\n\
            - Receive the output token in return\n\n\
            *A swap fee applies (set by the pool).*".to_string()
        }

        "add_liquidity" => {
            "## Add Liquidity\n\n\
            You are adding liquidity to a Rumi AMM pool.\n\n\
            This will:\n\
            - Transfer both tokens to the pool\n\
            - Mint LP shares representing your position\n\n\
            *You can withdraw your liquidity at any time.*".to_string()
        }

        "remove_liquidity" => {
            "## Remove Liquidity\n\n\
            You are withdrawing liquidity from a Rumi AMM pool.\n\n\
            This will:\n\
            - Burn your LP shares\n\
            - Return a proportional share of both tokens\n\n\
            *No fee for withdrawal.*".to_string()
        }

        "claim_pending" => {
            "## Claim Pending Tokens\n\n\
            You are retrying a previously failed token transfer.\n\n\
            This will:\n\
            - Transfer tokens owed to you from a prior operation\n\n\
            *No additional fee applies.*".to_string()
        }

        // Query methods
        "health" | "get_pool" | "get_pools" | "get_quote" | "get_lp_balance" |
        "is_pool_creation_open" | "is_maintenance_mode" | "get_pending_claims" => {
            format!(
                "## Query: {}\n\n\
                This is a read-only query that does not modify any state.",
                method
            )
        }

        // Admin methods
        "create_pool" | "set_fee" | "set_protocol_fee" | "withdraw_protocol_fees" |
        "pause_pool" | "unpause_pool" | "set_pool_creation_open" |
        "set_maintenance_mode" | "resolve_pending_claim" => {
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

// ─── Public API ───

pub fn icrc21_canister_call_consent_message(
    request: ConsentMessageRequest,
) -> Icrc21ConsentMessageResult {
    let message = generate_consent_message(&request.method);

    let consent_message = match &request.user_preferences.device_spec {
        Some(DeviceSpec::LineDisplay { characters_per_line, lines_per_page }) => {
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
                .map(|chunk| LineDisplayPage { lines: chunk.to_vec() })
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
