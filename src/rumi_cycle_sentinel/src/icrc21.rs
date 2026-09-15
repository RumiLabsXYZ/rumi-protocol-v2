//! ICRC-21 consent messages for Cycle Sentinel signer-wallet calls.
//!
//! This module is deliberately stateless.  A signer receives the message
//! before it submits the requested call, so the wording must remain accurate
//! even if governance state changes in the interval between the two calls.

use candid::{CandidType, Nat};
use serde::{Deserialize, Serialize};

const MAX_LANGUAGE_BYTES: usize = 64;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageRequest {
    pub method: String,
    pub arg: Vec<u8>,
    pub user_preferences: ConsentMessageSpec,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConsentMessageSpec {
    pub metadata: ConsentMessageMetadata,
    pub device_spec: Option<DeviceSpec>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum DeviceSpec {
    GenericDisplay,
    FieldsDisplay,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ConsentMessageMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ConsentMessageResult {
    #[serde(rename = "Ok")]
    Ok(ConsentInfo),
    #[serde(rename = "Err")]
    Err(Icrc21Error),
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ConsentInfo {
    pub consent_message: ConsentMessage,
    pub metadata: ConsentMessageMetadata,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ConsentMessage {
    GenericDisplayMessage(String),
    FieldsDisplayMessage {
        intent: String,
        fields: Vec<(String, Value)>,
    },
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TokenAmount {
    pub decimals: u8,
    pub amount: u64,
    pub symbol: String,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TimestampSeconds {
    pub amount: u64,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DurationSeconds {
    pub amount: u64,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TextValue {
    pub content: String,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum Value {
    TokenAmount(TokenAmount),
    TimestampSeconds(TimestampSeconds),
    DurationSeconds(DurationSeconds),
    Text(TextValue),
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum Icrc21Error {
    UnsupportedCanisterCall(ErrorInfo),
    ConsentMessageUnavailable(ErrorInfo),
    InsufficientPayment(ErrorInfo),
    GenericError {
        error_code: Nat,
        description: String,
    },
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ErrorInfo {
    pub description: String,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct StandardRecord {
    pub name: String,
    pub url: String,
}

fn consent_message(method: &str) -> Option<&'static str> {
    match method {
        // Read-only calls are included because some signer agents request a
        // consent message before forwarding every actor call, including a
        // query such as get_my_permissions.
        "cycles_status"
        | "get_my_permissions"
        | "get_public_overview"
        | "get_public_target"
        | "list_governance_proposals"
        | "list_public_alarms"
        | "list_public_samples"
        | "list_public_targets"
        | "list_public_topups"
        | "list_unresolved_funding_operations" => Some(
            "Read Cycle Sentinel telemetry. This call cannot change a target, governance proposal, signer set, or cycle balance.",
        ),
        "propose_register_target" | "propose_update_target" | "propose_remove_target" => Some(
            "Create a Cycle Sentinel target-registry proposal. This records a proposal only; it cannot change a target or spend cycles until the required signers approve it and the proposal is executed.",
        ),
        "propose_set_global_policy" | "propose_unpause_target" => Some(
            "Create a Cycle Sentinel policy proposal. This records a proposal only; it cannot change automatic maintenance or spend cycles until the required signers approve it and the proposal is executed.",
        ),
        "propose_add_signer" | "propose_remove_signer" | "propose_set_signer_threshold" => Some(
            "Create a Cycle Sentinel signer-governance proposal. This records a proposal only; it cannot change the signer set or approval threshold until the required signers approve it and the proposal is executed.",
        ),
        "approve_proposal" => Some(
            "Approve a Cycle Sentinel governance proposal. Your approval can help make the proposal executable under the configured signer threshold, but does not execute it or transfer cycles by itself.",
        ),
        "execute_proposal" => Some(
            "Execute an approved Cycle Sentinel governance proposal. The proposal may change registered targets, policy settings, or signer governance. This call does not itself initiate a cycle top-up.",
        ),
        "cancel_proposal" => Some(
            "Cancel a Cycle Sentinel governance proposal. This prevents that proposal from being executed and cannot transfer cycles.",
        ),
        "pause_target" => Some(
            "Pause Cycle Sentinel maintenance for the selected target immediately. This protective action cannot transfer cycles.",
        ),
        "acknowledge_alarm" => Some(
            "Acknowledge a Cycle Sentinel alarm. This changes the alarm record only and cannot transfer cycles.",
        ),
        "manual_top_up" => Some(
            "Request a manual Cycle Sentinel top-up for the selected target. If authorized by the stored policy and available funding rail, this can transfer cycles to that target.",
        ),
        "attach_block_proof" | "attach_refund_block_proof" | "resolve_unknown_as_spent" => Some(
            "Reconcile a retained Cycle Sentinel funding operation using the supplied operation evidence. This can settle recorded accounting state but cannot initiate a new cycle transfer.",
        ),
        _ => None,
    }
}

pub fn icrc21_canister_call_consent_message(
    request: ConsentMessageRequest,
) -> ConsentMessageResult {
    if request.user_preferences.metadata.language.len() > MAX_LANGUAGE_BYTES {
        return ConsentMessageResult::Err(Icrc21Error::ConsentMessageUnavailable(ErrorInfo {
            description: "Requested consent language is too long.".to_string(),
        }));
    }

    // `arg` is intentionally not decoded here: every message describes the
    // full, bounded class of effect for its known method and never makes a
    // state-dependent promise about a target, proposal, or funding result.
    let Some(message) = consent_message(&request.method) else {
        return ConsentMessageResult::Err(Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
            description:
                "The requested method is not supported by Cycle Sentinel consent messages."
                    .to_string(),
        }));
    };

    ConsentMessageResult::Ok(ConsentInfo {
        consent_message: ConsentMessage::GenericDisplayMessage(message.to_string()),
        metadata: request.user_preferences.metadata,
    })
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str) -> ConsentMessageRequest {
        ConsentMessageRequest {
            method: method.to_string(),
            arg: vec![],
            user_preferences: ConsentMessageSpec {
                metadata: ConsentMessageMetadata {
                    language: "en-US".to_string(),
                    utc_offset_minutes: Some(-420),
                },
                device_spec: Some(DeviceSpec::GenericDisplay),
            },
        }
    }

    #[test]
    fn supports_read_governance_and_funding_messages() {
        for method in ["get_my_permissions", "approve_proposal", "manual_top_up"] {
            let result = icrc21_canister_call_consent_message(request(method));
            let ConsentMessageResult::Ok(info) = result else {
                panic!("{method} should have a consent message");
            };
            assert_eq!(info.metadata.language, "en-US");
            assert!(matches!(
                info.consent_message,
                ConsentMessage::GenericDisplayMessage(_)
            ));
        }
    }

    #[test]
    fn accepts_the_standard_fields_display_preference() {
        let mut fields_display = request("get_my_permissions");
        fields_display.user_preferences.device_spec = Some(DeviceSpec::FieldsDisplay);
        assert!(matches!(
            icrc21_canister_call_consent_message(fields_display),
            ConsentMessageResult::Ok(ConsentInfo {
                consent_message: ConsentMessage::GenericDisplayMessage(_),
                ..
            })
        ));
    }

    #[test]
    fn refuses_unknown_methods_without_echoing_attacker_controlled_text() {
        let result = icrc21_canister_call_consent_message(request("not_a_sentinel_method"));
        assert_eq!(
            result,
            ConsentMessageResult::Err(Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
                description:
                    "The requested method is not supported by Cycle Sentinel consent messages."
                        .to_string(),
            }))
        );
    }

    #[test]
    fn bounds_language_echo() {
        let mut unsupported = request("get_my_permissions");
        unsupported.user_preferences.metadata.language = "x".repeat(MAX_LANGUAGE_BYTES + 1);
        assert!(matches!(
            icrc21_canister_call_consent_message(unsupported),
            ConsentMessageResult::Err(Icrc21Error::ConsentMessageUnavailable(_))
        ));
    }
}
