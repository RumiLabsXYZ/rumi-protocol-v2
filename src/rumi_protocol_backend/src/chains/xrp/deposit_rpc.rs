//! Exact native inbound receipt proof. Kept separate from outgoing settlement.
use super::xrp_rpc;
use ic_cdk::api::management_canister::http_request::{HttpResponse, TransformArgs};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePaymentReceipt {
    pub tx_hash: String,
    pub source: String,
    pub destination: String,
    pub delivered_drops: u64,
    pub ledger_index: u32,
    pub ledger_hash: String,
    pub flags: u32,
}

pub fn canonical_hash(hash: &str) -> Result<String, String> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected a 64-character hexadecimal transaction/ledger hash".into());
    }
    Ok(hash.to_ascii_uppercase())
}

/// Normalize rippled API v1 and v2 without falling back to nominal Amount for
/// credit. DeliverMax/Amount is checked only to constrain this rail to ordinary
/// direct native payments; delivered_amount remains the credited quantity.
pub fn transform(args: TransformArgs) -> HttpResponse {
    let body = match serde_json::from_slice::<Value>(&args.response.body) {
        Ok(v) => {
            let r = &v["result"];
            let tx = r.get("tx_json").unwrap_or(r);
            json!({
                "validated": r.get("validated"),
                "tx_hash": r.get("hash").or_else(|| tx.get("hash")),
                "ledger_index": r.get("ledger_index").or_else(|| tx.get("ledger_index")),
                "ledger_hash": r.get("ledger_hash"),
                "result": r.pointer("/meta/TransactionResult"),
                "delivered": r.pointer("/meta/delivered_amount"),
                "kind": tx.get("TransactionType"),
                "source": tx.get("Account"),
                "destination": tx.get("Destination"),
                "flags": tx.get("Flags").cloned().unwrap_or(json!(0)),
                "amount": tx.get("DeliverMax").or_else(|| tx.get("Amount")),
                "send_max": tx.get("SendMax"),
                "paths": tx.get("Paths"),
            })
        }
        Err(_) => json!({"parse_error":true}),
    };
    xrp_rpc::reduced_response(body.to_string().into_bytes())
}

fn native_drops(v: &Value) -> Result<u64, String> {
    // rippled encodes native currency as a decimal integer string. Do not
    // accept issued-currency objects, floats, unavailable, signs, or overflow.
    let s = v.as_str().ok_or("native drops must be an integer string")?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid native drops".into());
    }
    s.parse::<u64>()
        .map_err(|_| "native drops exceed u64".into())
}

pub fn parse(body: &[u8], expected_hash: &str) -> Result<NativePaymentReceipt, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    if v["validated"].as_bool() != Some(true)
        || v["result"].as_str() != Some("tesSUCCESS")
        || v["kind"].as_str() != Some("Payment")
    {
        return Err("receipt must be a validated successful Payment".into());
    }
    let tx_hash = canonical_hash(v["tx_hash"].as_str().ok_or("missing transaction hash")?)?;
    if tx_hash != canonical_hash(expected_hash)? {
        return Err("receipt transaction hash mismatch".into());
    }
    let ledger_index = u32::try_from(v["ledger_index"].as_u64().ok_or("missing ledger index")?)
        .map_err(|_| "ledger index exceeds u32")?;
    if ledger_index == 0 {
        return Err("zero ledger index".into());
    }
    let ledger_hash = canonical_hash(v["ledger_hash"].as_str().ok_or("missing ledger hash")?)?;
    let flags = u32::try_from(v["flags"].as_u64().ok_or("invalid flags")?)
        .map_err(|_| "flags exceed u32")?;
    if flags & 0x0002_0000 != 0 || !v["send_max"].is_null() || !v["paths"].is_null() {
        return Err(
            "only direct native payments without partial-payment flags are supported".into(),
        );
    }
    let delivered_drops = native_drops(&v["delivered"])?;
    if delivered_drops == 0 || native_drops(&v["amount"])? != delivered_drops {
        return Err("native delivered amount must be positive and match direct Payment".into());
    }
    let source = v["source"].as_str().ok_or("missing source")?.to_string();
    let destination = v["destination"]
        .as_str()
        .ok_or("missing destination")?
        .to_string();
    if source == destination
        || !super::address::is_valid_classic_address(&source)
        || !super::address::is_valid_classic_address(&destination)
    {
        return Err("invalid or self-payment custody receipt".into());
    }
    Ok(NativePaymentReceipt {
        tx_hash,
        source,
        destination,
        delivered_drops,
        ledger_index,
        ledger_hash,
        flags,
    })
}

fn quorum(bodies: &[Vec<u8>], expected_hash: &str) -> Result<NativePaymentReceipt, String> {
    let parsed: Vec<_> = bodies
        .iter()
        .filter_map(|b| parse(b, expected_hash).ok())
        .collect();
    for proof in &parsed {
        if parsed.iter().filter(|other| *other == proof).count() >= xrp_rpc::READ_QUORUM {
            return Ok(proof.clone());
        }
    }
    Err("native Payment receipt provider quorum failed".into())
}

pub async fn fetch(key_name: &str, hash: &str) -> Result<NativePaymentReceipt, String> {
    let hash = canonical_hash(hash)?;
    xrp_rpc::outcall_read_quorum(
        xrp_rpc::read_provider_urls(key_name),
        |url| {
            xrp_rpc::build_request_to_url(
                url,
                "tx",
                json!({"transaction":hash,"binary":false,"api_version":2}),
                xrp_rpc::MAX_TX_BYTES,
                "xrp_transform_deposit_receipt",
            )
        },
        |bodies| quorum(bodies, &hash),
    )
    .await
}

#[cfg(test)]
mod sec11_tests {
    use super::*;
    fn fixture(v2: bool) -> Value {
        let tx = json!({"Account":"rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh", "Destination":"rPT1Sjq2YGrBMTttX4GZHjKu9dyfzbpAYe", "TransactionType":"Payment", "Amount":"10000", "Flags":0});
        let mut result = json!({"validated":true, "hash":"AA".repeat(32), "ledger_index":101, "ledger_hash":"BB".repeat(32), "meta":{"TransactionResult":"tesSUCCESS","delivered_amount":"10000"}});
        if v2 {
            let mut tx = tx;
            tx["DeliverMax"] = tx["Amount"].take();
            tx.as_object_mut().unwrap().remove("Amount");
            result["tx_json"] = tx;
        } else {
            result
                .as_object_mut()
                .unwrap()
                .extend(tx.as_object().unwrap().clone());
        }
        json!({"result":result})
    }
    fn reduced(v: &Value) -> Vec<u8> {
        transform(TransformArgs {
            context: vec![],
            response: HttpResponse {
                status: 200u32.into(),
                headers: vec![],
                body: serde_json::to_vec(v).unwrap(),
            },
        })
        .body
    }
    #[test]
    fn sec11_v1_v2_receipts_agree_and_quorum_binds_every_consumed_field() {
        let a = reduced(&fixture(false));
        let b = reduced(&fixture(true));
        let hash = "aa".repeat(32);
        assert_eq!(parse(&a, &hash).unwrap(), parse(&b, &hash).unwrap());
        assert_eq!(
            quorum(&[a.clone(), b], &hash).unwrap().delivered_drops,
            10000
        );
        for field in [
            "ledger_hash",
            "source",
            "destination",
            "ledger_index",
            "delivered",
            "flags",
        ] {
            let mut v: Value = serde_json::from_slice(&a).unwrap();
            v[field] = match field {
                "ledger_hash" => json!("CC".repeat(32)),
                "source" => json!("rPT1Sjq2YGrBMTttX4GZHjKu9dyfzbpAYe"),
                "destination" => json!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh"),
                "ledger_index" => json!(102),
                "flags" => json!(0x80000000u32),
                _ => json!("9999"),
            };
            assert!(
                quorum(&[a.clone(), serde_json::to_vec(&v).unwrap()], &hash).is_err(),
                "{field}"
            );
        }
    }
    #[test]
    fn sec11_receipt_rejects_unvalidated_failed_partial_self_issued_missing_and_overflow() {
        let body = reduced(&fixture(false));
        let valid: Value = serde_json::from_slice(&body).unwrap();
        for (field, bad) in [
            ("validated", json!(false)),
            ("result", json!("tecPATH_DRY")),
            ("kind", json!("TrustSet")),
            ("flags", json!(0x00020000u32)),
            ("delivered", json!({"currency":"USD","value":"10000"})),
            ("delivered", json!("unavailable")),
            ("delivered", json!("0")),
            ("delivered", json!("18446744073709551616")),
            ("ledger_index", json!(u64::from(u32::MAX) + 1)),
            ("ledger_index", json!(0)),
            ("ledger_hash", Value::Null),
            ("tx_hash", json!("CC".repeat(32))),
            ("send_max", json!({"currency":"USD","value":"1"})),
            ("destination", valid["source"].clone()),
        ] {
            let mut v = valid.clone();
            v[field] = bad;
            assert!(
                parse(&serde_json::to_vec(&v).unwrap(), &"AA".repeat(32)).is_err(),
                "{field}: {v}"
            );
        }
    }
}
