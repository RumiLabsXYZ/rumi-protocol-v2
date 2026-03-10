// ICRC-21 Consent Message Support for Oisy Wallet Integration
// Implements human-readable consent messages for the 3pool canister

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
            "## Swap Stablecoins\n\n\
            You are swapping stablecoins in the Rumi 3pool.\n\n\
            This will:\n\
            - Transfer your input token to the pool\n\
            - Receive the output token in return\n\n\
            *A small swap fee applies.*".to_string()
        }

        "add_liquidity" => {
            "## Add Liquidity\n\n\
            You are adding liquidity to the Rumi 3pool.\n\n\
            This will:\n\
            - Transfer your stablecoins to the pool\n\
            - Mint LP tokens representing your share\n\n\
            *You can withdraw your liquidity at any time.*".to_string()
        }

        "remove_liquidity" => {
            "## Remove Liquidity\n\n\
            You are withdrawing liquidity from the Rumi 3pool.\n\n\
            This will:\n\
            - Burn your LP tokens\n\
            - Return a proportional share of all pool tokens\n\n\
            *No fees for proportional withdrawal.*".to_string()
        }

        "remove_one_coin" => {
            "## Remove Liquidity (Single Token)\n\n\
            You are withdrawing liquidity from the Rumi 3pool as a single token.\n\n\
            This will:\n\
            - Burn your LP tokens\n\
            - Return a single stablecoin\n\n\
            *An imbalance fee may apply.*".to_string()
        }

        "donate" => {
            "## Donate to Pool\n\n\
            You are donating tokens to the Rumi 3pool.\n\n\
            This will:\n\
            - Transfer tokens from your wallet to the pool\n\
            - Increase the virtual price for all LP holders\n\n\
            *No LP tokens are minted — this is a pure yield contribution.*".to_string()
        }

        // Query methods
        "health" | "get_pool_status" | "get_lp_balance" | "calc_swap" |
        "calc_add_liquidity_query" | "calc_remove_liquidity_query" |
        "calc_remove_one_coin_query" | "get_admin_fees" => {
            format!(
                "## Query: {}\n\n\
                This is a read-only query that does not modify any state.",
                method
            )
        }

        // Admin methods
        "ramp_a" | "stop_ramp_a" | "withdraw_admin_fees" | "set_paused" => {
            format!(
                "## Admin: {}\n\n\
                You are calling an admin method on the Rumi 3pool.\n\n\
                *Only the pool admin can execute this.*",
                method
            )
        }

        _ => {
            format!(
                "## Rumi 3pool Action\n\n\
                You are calling the **{}** method on the Rumi 3pool.\n\n\
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
            name: "ICRC-21".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string(),
        },
        StandardRecord {
            name: "ICRC-28".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-28/ICRC-28.md".to_string(),
        },
    ]
}
