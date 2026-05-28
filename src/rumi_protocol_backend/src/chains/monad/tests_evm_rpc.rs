use super::evm_rpc::{parse_hex_quantity, BurnLog};

#[test]
fn parses_hex_quantity() {
    assert_eq!(parse_hex_quantity("0x0").unwrap(), 0u128);
    assert_eq!(parse_hex_quantity("0x10").unwrap(), 16u128);
    assert_eq!(parse_hex_quantity("0x2540be400").unwrap(), 10_000_000_000u128); // 100 icUSD @ 8dp
    assert!(parse_hex_quantity("not-hex").is_err());
}

#[test]
fn decodes_burn_log() {
    // Burn(uint256 vault_id, address burner, uint256 amount):
    //   topics[0] = keccak("Burn(uint256,address,uint256)")
    //   topics[1] = vault_id (indexed), topics[2] = burner (indexed), data = amount
    let topic0 = super::evm_rpc::BURN_EVENT_TOPIC0.to_string();
    let vault_id_topic = format!("0x{:064x}", 7u64);
    let burner_topic = format!("0x{:064x}", 0u8);
    let amount_data = format!("0x{:064x}", 10_000_000_000u128);
    let log = BurnLog::from_raw(&[topic0, vault_id_topic, burner_topic], &amount_data, "0xtxhash", 110)
        .expect("decode burn");
    assert_eq!(log.vault_id, 7);
    assert_eq!(log.amount_e8s, 10_000_000_000);
    assert_eq!(log.block_number, 110);
}

#[test]
fn rejects_log_with_wrong_topic0() {
    let res = BurnLog::from_raw(
        &["0xdeadbeef".into(), format!("0x{:064x}", 1u64), format!("0x{:064x}", 0u8)],
        &format!("0x{:064x}", 1u128), "0xtx", 1);
    assert!(res.is_err());
}
