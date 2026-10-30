//! Focused tests: the nine embedded profiles pass, and each rule family has at
//! least one negative control built by mutating a freshly parsed profile.

use crate::*;

const TODAY: &str = "2026-10-30";

fn embedded() -> Vec<Profile> {
    load_embedded().expect("embedded profiles parse")
}

fn profile<'a>(ps: &'a mut [Profile], id: &str) -> &'a mut Profile {
    ps.iter_mut().find(|p| p.id == id).expect("profile present")
}

fn rules(ps: &[Profile], today: &str) -> Vec<&'static str> {
    match validate(ps, today) {
        Ok(()) => Vec::new(),
        Err(v) => v.into_iter().map(|x| x.rule).collect(),
    }
}

fn assert_rule(ps: &[Profile], rule: &str) {
    let got = rules(ps, TODAY);
    assert!(
        got.contains(&rule),
        "expected a {rule} violation, got {got:?}"
    );
}

/// Put `text` into the first prerequisite of the sui profile and return the profiles.
fn with_prereq_text(text: String) -> Vec<Profile> {
    let mut ps = embedded();
    profile(&mut ps, "sui").prerequisites[0].value.text = text;
    ps
}

#[test]
fn all_nine_embedded_profiles_pass() {
    let ps = embedded();
    assert_eq!(ps.len(), 9);
    assert_eq!(validate(&ps, TODAY), Ok(()));
    let ids: Vec<&str> = ps.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, REQUIRED_PROFILE_IDS);
}

#[test]
fn every_network_is_disabled_with_default_claims() {
    for p in embedded() {
        assert!(!p.networks.is_empty());
        for n in &p.networks {
            assert_eq!(n.status, Status::Disabled, "{}", n.network_id);
            assert_eq!(n.instance_ref, None, "{}", n.network_id);
            assert_eq!(n.claims.deployment, Deployment::None);
            assert_eq!(n.claims.mint_authority, MintAuthority::None);
            assert!(
                !n.claims.mint_enabled && !n.claims.finality_checked && !n.claims.settlement_proven
            );
        }
    }
}

#[test]
fn unknown_fields_are_rejected_at_parse() {
    let with_extra = PROFILE_FILES[1].1.replacen('{', "{\"surprise\": true,", 1);
    assert!(parse_profile(&with_extra).is_err());
    assert!(parse_profile(PROFILE_FILES[1].1).is_ok());
}

#[test]
fn v1_duplicate_identity_rejected() {
    let mut ps = embedded();
    let copy = profile(&mut ps, "base").networks[0].identity.clone();
    profile(&mut ps, "arbitrum").networks[0].identity = copy;
    assert_rule(&ps, "V1");
}

#[test]
fn v2_enabled_status_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "base").networks[0].status = Status::Enabled;
    assert_rule(&ps, "V2");
}

#[test]
fn v3_decimals_seven_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "ethereum").representation.decimals = 7;
    assert_rule(&ps, "V3");
}

#[test]
fn v3_xrpl_bound_above_fifteen_digits_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "xrpl")
        .representation
        .amount_bounds
        .value
        .max_e8s = 1_000_000_000_000_000;
    assert_rule(&ps, "V3");
}

#[test]
fn v4_fact_without_a_source_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "solana").prerequisites[0].source_url = String::new();
    assert_rule(&ps, "V4");
}

#[test]
fn v4_fact_with_a_future_date_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "solana").prerequisites[0].verified = "2026-11-15".to_string();
    assert_rule(&ps, "V4");
}

#[test]
fn v4_today_is_a_parameter_not_a_clock() {
    let ps = embedded();
    // Every fact was verified on 2026-10-30, so an earlier "today" fails and a later one passes.
    assert!(rules(&ps, "2026-10-29").contains(&"V4"));
    assert!(rules(&ps, "2027-01-01").is_empty());
    assert!(rules(&ps, "2026-13-40").contains(&"V4"));
}

#[test]
fn v4_secondary_identity_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "sui").networks[0].identity.source_kind = SourceKind::Secondary;
    assert_rule(&ps, "V4");
}

#[test]
fn v5_monad_without_monad_verified_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "monad").networks[0].finality.rule = FinalityRule::EvmTag;
    assert_rule(&ps, "V5");
}

#[test]
fn v5_family_scheme_mismatch_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "solana").signing.scheme = Scheme::EcdsaSecp256k1;
    assert_rule(&ps, "V5");
}

#[test]
fn v6_non_base_reusing_a_live_base_string_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "arbitrum").signing.derivation_domain[0] =
        "rumi-icusd-base-bridge".to_string();
    assert_rule(&ps, "V6");

    let mut ps = embedded();
    profile(&mut ps, "monad").signing.domains.mint_id = Some("rumi.icusd.base.mint.v1".to_string());
    assert_rule(&ps, "V6");
}

#[test]
fn v6_mainnet_key_must_be_key_1() {
    let mut ps = embedded();
    profile(&mut ps, "ethereum").signing.key_name.value.mainnet = "test_key_1".to_string();
    assert_rule(&ps, "V6");
}

#[test]
fn v7_hyperevm_missing_its_blocker_rejected() {
    let mut ps = embedded();
    for n in &mut profile(&mut ps, "hyperevm").networks {
        n.activation_blockers
            .retain(|b| b.id != "hyperevm-historical-state");
    }
    assert_rule(&ps, "V7");
}

#[test]
fn v8_second_xrpl_profile_rejected() {
    let mut ps = embedded();
    let mut second = profile(&mut ps, "xrpl").clone();
    second.id = "xrpl-mpt".to_string();
    second.representation.standard.value = Standard::XrplMpt;
    ps.push(second);
    assert_rule(&ps, "V8");
}

#[test]
fn v8_xrpl_must_be_the_pr586_backend_module() {
    let mut ps = embedded();
    profile(&mut ps, "xrpl").topology.kind.value = TopologyKind::BridgeInstance;
    assert_rule(&ps, "V8");
}

#[test]
fn v9_mainnet_quorum_one_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "ethereum").networks[0]
        .rpc
        .min_quorum
        .value = 1;
    assert_rule(&ps, "V9");
}

#[test]
fn v9_provider_shortage_needs_a_blocker() {
    let mut ps = embedded();
    for n in &mut profile(&mut ps, "robinhood").networks {
        n.activation_blockers
            .retain(|b| b.id != "single-keyless-endpoint");
    }
    assert_rule(&ps, "V9");
}

#[test]
fn v10_address_hash_and_path_hygiene() {
    let cases = [
        ("a 20-byte address", format!("see 0x{}", "ab".repeat(20))),
        ("a 64-hex hash", format!("0x{}", "cd".repeat(32))),
        ("a bare 64-hex hash", "ef".repeat(32)),
        (
            "an XRPL classic address",
            format!("account r{}", "a".repeat(26)),
        ),
        ("an absolute path", "see /Users/someone/project".to_string()),
        (
            "a URL key parameter",
            "https://example.com/rpc?apikey=abc".to_string(),
        ),
        (
            "URL credentials",
            "https://user:pass@example.com/".to_string(),
        ),
    ];
    for (name, text) in cases {
        let got = rules(&with_prereq_text(text), TODAY);
        assert!(got.contains(&"V10"), "{name} was not flagged: {got:?}");
    }
}

#[test]
fn v10_public_ids_and_plain_prose_are_allowed() {
    // Chain ids, genesis hashes, short hex constants and repo paths are not findings.
    let text =
        "chain 8453, prefix 0x53545800, see foundry/src/BridgedIcUSD.sol and https://docs.sui.io/x";
    assert!(!rules(&with_prereq_text(text.to_string()), TODAY).contains(&"V10"));
}

#[test]
fn v10_endpoints_must_be_plain_hosts() {
    let mut ps = embedded();
    profile(&mut ps, "ethereum").networks[0].rpc.endpoints.value =
        vec!["https://rpc.example.com/path".to_string()];
    assert_rule(&ps, "V10");
}

#[test]
fn v11_eight_profiles_instead_of_nine_rejected() {
    let mut ps = embedded();
    ps.pop();
    assert_rule(&ps, "V11");
}

#[test]
fn validate_returns_every_violation() {
    let mut ps = embedded();
    profile(&mut ps, "base").networks[0].status = Status::Enabled;
    profile(&mut ps, "ethereum").representation.decimals = 7;
    let got = rules(&ps, TODAY);
    assert!(got.contains(&"V2") && got.contains(&"V3"), "{got:?}");
}

#[test]
fn monad_execution_delay_is_sourced_and_verified_head_blocker_kept() {
    let ps = embedded();
    let monad = ps.iter().find(|p| p.id == "monad").unwrap();
    for n in &monad.networks {
        let f = &n.finality.parameters;
        assert_eq!(f.value.execution_delay_blocks, Some(3));
        assert_eq!(
            f.source_url,
            "https://docs.monad.xyz/developer-essentials/summary"
        );
        assert_eq!(f.source_kind, SourceKind::Primary);
        assert!(n
            .activation_blockers
            .iter()
            .any(|b| b.id == "monad-verified-head"));
    }
}

#[test]
fn solana_carries_the_return_custody_blocker() {
    let ps = embedded();
    let solana = ps.iter().find(|p| p.id == "solana").unwrap();
    for n in &solana.networks {
        let ids: Vec<&str> = n
            .activation_blockers
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert!(ids.contains(&"solana-return-custody"), "{ids:?}");
        assert!(ids.contains(&"solana-guard-missing"), "{ids:?}");
    }
}

#[test]
fn sources_contain_no_em_dashes() {
    let em = '\u{2014}';
    for (name, json) in PROFILE_FILES {
        assert!(!json.contains(em), "{name}.json");
    }
    for src in [
        include_str!("lib.rs"),
        include_str!("types.rs"),
        include_str!("validate.rs"),
    ] {
        assert!(!src.contains(em));
    }
}

#[test]
fn settled_decisions_use_planning_status_and_d6_stays_open() {
    let settled = ["D1", "D1a", "D1b", "D2", "D3", "D4", "D5", "D7", "D8", "D9"];
    let still_recommendations = ["D1c", "D5xD8"];
    let mut seen = std::collections::BTreeSet::new();
    for p in embedded() {
        for d in &p.open_decisions {
            seen.insert(d.id.clone());
            if settled.contains(&d.id.as_str()) {
                assert_eq!(
                    d.choice.status,
                    DecisionStatus::PlanningDecision,
                    "{} {}",
                    p.id,
                    d.id
                );
            } else if still_recommendations.contains(&d.id.as_str()) {
                assert_eq!(
                    d.choice.status,
                    DecisionStatus::Recommendation,
                    "{} {}",
                    p.id,
                    d.id
                );
            } else {
                panic!("{}: unexpected decision {} (D6 must stay open)", p.id, d.id);
            }
        }
    }
    for id in settled.iter().chain(&still_recommendations) {
        assert!(seen.contains(*id), "no profile records {id}");
    }
}

#[test]
fn v1_d6_cannot_be_a_planning_decision() {
    let mut ps = embedded();
    profile(&mut ps, "sui").open_decisions.push(OpenDecision {
        id: "D6".to_string(),
        topic: "controllers".to_string(),
        choice: Decision {
            value: "decided".to_string(),
            rationale: "test".to_string(),
            status: DecisionStatus::PlanningDecision,
        },
    });
    assert_rule(&ps, "V1");
}

#[test]
fn xrpl_caps_each_operation_and_total_outstanding_at_fifteen_digits() {
    const CAP: u64 = 999_999_999_999_999;
    for p in embedded() {
        let b = &p.representation.amount_bounds.value;
        if p.family == Family::Xrpl {
            assert_eq!(b.encoding, Encoding::XrplDecimal15);
            assert_eq!(b.max_e8s, CAP);
            assert_eq!(b.max_outstanding_e8s, Some(CAP));
            assert_eq!(p.representation.standard.value, Standard::XrplIou);
            assert_eq!(
                p.representation.standard.status,
                DecisionStatus::PlanningDecision
            );
        } else {
            assert_eq!(b.max_outstanding_e8s, None, "{}", p.id);
        }
    }
}

#[test]
fn v3_xrpl_total_outstanding_above_fifteen_digits_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "xrpl")
        .representation
        .amount_bounds
        .value
        .max_outstanding_e8s = Some(1_000_000_000_000_000);
    assert_rule(&ps, "V3");
}

#[test]
fn v3_xrpl_total_outstanding_missing_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "xrpl")
        .representation
        .amount_bounds
        .value
        .max_outstanding_e8s = None;
    assert_rule(&ps, "V3");
}

#[test]
fn testnet_and_devnet_records_note_the_synthetic_ledger() {
    for p in embedded() {
        for n in &p.networks {
            let noted = n
                .activation_blockers
                .iter()
                .any(|b| b.id == "testnet-synthetic-ledger-only");
            assert_eq!(noted, n.role != Role::Mainnet, "{}", n.network_id);
        }
    }
}

#[test]
fn monad_requires_explicit_labels_and_never_binds_the_debt_token() {
    let ps = embedded();
    let monad = ps.iter().find(|p| p.id == "monad").unwrap();
    let labels = monad
        .prerequisites
        .iter()
        .find(|f| f.value.id == "monad-explicit-labels")
        .expect("explicit labels prerequisite");
    assert!(labels.value.unavoidable);
    assert!(labels.value.text.contains("debt-backed"));
    let d8 = monad.open_decisions.iter().find(|d| d.id == "D8").unwrap();
    assert_eq!(d8.choice.status, DecisionStatus::PlanningDecision);
}

// ---------------------------------------------------------------------------
// Finality parameter shape (V5)
// ---------------------------------------------------------------------------

fn violations_for(ps: &[Profile], rule: &str, needle: &str) -> bool {
    match validate(ps, TODAY) {
        Ok(()) => false,
        Err(v) => v
            .iter()
            .any(|x| x.rule == rule && x.message.contains(needle)),
    }
}

#[test]
fn v5_evm_tag_latest_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "base").networks[0]
        .finality
        .parameters
        .value
        .tag = Some("latest".to_string());
    assert!(violations_for(&ps, "V5", "finality parameter tag"));
}

#[test]
fn v5_evm_tag_missing_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "robinhood").networks[0]
        .finality
        .parameters
        .value
        .tag = None;
    assert!(violations_for(&ps, "V5", "finality parameter tag"));
}

#[test]
fn v5_solana_commitment_processed_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "solana").networks[0]
        .finality
        .parameters
        .value
        .commitment = Some("processed".to_string());
    assert!(violations_for(&ps, "V5", "finality parameter commitment"));
}

#[test]
fn v5_sui_must_use_the_finalized_checkpoint_field() {
    let mut ps = embedded();
    profile(&mut ps, "sui").networks[0]
        .finality
        .parameters
        .value
        .checkpoint = None;
    assert!(violations_for(&ps, "V5", "finality parameter checkpoint"));
}

#[test]
fn v5_xrpl_must_use_the_validated_ledger_fields() {
    let mut ps = embedded();
    profile(&mut ps, "xrpl").networks[0]
        .finality
        .parameters
        .value
        .ledger_index = Some("current".to_string());
    assert!(violations_for(&ps, "V5", "finality parameter ledger_index"));

    let mut ps = embedded();
    profile(&mut ps, "xrpl").networks[0]
        .finality
        .parameters
        .value
        .required_result = None;
    assert!(violations_for(
        &ps,
        "V5",
        "finality parameter required_result"
    ));
}

#[test]
fn v5_another_familys_finality_field_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "solana").networks[0]
        .finality
        .parameters
        .value
        .tag = Some("finalized".to_string());
    assert!(violations_for(&ps, "V5", "finality parameter tag"));
}

#[test]
fn v5_monad_delay_zero_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "monad").networks[0]
        .finality
        .parameters
        .value
        .execution_delay_blocks = Some(0);
    assert!(violations_for(&ps, "V5", "must be positive"));
}

#[test]
fn v5_monad_delay_missing_without_blocker_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "monad").networks[0]
        .finality
        .parameters
        .value
        .execution_delay_blocks = None;
    assert!(violations_for(
        &ps,
        "V5",
        "execution_delay_blocks is missing"
    ));
}

#[test]
fn v5_monad_delay_missing_is_allowed_only_with_the_unresolved_blocker() {
    let mut ps = embedded();
    let net = &mut profile(&mut ps, "monad").networks[0];
    net.finality.parameters.value.execution_delay_blocks = None;
    net.activation_blockers.push(Blocker {
        id: "monad-execution-delay-unknown".to_string(),
        text: "The execution delay has no primary source.".to_string(),
        kind: BlockerKind::External,
        source: "docs/coordination/multichain/sources-evm.md".to_string(),
    });
    assert!(!rules(&ps, TODAY).contains(&"V5"));
}

#[test]
fn v5_monad_delay_needs_a_primary_source() {
    let mut ps = embedded();
    profile(&mut ps, "monad").networks[0]
        .finality
        .parameters
        .source_kind = SourceKind::Secondary;
    assert!(violations_for(&ps, "V5", "primary source"));
}

#[test]
fn v5_delay_on_a_non_monad_chain_rejected() {
    let mut ps = embedded();
    profile(&mut ps, "ethereum").networks[0]
        .finality
        .parameters
        .value
        .execution_delay_blocks = Some(3);
    assert!(violations_for(&ps, "V5", "only applies to monad_verified"));
}

// ---------------------------------------------------------------------------
// Required blockers (V7): Solana return custody
// ---------------------------------------------------------------------------

#[test]
fn v7_solana_missing_return_custody_rejected() {
    let mut ps = embedded();
    for n in &mut profile(&mut ps, "solana").networks {
        n.activation_blockers
            .retain(|b| b.id != "solana-return-custody");
    }
    assert!(violations_for(&ps, "V7", "solana-return-custody"));
}

// ---------------------------------------------------------------------------
// Lossless amounts: decimal strings, checked u64
// ---------------------------------------------------------------------------

#[test]
fn amount_u64_max_round_trips_exactly() {
    let json = r#"{"min_e8s":"1","max_e8s":"18446744073709551615","max_outstanding_e8s":null,"encoding":"u64"}"#;
    let bounds: AmountBounds = serde_json::from_str(json).expect("parses");
    assert_eq!(bounds.max_e8s, u64::MAX);
    let out = serde_json::to_string(&bounds).expect("serializes");
    assert!(out.contains(r#""max_e8s":"18446744073709551615""#), "{out}");
    let again: AmountBounds = serde_json::from_str(&out).expect("re-parses");
    assert_eq!(again, bounds);
}

#[test]
fn amount_strings_must_be_canonical_decimal() {
    assert_eq!(parse_u64_decimal("0"), Ok(0));
    assert_eq!(parse_u64_decimal("18446744073709551615"), Ok(u64::MAX));
    for bad in [
        "",
        "+1",
        "-1",
        "01",
        "00",
        " 1",
        "1 ",
        "1e3",
        "1.0",
        "0x10",
        "١٢",
        "18446744073709551616",
    ] {
        assert!(
            parse_u64_decimal(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn amount_bounds_reject_numbers_and_bad_strings_at_parse() {
    let bad_values = [
        r#"1"#,                      // a JSON number
        r#""+1""#,                   // sign
        r#""01""#,                   // leading zero
        r#""""#,                     // empty
        r#""12a""#,                  // non-digit
        r#""18446744073709551616""#, // above u64::MAX
    ];
    for v in bad_values {
        let json = format!(
            r#"{{"min_e8s":"1","max_e8s":{v},"max_outstanding_e8s":null,"encoding":"u64"}}"#
        );
        assert!(
            serde_json::from_str::<AmountBounds>(&json).is_err(),
            "max_e8s {v} should be rejected"
        );
    }
    let json = r#"{"min_e8s":"1","max_e8s":"5","max_outstanding_e8s":7,"encoding":"u64"}"#;
    assert!(
        serde_json::from_str::<AmountBounds>(json).is_err(),
        "outstanding as a number should be rejected"
    );
}

#[test]
fn embedded_profiles_carry_amounts_as_strings_and_no_lossy_numbers() {
    fn walk(v: &serde_json::Value, path: &str) {
        match v {
            serde_json::Value::Number(n) => {
                let ok = n
                    .as_u64()
                    .map(|x| x <= 9_007_199_254_740_991)
                    .unwrap_or(false);
                assert!(ok, "{path}: {n} is not exactly representable in JavaScript");
            }
            serde_json::Value::Array(a) => a
                .iter()
                .enumerate()
                .for_each(|(i, x)| walk(x, &format!("{path}[{i}]"))),
            serde_json::Value::Object(m) => {
                m.iter().for_each(|(k, x)| walk(x, &format!("{path}.{k}")))
            }
            _ => {}
        }
    }
    for (name, json) in PROFILE_FILES {
        let v: serde_json::Value = serde_json::from_str(json).expect("valid json");
        walk(&v, name);
        let bounds = &v["representation"]["amount_bounds"]["value"];
        assert!(bounds["min_e8s"].is_string(), "{name}");
        assert!(bounds["max_e8s"].is_string(), "{name}");
        assert!(
            bounds["max_outstanding_e8s"].is_string() || bounds["max_outstanding_e8s"].is_null(),
            "{name}"
        );
    }
}
