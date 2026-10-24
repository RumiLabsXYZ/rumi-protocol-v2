//! ICRC-21 consent messages (required for Oisy) and ICRC-10/28 metadata.
//!
//! Messages state exactly what moves and what does not. Amounts are rendered from
//! integer e8s without floating point.

use crate::ids;
use crate::state::read_core;
use crate::types::BridgeToBaseArgs;
use candid::{CandidType, Decode, Deserialize};

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum DeviceSpec {
    GenericDisplay,
    LineDisplay { characters_per_line: u16, lines_per_page: u16 },
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

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc28TrustedOriginsResponse {
    pub trusted_origins: Vec<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct StandardRecord {
    pub name: String,
    pub url: String,
}

/// Exact decimal rendering of an e8s amount, trailing zeros trimmed.
pub fn format_icusd(e8s: u64) -> String {
    let whole = e8s / 100_000_000;
    let frac = e8s % 100_000_000;
    if frac == 0 {
        format!("{whole} icUSD")
    } else {
        let f = format!("{frac:08}");
        format!("{whole}.{} icUSD", f.trim_end_matches('0'))
    }
}

fn unsupported(method: &str) -> Icrc21Error {
    Icrc21Error::UnsupportedCanisterCall(ErrorInfo { description: format!("no consent message for {method}") })
}

fn bad_arg(e: impl std::fmt::Display) -> Icrc21Error {
    Icrc21Error::ConsentMessageUnavailable(ErrorInfo { description: format!("cannot decode arguments: {e}") })
}

pub fn consent_text(method: &str, arg: &[u8]) -> Result<String, Icrc21Error> {
    match method {
        "bridge_to_base" => {
            let a = Decode!(arg, BridgeToBaseArgs).map_err(bad_arg)?;
            let recipient = ids::parse_address(&a.base_recipient).map_err(bad_arg)?;
            let (fee, chain) = read_core(|c| (c.config.fees.deposit_fee_e8s, c.config.chain_id));
            if a.expected_deposit_fee_e8s != fee {
                return Err(Icrc21Error::ConsentMessageUnavailable(ErrorInfo {
                    description: format!(
                        "deposit fee changed: you were shown {}, current fee is {}",
                        format_icusd(a.expected_deposit_fee_e8s),
                        format_icusd(fee)
                    ),
                }));
            }
            let receive = a.amount_e8s.checked_sub(fee).ok_or_else(|| bad_arg("amount below fee"))?;
            let source = match &a.from_subaccount {
                None => "your default icUSD account".to_string(),
                Some(sub) if sub.len() == 32 => format!("your icUSD subaccount `{}`", hex::encode(sub)),
                Some(_) => return Err(bad_arg("from_subaccount must be 32 bytes")),
            };
            Ok(format!(
                "## Bridge icUSD to Base\n\n\
                 * Moves **{}** from {source} into the Rumi bridge escrow. The icUSD ledger fee is charged to you separately.\n\
                 * Mints **{}** icUSD on Base (chain {chain}) to `{}`.\n\
                 * Bridge fee: {}.\n\
                 * Delivery is expected after Base L1 batch finality, about 20 minutes. Your escrowed icUSD stays locked while it circulates on Base.\n\
                 * This does not borrow or create debt.",
                format_icusd(a.amount_e8s),
                format_icusd(receive).trim_end_matches(" icUSD"),
                // EIP-55 form, as the bridge page and Base wallets show it (same bytes).
                ids::to_checksum(&recipient),
                format_icusd(fee),
            ))
        }
        "request_deposit_refund" => {
            let id = Decode!(arg, u64).map_err(bad_arg)?;
            Ok(format!(
                "## Refund bridge deposit #{id}\n\n\
                 * Returns the escrowed icUSD of deposit #{id} to the account it came from, minus the icUSD ledger fee.\n\
                 * Only possible when the Base mint is proven not to have happened."
            ))
        }
        "notify_base_bridge_out" => {
            let tx = Decode!(arg, String).map_err(bad_arg)?;
            let tx = ids::hex0x(&ids::parse_bytes32(&tx).map_err(bad_arg)?);
            Ok(format!(
                "## Report a Base bridge transaction\n\n\
                 * Tells the bridge to check Base transaction `{tx}` sooner.\n\
                 * This moves no funds. Settlement happens only after the bridge verifies the finalized Base event."
            ))
        }
        other => Err(unsupported(other)),
    }
}

pub fn consent_message(req: ConsentMessageRequest) -> Result<ConsentInfo, Icrc21Error> {
    let text = consent_text(&req.method, &req.arg)?;
    let message = match req.user_preferences.device_spec {
        Some(DeviceSpec::LineDisplay { characters_per_line, lines_per_page }) => {
            ConsentMessage::LineDisplayMessage { pages: paginate(&text, characters_per_line, lines_per_page) }
        }
        _ => ConsentMessage::GenericDisplayMessage(text),
    };
    Ok(ConsentInfo {
        metadata: ConsentMessageMetadata { language: "en".into(), utc_offset_minutes: req.user_preferences.metadata.utc_offset_minutes },
        consent_message: message,
    })
}

fn paginate(text: &str, cpl: u16, lpp: u16) -> Vec<LineDisplayPage> {
    let cpl = cpl.max(8) as usize;
    let lpp = lpp.max(1) as usize;
    let plain = text.replace(['#', '*', '`'], "");
    let mut lines = Vec::new();
    for raw in plain.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let mut cur = String::new();
        for word in raw.split_whitespace() {
            if !cur.is_empty() && cur.len() + 1 + word.len() > cpl {
                lines.push(std::mem::take(&mut cur));
            }
            if word.len() > cpl {
                for chunk in word.as_bytes().chunks(cpl) {
                    lines.push(String::from_utf8_lossy(chunk).into_owned());
                }
                continue;
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines.chunks(lpp).map(|c| LineDisplayPage { lines: c.to_vec() }).collect()
}

pub fn trusted_origins() -> Icrc28TrustedOriginsResponse {
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

pub fn supported_standards() -> Vec<StandardRecord> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Encode;

    #[test]
    fn exact_amount_formatting() {
        assert_eq!(format_icusd(0), "0 icUSD");
        assert_eq!(format_icusd(1), "0.00000001 icUSD");
        assert_eq!(format_icusd(100_000_000), "1 icUSD");
        assert_eq!(format_icusd(12_345_000_000), "123.45 icUSD");
        assert_eq!(format_icusd(u64::MAX), "184467440737.09551615 icUSD");
    }

    #[test]
    fn bridge_to_base_message_names_amounts_and_recipient() {
        crate::state::reset_for_tests();
        let args = BridgeToBaseArgs {
            amount_e8s: 100_000_000,
            base_recipient: "0x00000000000000000000000000000000000000aa".into(),
            from_subaccount: None,
            client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
            expected_deposit_fee_e8s: 0,
        };
        let text = consent_text("bridge_to_base", &Encode!(&args).unwrap()).unwrap();
        assert!(text.contains("**1 icUSD**"));
        assert!(text.contains(&ids::to_checksum(&ids::parse_address("0x00000000000000000000000000000000000000aa").unwrap())));
        assert!(text.contains("does not borrow"));
        // A checksummed recipient is shown exactly in its EIP-55 form.
        let vector = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
        let args = BridgeToBaseArgs { base_recipient: vector.to_ascii_lowercase(), ..args };
        let text = consent_text("bridge_to_base", &Encode!(&args).unwrap()).unwrap();
        assert!(text.contains(vector), "{text}");
    }

    #[test]
    fn stale_fee_is_refused() {
        crate::state::reset_for_tests();
        let args = BridgeToBaseArgs {
            amount_e8s: 100_000_000,
            base_recipient: "0x00000000000000000000000000000000000000aa".into(),
            from_subaccount: None,
            client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
            expected_deposit_fee_e8s: 5,
        };
        assert!(consent_text("bridge_to_base", &Encode!(&args).unwrap()).is_err());
    }

    #[test]
    fn notify_consent_requires_a_valid_hash_and_deposit_names_the_subaccount() {
        crate::state::reset_for_tests();
        assert!(consent_text("notify_base_bridge_out", &Encode!(&"not a hash".to_string()).unwrap()).is_err());
        assert!(consent_text("notify_base_bridge_out", &Encode!(&format!("0x{}", "ab".repeat(32))).unwrap()).is_ok());
        let args = BridgeToBaseArgs {
            amount_e8s: 100_000_000,
            base_recipient: "0x00000000000000000000000000000000000000aa".into(),
            from_subaccount: Some(serde_bytes::ByteBuf::from(vec![7u8; 32])),
            client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
            expected_deposit_fee_e8s: 0,
        };
        let text = consent_text("bridge_to_base", &Encode!(&args).unwrap()).unwrap();
        assert!(text.contains(&hex::encode([7u8; 32])));
    }

    #[test]
    fn unknown_methods_and_bad_args_are_refused() {
        assert!(matches!(consent_text("admin_set_pauses", &[]), Err(Icrc21Error::UnsupportedCanisterCall(_))));
        assert!(matches!(consent_text("bridge_to_base", &[1, 2, 3]), Err(Icrc21Error::ConsentMessageUnavailable(_))));
    }

    #[test]
    fn line_display_pagination_respects_width() {
        let pages = paginate("## Title\n\n* a fairly long line that must wrap across the display", 12, 2);
        for p in &pages {
            assert!(p.lines.len() <= 2);
            for l in &p.lines {
                assert!(l.len() <= 12, "{l}");
            }
        }
    }
}
