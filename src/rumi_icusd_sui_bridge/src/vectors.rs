//! Golden vectors, test builds only. Every value here was produced by
//! `reference/sui_golden_reference.py` (standard library only, RFC 8032 Ed25519 written out
//! with its own test vector, BLAKE2b from `hashlib`), not by this crate. The Python script is the
//! source of truth: rerun `python3 -I reference/sui_golden_reference.py` to regenerate them.

use crate::gate::ConfiguredGate;
use crate::tx::RoleAddresses;
use crate::sui::Bytes32;

pub const CHAIN_A: Bytes32 = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
];

/// Deterministic TEST seeds (public constants, never keys of any account).
pub const SEED_TREASURY: [u8; 32] = [0x11; 32];
pub const SEED_GAS: [u8; 32] = [0x22; 32];
pub const SEED_CUSTODY: [u8; 32] = [0x33; 32];

pub const PUB_TREASURY: &str = "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737";
pub const PUB_GAS: &str = "a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f0";
pub const PUB_CUSTODY: &str = "17cb79fb2b4120f2b1ec65e4198d6e08b28e813feb01e4a400839b85e18080ce";
pub const ADDR_TREASURY: &str = "0881c07520943bbf13989b92892093c1b50672156fa5f873c22892701cb2e207";
pub const ADDR_GAS: &str = "48936240c871663e6ba416724158736101ccab29436f72ae3ae4c8fa1ce7e45f";
pub const ADDR_CUSTODY: &str = "11fd1b265883bba07e9e04a95997603dadaf15738badbe9070fa5e06675df497";

// The sample mint transaction (see `mint_transaction()` in the Python reference).
pub const PACKAGE: Bytes32 = [0x70; 32];
pub const GATE_ID: Bytes32 = [0x6a; 32];
pub const GATE_INITIAL_SHARED_VERSION: u64 = 42;
pub const MINT_CAP_ID: Bytes32 = [0x6d; 32];
pub const ADMIN_CAP_ID: Bytes32 = [0x6e; 32];
pub const MINT_CAP_VERSION: u64 = 77;
pub const MINT_CAP_DIGEST: Bytes32 = [0x55; 32];
pub const GAS_COIN_ID: Bytes32 = [0x67; 32];
pub const GAS_COIN_VERSION: u64 = 9;
pub const GAS_COIN_DIGEST: Bytes32 = [0x56; 32];
pub const GAS_PRICE: u64 = 1000;
pub const GAS_BUDGET: u64 = 5_000_000;
pub const EPOCH: u64 = 123;
pub const RECIPIENT: Bytes32 = [0x8b; 32];
pub const AMOUNT: u64 = 250 * 100_000_000;

pub fn mint_id_bytes() -> Bytes32 {
    let mut id = [0u8; 32];
    for (i, b) in id.iter_mut().enumerate() {
        *b = 0x80 + i as u8;
    }
    id
}

pub const TX_BCS: &str = "00000501016a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a2a000000000000000101006d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d4d00000000000000205555555555555555555555555555555555555555555555555555555555555555002120808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f00208b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b000800ba1dd205000000010070707070707070707070707070707070707070707070707070707070707070700f72756d695f69637573645f67617465046d696e7400050100000101000102000103000104000881c07520943bbf13989b92892093c1b50672156fa5f873c22892701cb2e207016767676767676767676767676767676767676767676767676767676767676767090000000000000020565656565656565656565656565656565656565656565656565656565656565648936240c871663e6ba416724158736101ccab29436f72ae3ae4c8fa1ce7e45fe803000000000000404b4c0000000000017b00000000000000";
pub const SIGNING_DIGEST: &str = "8aca64db93860d7464e3cd22814bdec3c0a0109b71baec5d244d12904c270868";
pub const TRANSACTION_DIGEST: &str = "bd3d1d0eb0fff7b862148c1096759c1e8949c77a0216f0b3a01353a5cd59cc41";
pub const TRANSACTION_DIGEST_B58: &str = "Dji72ky3XnNaYDaXTvoN1Dikgi56SXAdrUreR2TNRNpQ";
pub const SIG_TREASURY: &str = "fe7ebe21c00566d3aedda1ee819e865ef1ee92f26568d8be0cfe8854ed2a5c935a2a7aeb940fb0d5f899135b612c6a020a2dc4a189160784c3d77bd4168f9602";
pub const SIG_GAS: &str = "810400c34fdb45bbe25abe4120bcbfcb124c71f2efda64695dd4d08bf02a1be08de1305a17b8e6fcfb50b48b0f737572a144536e6773fa27db66ee072957f205";
pub const WIRE_TREASURY_B64: &str = "AP5+viHABWbTrt2h7oGehl7x7pLyZWjYvgz+iFTtKlyTWip665QPsNX4mRNbYSxqAgotxKGJFgeEw9d71BaPlgLQSrIydCu0qzoTaL1GFeTm0CJKtxoBa6+FIKMyyXeHNw==";
pub const WIRE_GAS_B64: &str = "AIEEAMNP20W74lq+QSC8v8sSTHHy79pkaV3U0IvwKhvgjeEwWhe45vz7ULSLD3N1cqFEU25nc/on22buBylX8gWgmqX0emdZgC/5VfjcLSoUpcmdI76X+GQSf/k4NFWk8A==";

pub const GATE_BCS: &str = "6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a0100000000000000dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd00902f5009000000204c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b000e87648170000000100000000000000004a4d220f000000000000000000000000ba1dd205000000000000000000000004000000000000000300000000000000e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e10400000000000000e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e20300000000000000";
pub const GATE_BCS_B64: &str = "ampqampqampqampqampqampqampqampqampqampqamoBAAAAAAAAAN3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3dAJAvUAkAAAAgTHitrPKi9a2A8n7X1UqmnTp48cpn/e+ez1dU9bi7d7AA6HZIFwAAAAEAAAAAAAAAAEpNIg8AAAAAAAAAAAAAAAC6HdIFAAAAAAAAAAAAAAAEAAAAAAAAAAMAAAAAAAAA4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eHh4eEEAAAAAAAAAOLi4uLi4uLi4uLi4uLi4uLi4uLi4uLi4uLi4uLi4uLiAwAAAAAAAAA=";
pub const GATE_BCS_VERSION_2: &str = "6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a0200000000000000dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd00902f5009000000204c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b000e87648170000000100000000000000004a4d220f000000000000000000000000ba1dd205000000000000000000000004000000000000000300000000000000e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e10400000000000000e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e20300000000000000";
pub const MINT_RECORD_BCS: &str = "8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b00ba1dd2050000000400000000000000";
pub const RETURN_RECORD_BCS: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb00ba1dd20500000001000000000000001d0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d206465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182832a00000000000000";
/// `bcs(vector<u8>)` of the sample mint id, base64: the `DynamicFieldName.bcs` of its table entry.
pub const MINT_ID_NAME_B64: &str = "IICBgoOEhYaHiImKi4yNjo+QkZKTlJWWl5iZmpucnZ6f";

pub fn configured() -> ConfiguredGate {
    ConfiguredGate { package: PACKAGE, gate: GATE_ID, mint_cap: MINT_CAP_ID, admin_cap: ADMIN_CAP_ID }
}

pub fn h32(s: &str) -> Bytes32 {
    let v = hex::decode(s).expect("hex");
    <Bytes32>::try_from(v.as_slice()).expect("32 bytes")
}

/// The three test role addresses (from the Python reference; `tx` tests also derive them from the
/// seeds and compare).
pub fn roles() -> RoleAddresses {
    RoleAddresses { treasury: h32(ADDR_TREASURY), gas: h32(ADDR_GAS), upgrade_custody: h32(ADDR_CUSTODY) }
}
