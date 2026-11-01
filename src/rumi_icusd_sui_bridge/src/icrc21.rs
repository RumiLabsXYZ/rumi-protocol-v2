//! ICRC-21 consent messages (required for Oisy) and ICRC-10/28 metadata, with Sui wording. Ported
//! from the Solana instance's `icrc21.rs` (4394b472), itself the Base instance's; the wire types and
//! pagination are unchanged.
//!
//! Messages state exactly what moves and what does not. Amounts are rendered from integer e8s without
//! floating point. The deposit and the return flows are covered:
//!
//! * `bridge_to_sui` (deposit): the icUSD moves into escrow and a mint follows;
//! * `request_deposit_refund`: an escrowed deposit goes back when the mint is proven absent;
//! * `notify_sui_return` (return): a hint only. The return itself is a Sui transaction the holder
//!   signs in a Sui wallet (`bridge_out`, which burns the coin); this call moves no funds.

use crate::state::{read_core, read_runtime};
use crate::sui::parse_digest_base58;
use crate::types::BridgeToSuiArgs;
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

fn network_name() -> &'static str {
    match read_core(|c| c.network) {
        crate::ids::SuiNetwork::Mainnet => "Sui",
        crate::ids::SuiNetwork::Testnet => "Sui (testnet)",
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
        "bridge_to_sui" => {
            let a = Decode!(arg, BridgeToSuiArgs).map_err(bad_arg)?;
            let recipient = crate::flows::deposit::parse_recipient(&a.sui_recipient).map_err(|e| bad_arg(format!("{e:?}")))?;
            let fee = read_runtime(|c| c.config.fees.deposit_fee_e8s);
            let name = network_name();
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
                "## Bridge icUSD to {name}\n\n\
                 * Moves **{}** from {source} into the Rumi bridge escrow. The icUSD ledger fee is charged to you separately.\n\
                 * Mints **{}** icUSD on {name} to the address `{}`.\n\
                 * Bridge fee: {}.\n\
                 * Delivery is expected after the transaction is finalized on {name}. Your escrowed icUSD stays locked while it circulates on {name}.\n\
                 * This does not borrow or create debt.",
                format_icusd(a.amount_e8s),
                format_icusd(receive).trim_end_matches(" icUSD"),
                crate::sui::format_address(&recipient),
                format_icusd(fee),
            ))
        }
        "request_deposit_refund" => {
            let id = Decode!(arg, u64).map_err(bad_arg)?;
            let name = network_name();
            Ok(format!(
                "## Refund bridge deposit #{id}\n\n\
                 * Returns the escrowed icUSD of deposit #{id} to the account it came from, minus the icUSD ledger fee.\n\
                 * Only possible when the {name} mint is proven not to have happened."
            ))
        }
        "notify_sui_return" => {
            let digest = Decode!(arg, String).map_err(bad_arg)?;
            parse_digest_base58(digest.trim()).map_err(bad_arg)?;
            let name = network_name();
            Ok(format!(
                "## Report a {name} return transaction\n\n\
                 * Tells the bridge to check {name} transaction `{}` sooner.\n\
                 * This moves no funds. A return is credited only after the bridge verifies the finalized transaction and the gate's own record of it, and icUSD is released on ICP only to the account that record names.",
                digest.trim()
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
    use crate::sui::{format_address, format_digest_base58};
    use candid::Encode;

    const RECIPIENT: [u8; 32] = [0x77; 32];

    fn deposit(from_subaccount: Option<serde_bytes::ByteBuf>, fee: u64) -> Vec<u8> {
        Encode!(&BridgeToSuiArgs {
            amount_e8s: 250_000_000,
            sui_recipient: format_address(&RECIPIENT),
            from_subaccount,
            client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
            expected_deposit_fee_e8s: fee,
        })
        .unwrap()
    }

    fn on_network(network: crate::ids::SuiNetwork) {
        crate::state::reset_for_tests();
        crate::state::mutate_core(|c| c.network = network);
    }

    #[test]
    fn exact_amount_formatting() {
        assert_eq!(format_icusd(0), "0 icUSD");
        assert_eq!(format_icusd(1), "0.00000001 icUSD");
        assert_eq!(format_icusd(100_000_000), "1 icUSD");
        assert_eq!(format_icusd(12_345_000_000), "123.45 icUSD");
        assert_eq!(format_icusd(u64::MAX), "184467440737.09551615 icUSD");
    }

    #[test]
    fn deposit_message_names_amounts_recipient_and_the_network() {
        on_network(crate::ids::SuiNetwork::Testnet);
        let text = consent_text("bridge_to_sui", &deposit(None, 0)).unwrap();
        assert!(text.starts_with("## Bridge icUSD to Sui (testnet)\n"), "{text}");
        assert!(text.contains("**2.5 icUSD**"));
        assert!(text.contains("Mints **2.5** icUSD"));
        assert!(text.contains(&format!("`{}`", format_address(&RECIPIENT))));
        assert!(text.contains("does not borrow"));
        assert!(text.contains("finalized on Sui"));
        assert!(!text.contains("Base") && !text.contains("Solana"), "{text}");
    }

    #[test]
    fn mainnet_reads_as_plain_sui() {
        on_network(crate::ids::SuiNetwork::Mainnet);
        let text = consent_text("bridge_to_sui", &deposit(None, 0)).unwrap();
        assert!(text.starts_with("## Bridge icUSD to Sui\n"), "{text}");
    }

    #[test]
    fn stale_fee_is_refused_and_a_fee_is_deducted_from_the_mint() {
        on_network(crate::ids::SuiNetwork::Testnet);
        assert!(consent_text("bridge_to_sui", &deposit(None, 5)).is_err());
        crate::state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 10_000_000);
        let text = consent_text("bridge_to_sui", &deposit(None, 10_000_000)).unwrap();
        assert!(text.contains("Mints **2.4** icUSD"), "{text}");
        assert!(text.contains("Bridge fee: 0.1 icUSD."), "{text}");
        // An amount below the fee cannot be shown.
        crate::state::mutate_runtime(|c| c.config.fees.deposit_fee_e8s = 300_000_000);
        assert!(consent_text("bridge_to_sui", &deposit(None, 300_000_000)).is_err());
    }

    #[test]
    fn deposit_names_the_subaccount_and_refuses_bad_recipients() {
        on_network(crate::ids::SuiNetwork::Testnet);
        let text = consent_text("bridge_to_sui", &deposit(Some(serde_bytes::ByteBuf::from(vec![7u8; 32])), 0)).unwrap();
        assert!(text.contains(&hex::encode([7u8; 32])));
        assert!(consent_text("bridge_to_sui", &deposit(Some(serde_bytes::ByteBuf::from(vec![7u8; 31])), 0)).is_err());
        let with_recipient = |r: &str| {
            Encode!(&BridgeToSuiArgs {
                amount_e8s: 1,
                sui_recipient: r.into(),
                from_subaccount: None,
                client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
                expected_deposit_fee_e8s: 0,
            })
            .unwrap()
        };
        assert!(consent_text("bridge_to_sui", &with_recipient("not an address")).is_err());
        assert!(consent_text("bridge_to_sui", &with_recipient("0x2")).is_err(), "short form");
        assert!(consent_text("bridge_to_sui", &with_recipient(&format_address(&[0; 32]))).is_err(), "the zero address");
        assert!(consent_text("bridge_to_sui", &with_recipient(&format_address(&{ let mut a = [0u8; 32]; a[31] = 2; a }))).is_err(), "the system range");
        assert!(consent_text("bridge_to_sui", &with_recipient(&format_address(&RECIPIENT).to_uppercase())).is_err(), "uppercase is not the canonical form");
    }

    #[test]
    fn the_return_consent_moves_no_funds_and_names_the_gate_record() {
        on_network(crate::ids::SuiNetwork::Testnet);
        assert!(consent_text("notify_sui_return", &Encode!(&"not a digest".to_string()).unwrap()).is_err());
        assert!(consent_text("notify_sui_return", &Encode!(&bs58::encode([1u8; 31]).into_string()).unwrap()).is_err(), "a 31-byte value is not a digest");
        let digest = format_digest_base58(&[9u8; 32]);
        let text = consent_text("notify_sui_return", &Encode!(&digest).unwrap()).unwrap();
        assert!(text.contains(&format!("`{digest}`")));
        assert!(text.contains("moves no funds"));
        assert!(text.contains("gate's own record"));
        let refund = consent_text("request_deposit_refund", &Encode!(&7u64).unwrap()).unwrap();
        assert!(refund.contains("deposit #7") && refund.contains("Sui (testnet) mint is proven not to have happened"), "{refund}");
    }

    #[test]
    fn unknown_methods_and_bad_args_are_refused() {
        assert!(matches!(consent_text("admin_set_pauses", &[]), Err(Icrc21Error::UnsupportedCanisterCall(_))));
        assert!(matches!(consent_text("bridge_to_solana", &[]), Err(Icrc21Error::UnsupportedCanisterCall(_))), "other chains' methods do not exist here");
        assert!(matches!(consent_text("bridge_to_sui", &[1, 2, 3]), Err(Icrc21Error::ConsentMessageUnavailable(_))));
    }

    #[test]
    fn generic_and_line_display_requests_work() {
        on_network(crate::ids::SuiNetwork::Testnet);
        let req = |device_spec| ConsentMessageRequest {
            method: "request_deposit_refund".into(),
            arg: Encode!(&3u64).unwrap(),
            user_preferences: ConsentMessageSpec { metadata: ConsentMessageMetadata { language: "en".into(), utc_offset_minutes: Some(60) }, device_spec },
        };
        let generic = consent_message(req(None)).unwrap();
        assert_eq!(generic.metadata.utc_offset_minutes, Some(60));
        assert!(matches!(generic.consent_message, ConsentMessage::GenericDisplayMessage(_)));
        let lines = consent_message(req(Some(DeviceSpec::LineDisplay { characters_per_line: 16, lines_per_page: 3 }))).unwrap();
        assert!(matches!(lines.consent_message, ConsentMessage::LineDisplayMessage { .. }));
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

    #[test]
    fn standards_metadata() {
        assert_eq!(supported_standards().len(), 2);
        assert!(trusted_origins().trusted_origins.iter().all(|o| o.starts_with("https://")));
    }
}
