//! BB-52 correction tests (F1, F2, F3), kept in their own file so they can be added to the frozen 34e68d9e
//! source before the fix (red) and stay unchanged after it (green).
//!
//! F1. `Query.chainIdentifier` is the genesis checkpoint digest, Base58-encoded (pinned
//! `mainnet-v1.82.3`, `crates/sui-indexer-alt-graphql`: `schema.graphql` documents the field as
//! "The network's genesis checkpoint digest (uniquely identifies the network), Base58-encoded.",
//! and `src/api/query.rs` returns `Base58::encode(chain_id.wait().await.as_bytes())`). It is NOT
//! the four-byte hex of JSON-RPC. A provider is checked per response, before the quorum.
//!
//! F2. `request_object_at_version` promises an exact version, so a response for another version
//! must never become a usable or agreeing fact.
//!
//! F3. `parse_envelope` rejects an `errors` member that is not an array, so the transform must not
//! erase one: a reply the parser rejects must stay rejected after `canonicalize_response`.

use crate::graphql::*;
use crate::ids::{MAINNET_CHAIN_IDENTIFIER, TESTNET_CHAIN_IDENTIFIER};
use crate::sui::{format_address, format_digest_base58, Bytes32};
use crate::vectors::*;
use serde_json::{json, Value};

fn b58(d: &Bytes32) -> String {
    format_digest_base58(d)
}

fn testnet() -> String {
    b58(&TESTNET_CHAIN_IDENTIFIER)
}

fn mainnet() -> String {
    b58(&MAINNET_CHAIN_IDENTIFIER)
}

const RULE: QuorumRule = QuorumRule { min_quorum: 2, min_independent: 2 };

fn data_of(v: &Value) -> Value {
    parse_envelope(&serde_json::to_vec(v).unwrap()).unwrap()
}

fn checkpoint_body(chain: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "data": {
        "chainIdentifier": chain,
        "checkpoint": { "sequenceNumber": 500, "digest": format_digest_base58(&[3; 32]), "epoch": { "epochId": 77 } }
    }}))
    .unwrap()
}

fn object_json(version: Value, chain: &str) -> Value {
    json!({ "data": {
        "chainIdentifier": chain,
        "object": {
            "address": format_address(&GATE_ID),
            "version": version,
            "digest": format_digest_base58(&[0x77; 32]),
            "owner": { "__typename": "Shared", "initialSharedVersion": 42 },
            "asMoveObject": { "contents": { "type": { "repr": configured().gate_type() }, "bcs": GATE_BCS_B64 } }
        }
    }})
}

// ---------------------------------------------------------------------------
// F1: the network identity is the full Base58 genesis digest
// ---------------------------------------------------------------------------

#[test]
fn the_chain_identifier_is_the_full_base58_genesis_digest_of_the_configured_network() {
    // The sourced constants, in the form a provider reports them.
    assert_eq!(mainnet(), "4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S");
    assert_eq!(testnet(), "69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD");
    for network in [mainnet(), testnet()] {
        assert_eq!(check_chain_identifier(&json!({ "chainIdentifier": network }), &network), Ok(()), "{network}");
    }
}

#[test]
fn the_other_networks_genesis_digest_is_a_wrong_network() {
    assert_eq!(
        check_chain_identifier(&json!({ "chainIdentifier": mainnet() }), &testnet()),
        Err(Exclusion::WrongNetwork(mainnet()))
    );
    assert_eq!(
        check_chain_identifier(&json!({ "chainIdentifier": testnet() }), &mainnet()),
        Err(Exclusion::WrongNetwork(testnet()))
    );
}

#[test]
fn short_hex_is_never_accepted_for_the_graphql_chain_identifier() {
    let short = "4c78adac";
    // Not even when the configured value was (wrongly) given in the same short form.
    assert!(check_chain_identifier(&json!({ "chainIdentifier": short }), short).is_err(), "short hex must not match short hex");
    // And never against the correct full configured value.
    for expected in [testnet(), mainnet()] {
        assert!(check_chain_identifier(&json!({ "chainIdentifier": short }), &expected).is_err());
        assert!(check_chain_identifier(&json!({ "chainIdentifier": "35834a8a" }), &expected).is_err());
    }
}

#[test]
fn malformed_base58_is_rejected_as_malformed() {
    // Characters outside the Base58 alphabet: 0, O, I and l.
    for bad in ['0', 'O', 'I', 'l'] {
        let reported = format!("{bad}{}", &testnet()[1..]);
        assert!(
            matches!(check_chain_identifier(&json!({ "chainIdentifier": reported }), &testnet()), Err(Exclusion::Malformed(_))),
            "{reported}"
        );
    }
    for value in [json!(""), json!(12345), json!(null), json!(["x"])] {
        assert!(matches!(check_chain_identifier(&json!({ "chainIdentifier": value }), &testnet()), Err(Exclusion::Malformed(_))), "{value}");
    }
    assert!(matches!(check_chain_identifier(&json!({}), &testnet()), Err(Exclusion::Malformed(_))));
}

#[test]
fn base58_of_the_wrong_length_is_rejected_as_malformed() {
    for len in [31usize, 33] {
        let reported = bs58::encode(vec![7u8; len]).into_string();
        assert!(
            matches!(check_chain_identifier(&json!({ "chainIdentifier": reported }), &testnet()), Err(Exclusion::Malformed(_))),
            "{len} bytes"
        );
    }
}

#[test]
fn a_failing_provider_is_excluded_before_the_quorum_counts_it() {
    let tn = testnet();
    let read = |body: &[u8]| outcome_from_body(body, |d| parse_checkpoint(d, 500, &tn));
    let good = checkpoint_body(&tn);
    let other_network = checkpoint_body(&mainnet());
    let short_hex = checkpoint_body("4c78adac");
    let readings = |bodies: [&Vec<u8>; 3]| -> Vec<(String, Outcome<CheckpointFact>)> {
        bodies.iter().enumerate().map(|(i, b)| (format!("p{i}"), read(b))).collect()
    };

    // Two good providers agree even though a third reports another network.
    let r = readings([&good, &good, &other_network]);
    assert!(matches!(&r[2].1, Outcome::Excluded(Exclusion::WrongNetwork(_))));
    assert_eq!(tally(&r, RULE), Verdict::Agreed(CheckpointFact { sequence: 500, digest: [3; 32], epoch: 77 }));

    // A wrong-network or short-hex provider cannot make up a quorum with one good provider.
    for bad in [&other_network, &short_hex] {
        let r = readings([&good, bad, bad]);
        assert!(matches!(tally(&r, RULE), Verdict::Unavailable(_)), "one good provider is below the independence floor");
    }
    // And a good provider cannot be outvoted into agreement by two excluded ones.
    assert!(matches!(tally(&readings([&good, &short_hex, &other_network]), RULE), Verdict::Unavailable(_)));
}

#[test]
fn object_and_table_reads_use_the_same_identity_check() {
    let tn = testnet();
    let d = data_of(&object_json(json!(105), &tn));
    assert!(parse_object(&d, &GATE_ID, &tn).unwrap().is_some());
    let wrong = data_of(&object_json(json!(105), &mainnet()));
    assert!(matches!(parse_object(&wrong, &GATE_ID, &tn), Err(Exclusion::WrongNetwork(_))));
    let short = data_of(&object_json(json!(105), "4c78adac"));
    assert!(parse_object(&short, &GATE_ID, &tn).is_err());

    let entry = |chain: &str| json!({ "chainIdentifier": chain, "address": { "dynamicField": null } });
    assert_eq!(parse_table_entry(&entry(&tn), &mint_id_bytes(), &tn), Ok(None));
    assert!(matches!(parse_table_entry(&entry(&mainnet()), &mint_id_bytes(), &tn), Err(Exclusion::WrongNetwork(_))));
    assert!(parse_table_entry(&entry("4c78adac"), &mint_id_bytes(), &tn).is_err());
}

// ---------------------------------------------------------------------------
// F2: an exact-version request binds the response version
// ---------------------------------------------------------------------------

#[test]
fn an_exact_version_request_accepts_only_that_version() {
    let tn = testnet();
    let request = request_object_at_version(&GATE_ID, 105);
    assert_eq!(request.variables["v"], json!(105));
    let v105 = data_of(&object_json(json!(105), &tn));
    let v104 = data_of(&object_json(json!(104), &tn));
    let v106 = data_of(&object_json(json!(106), &tn));
    assert!(matches!(parse_object_at_version(&v105, &GATE_ID, 105, &tn), Ok(Some(o)) if o.version == 105));
    assert!(matches!(parse_object_at_version(&v104, &GATE_ID, 105, &tn), Err(Exclusion::Identity(_))), "version 104 for a request for 105");
    assert!(matches!(parse_object_at_version(&v106, &GATE_ID, 105, &tn), Err(Exclusion::Identity(_))), "version 106 for a request for 105");
    // UInt53 as a string is the same number.
    let as_string = data_of(&object_json(json!("105"), &tn));
    assert!(matches!(parse_object_at_version(&as_string, &GATE_ID, 105, &tn), Ok(Some(_))));
    // The address and network checks still apply.
    assert!(matches!(parse_object_at_version(&v105, &[0x6b; 32], 105, &tn), Err(Exclusion::Identity(_))));
    let wrong_net = data_of(&object_json(json!(105), &mainnet()));
    assert!(matches!(parse_object_at_version(&wrong_net, &GATE_ID, 105, &tn), Err(Exclusion::WrongNetwork(_))));
    // A null object at an exact version is a missing reading, not an error and not a fact.
    let missing = json!({ "chainIdentifier": tn, "object": null });
    assert_eq!(parse_object_at_version(&missing, &GATE_ID, 105, &tn), Ok(None));
}

#[test]
fn a_wrong_version_never_counts_toward_the_tally() {
    let tn = testnet();
    let body = |version: u64| serde_json::to_vec(&object_json(json!(version), &tn)).unwrap();
    let read = |b: &[u8]| outcome_from_body(b, |d| parse_object_at_version(d, &GATE_ID, 105, &tn));
    let readings = |versions: [u64; 3]| -> Vec<(String, Outcome<ObjectFact>)> {
        versions.iter().enumerate().map(|(i, v)| (format!("p{i}"), read(&body(*v)))).collect()
    };

    // The repro: request 105, one provider answers 104. It is set aside, not counted.
    let r = readings([105, 105, 104]);
    assert!(matches!(&r[2].1, Outcome::Excluded(Exclusion::Identity(_))));
    assert!(matches!(tally(&r, RULE), Verdict::Agreed(f) if f.version == 105));

    // A wrong version cannot agree with a right one, and two wrong versions cannot agree with each other.
    assert!(matches!(tally(&readings([105, 104, 104]), RULE), Verdict::Unavailable(_)));
    assert!(matches!(tally(&readings([104, 104, 104]), RULE), Verdict::Unavailable(_)), "agreement on the wrong version is not a fact");
}

#[test]
fn the_generic_object_decoder_is_unchanged_for_checkpoint_reads() {
    let tn = testnet();
    // An at-checkpoint read has no requested version: any version is the observed fact.
    let v104 = data_of(&object_json(json!(104), &tn));
    assert_eq!(parse_object(&v104, &GATE_ID, &tn).unwrap().unwrap().version, 104);
}

// ---------------------------------------------------------------------------
// F3: the transform never turns a rejected error-bearing reply into a usable fact
// ---------------------------------------------------------------------------

#[test]
fn a_transformed_response_never_turns_a_rejected_error_reply_into_a_usable_fact() {
    let fact_outcome = |body: &[u8]| outcome_from_body(body, |d| Ok::<_, Exclusion>(Some(d["x"].clone())));
    for errors in [json!({ "message": "failed" }), json!("failed"), json!(null), json!(7), json!(true)] {
        let raw = serde_json::to_vec(&json!({ "data": { "x": 1 }, "errors": errors })).unwrap();
        // Rejected raw, by the envelope parser.
        assert!(parse_envelope(&raw).is_err(), "raw {errors} must be rejected");
        assert!(matches!(fact_outcome(&raw), Outcome::Excluded(_)), "raw {errors}");
        // Still rejected after the transform: the transform either refuses it or keeps a
        // representation the parser refuses. It must never yield a usable fact.
        if let Ok(transformed) = canonicalize_response(&raw, 4096) {
            assert!(
                parse_envelope(&transformed).is_err(),
                "transformed {errors} became {}",
                String::from_utf8_lossy(&transformed)
            );
            assert!(matches!(fact_outcome(&transformed), Outcome::Excluded(_)), "transformed {errors}");
        }
    }
}

#[test]
fn valid_errors_arrays_keep_their_existing_behavior_through_the_transform() {
    let fact_outcome = |body: &[u8]| outcome_from_body(body, |d| Ok::<_, Exclusion>(Some(d["x"].clone())));
    // An empty array is no error: usable raw and after the transform.
    let empty = br#"{"data":{"x":1},"errors":[]}"#;
    assert!(parse_envelope(empty).is_ok());
    let transformed = canonicalize_response(empty, 4096).unwrap();
    assert!(parse_envelope(&transformed).is_ok());
    assert!(matches!(fact_outcome(&transformed), Outcome::Fact(v) if v == json!(1)));
    // A non-empty array fails raw and after the transform; its messages stay bounded.
    let long = "m".repeat(500);
    let nonempty = serde_json::to_vec(&json!({ "data": { "x": 1 }, "errors": [{ "message": long, "locations": [1] }, { "message": "b" }] })).unwrap();
    assert!(parse_envelope(&nonempty).is_err());
    let transformed = canonicalize_response(&nonempty, 4096).unwrap();
    assert!(matches!(parse_envelope(&transformed), Err(Exclusion::Transport(m)) if m.len() == 200));
    assert!(matches!(fact_outcome(&transformed), Outcome::Excluded(Exclusion::Transport(_))));
}
