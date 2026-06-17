//! `XrpAdapter`: `ChainAdapter` for the native XRP Ledger (mirrors
//! `chains::solana::adapter`).
//!
//! ## Model
//! XRP is a foreign COLLATERAL chain. The protocol controls an XRPL settlement
//! account (threshold-Ed25519, `settlement_derivation_path`) that holds deposited
//! XRP; icUSD itself is IC-native, so there is nothing to mint/burn on the XRPL.
//!
//! - `sign_withdrawal`: builds + threshold-signs a native-XRP `Payment` from the
//!   settlement account to the recipient (the withdraw flow from the field guide:
//!   read sequence/balance/reserve, reserve-check, build, sign `STX\0 ‖ blob`,
//!   insert `TxnSignature`, re-serialize). Returns the signed blob plus the
//!   LOCALLY computed tx hash — never a single RPC node's reported hash. The
//!   broadcaster submits the blob via `xrp_rpc::submit_blob`.
//! - `verify_deposit`: confirms a deposit Payment validated via `xrp_rpc::tx`.
//! - `fetch_finality`: an XRPL validated ledger IS final, so latest == finalized.
//! - `sign_mint` / `sign_burn`: `NotImplemented` (no icUSD token on the XRPL).
//! - `observe_event`: delegated to a future observer (wiring), like Solana.
//!
//! ## Destination tags
//! The shared `WithdrawalRequest` has no tag field, so the trait method withdraws
//! with `None`. The tag-bearing `sign_withdrawal_with_tag` is exposed for the
//! future wiring so exchange withdrawals (which require a tag) work without a
//! cross-chain trait change. The codec and its tests support `Some(0)` from day
//! one (see `codec`).

use async_trait::async_trait;
use candid::Principal;

use crate::chains::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot, MintInstruction, SignedBurn,
    SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use crate::chains::config::ChainId;

use super::xrp_rpc::{self, XrpAccountInfo, XrpTxStatus};
use super::{address, codec, sign, ted25519};

/// Fixed Phase-1 fee in drops (2x the 10-drop reference base fee; deterministic
/// across replicas — a dynamic open-ledger fee is harder to make consensus-safe
/// and is Phase 2).
pub const XRP_FEE_DROPS: u64 = 20;

/// Ledgers of validity window for a withdrawal (~3-4s each); bounds tx validity
/// so a stuck tx cannot replay later.
const LAST_LEDGER_BUFFER: u32 = 75;

/// Max valid drops amount (1e17 = 100 billion XRP, the total supply). Guards the
/// `Amount` encoding on BOTH the fixed and max-send paths so a buggy/lying RPC
/// balance can never feed an out-of-range value into the encoder.
const MAX_DROPS: u128 = 100_000_000_000_000_000;

/// Adapter binding the XRP Ledger to the Rumi protocol.
pub struct XrpAdapter {
    chain_id: ChainId,
}

impl XrpAdapter {
    /// Create a new `XrpAdapter`. In production this is `XRP_CHAIN_ID` (144);
    /// tests may pass an arbitrary id for isolation.
    pub fn new(chain_id: ChainId) -> Self {
        XrpAdapter { chain_id }
    }

    /// Build + threshold-sign a native-XRP withdrawal from the protocol SETTLEMENT
    /// address, with an OPTIONAL destination tag. The trait `sign_withdrawal`
    /// delegates here with `None`.
    pub async fn sign_withdrawal_with_tag(
        &self,
        recipient: &str,
        amount_drops_u128: u128,
        destination_tag: Option<u32>,
    ) -> Result<SignedWithdrawal, ChainAdapterError> {
        let path = ted25519::settlement_derivation_path(self.chain_id);
        self.sign_xrp_payment_from(path, recipient, amount_drops_u128, destination_tag)
            .await
            .map(|(signed, _last_ledger_sequence)| signed)
    }

    /// Build + threshold-sign a native-XRP `Payment` from an ARBITRARY custody
    /// derivation path — the settlement address, or a per-vault custody address for
    /// P4 claim settlement. Validates the recipient + amount at the boundary,
    /// derives the source address from `derivation_path`, reads its sequence/balance
    /// + the base reserve, reserve-checks + builds the Payment, signs
    /// `STX\0 ‖ blob` directly (Ed25519, no prehash), and returns the signed blob +
    /// the LOCAL tx hash. The caller submits the blob (`xrp_rpc::submit_blob`).
    pub async fn sign_xrp_payment_from(
        &self,
        derivation_path: Vec<Vec<u8>>,
        recipient: &str,
        amount_drops_u128: u128,
        destination_tag: Option<u32>,
    ) -> Result<(SignedWithdrawal, u32), ChainAdapterError> {
        let dest_id = decode_recipient(recipient)?;
        let amount_drops = checked_drops_u64(amount_drops_u128)?;

        let (pubkey, sender_addr) = ted25519::derive_xrp_address(derivation_path.clone())
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        let sender_id = address::account_id_from_classic_address(&sender_addr)
            .map_err(ChainAdapterError::SignatureFailed)?;

        let acct = xrp_rpc::fetch_account_info(&sender_addr)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "rippled".to_string(),
                message,
            })?;
        let reserve = xrp_rpc::fetch_reserve_base()
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "rippled".to_string(),
                message,
            })?;

        let payment = build_withdrawal_payment(
            sender_id,
            dest_id,
            &pubkey,
            &acct,
            reserve,
            amount_drops,
            destination_tag,
        )?;

        let unsigned = codec::serialize_unsigned(&payment);
        let message = sign::signing_message(&unsigned);
        let signature = ted25519::sign_message(message, derivation_path)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        let last_ledger_sequence = payment.last_ledger_sequence;
        let signed = codec::serialize_signed(&payment, &signature);
        let tx_hash = hex::encode_upper(sign::tx_hash(&signed));
        Ok((
            SignedWithdrawal {
                raw_tx: signed,
                tx_hash,
            },
            last_ledger_sequence,
        ))
    }
}

// ─── pure helpers (synchronous; unit-tested) ─────────────────────────────────

/// Decode + validate a recipient classic address at the trust boundary, mapping
/// any malformed input (X-address, wrong version, bad checksum, empty) to
/// `InvalidPayload` so a bad address NEVER panics deep in tx-building.
pub(crate) fn decode_recipient(addr: &str) -> Result<[u8; 20], ChainAdapterError> {
    address::account_id_from_classic_address(addr)
        .map_err(|e| ChainAdapterError::InvalidPayload(format!("invalid recipient address: {e}")))
}

/// Checked u128 (drops) -> u64, mapping overflow to `InvalidPayload`. XRPL amounts
/// are u64 drops on the wire.
pub(crate) fn checked_drops_u64(drops: u128) -> Result<u64, ChainAdapterError> {
    u64::try_from(drops)
        .map_err(|_| ChainAdapterError::InvalidPayload("amount exceeds u64 drops".to_string()))
}

/// Validate funds against the base reserve + fee and build the `Payment`. Pure so
/// the financial logic is unit-tested directly (the async signing/broadcast path
/// is covered by a future PocketIC test, mirroring the Solana adapter).
pub(crate) fn build_withdrawal_payment(
    sender_id: [u8; 20],
    dest_id: [u8; 20],
    pubkey: &[u8; 32],
    acct: &XrpAccountInfo,
    reserve_drops: u128,
    amount_drops: u64,
    destination_tag: Option<u32>,
) -> Result<codec::Payment, ChainAdapterError> {
    if amount_drops == 0 {
        return Err(ChainAdapterError::InvalidPayload(
            "amount_drops must be > 0".to_string(),
        ));
    }
    if u128::from(amount_drops) > MAX_DROPS {
        return Err(ChainAdapterError::InvalidPayload(
            "amount_drops exceeds max supply".to_string(),
        ));
    }
    if !acct.exists {
        return Err(ChainAdapterError::InvalidPayload(
            "settlement XRP account is unfunded".to_string(),
        ));
    }
    let need = u128::from(amount_drops) + u128::from(XRP_FEE_DROPS) + reserve_drops;
    if acct.balance_drops < need {
        return Err(ChainAdapterError::InvalidPayload(format!(
            "insufficient XRP: balance {} drops < amount {} + fee {} + reserve {}",
            acct.balance_drops, amount_drops, XRP_FEE_DROPS, reserve_drops
        )));
    }
    Ok(codec::Payment {
        account: sender_id,
        destination: dest_id,
        amount_drops,
        fee_drops: XRP_FEE_DROPS,
        sequence: acct.sequence,
        last_ledger_sequence: acct.ledger_index + LAST_LEDGER_BUFFER,
        destination_tag,
        signing_pub_key: sign::ed25519_signing_pubkey(pubkey),
    })
}

// ─── ChainAdapter impl ───────────────────────────────────────────────────────

#[async_trait(?Send)]
impl ChainAdapter for XrpAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    /// Confirm `tx_hash` validated and succeeded on the XRPL. Returns a
    /// `DepositRecord` with `block_number` = the validating ledger index and
    /// `amount_e8s` = the partial-payment-safe `delivered_amount` in DROPS (the
    /// field carries native units here, mirroring the lamports/wei wart in the
    /// Solana/Monad adapters — it is NOT 8-decimal e8s). Crediting
    /// `delivered_amount` (never the Payment `Amount`) closes the XRPL
    /// partial-payment drainer. `depositor` is left empty: the per-vault custody
    /// address identifies the vault; a future observer can fill it.
    async fn verify_deposit(&self, tx_hash: &str) -> Result<DepositRecord, ChainAdapterError> {
        match xrp_rpc::fetch_tx_status(tx_hash).await {
            Ok(XrpTxStatus::Validated {
                ledger_index,
                delivered_drops,
            }) => Ok(DepositRecord {
                depositor: String::new(),
                amount_e8s: delivered_drops,
                block_number: u64::from(ledger_index),
                tx_hash: tx_hash.to_string(),
            }),
            Ok(XrpTxStatus::NotFound) => Err(ChainAdapterError::InvalidPayload(
                "deposit not validated".to_string(),
            )),
            Ok(XrpTxStatus::Failed) => Err(ChainAdapterError::InvalidPayload(
                "deposit failed".to_string(),
            )),
            Err(message) => Err(ChainAdapterError::RpcError {
                provider: "rippled".to_string(),
                message,
            }),
        }
    }

    /// Build + threshold-sign a native-XRP `Payment`. `req.amount_e8s` carries the
    /// XRP amount in its native denomination (drops), mirroring the Monad/Solana
    /// adapters' wei/lamports wart in the same field. Withdraws with no
    /// destination tag (use `sign_withdrawal_with_tag` when wiring tagged
    /// exchange withdrawals).
    async fn sign_withdrawal(
        &self,
        req: WithdrawalRequest,
    ) -> Result<SignedWithdrawal, ChainAdapterError> {
        self.sign_withdrawal_with_tag(&req.recipient, req.amount_e8s, None)
            .await
    }

    /// icUSD is IC-native: there is no icUSD token on the XRPL to mint.
    async fn sign_mint(&self, _instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    /// icUSD is IC-native: there is no icUSD token on the XRPL to burn.
    async fn sign_burn(
        &self,
        _amount_e8s: u128,
        _burner: Principal,
    ) -> Result<SignedBurn, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    /// An XRPL validated ledger IS final (no block-depth confirmation), so latest
    /// == finalized. Reads the validated ledger index via the settlement account
    /// (`account_info` returns a ledger index even for an unfunded account).
    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        let path = ted25519::settlement_derivation_path(self.chain_id);
        let (_pubkey, addr) = ted25519::derive_xrp_address(path)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        let acct = xrp_rpc::fetch_account_info(&addr)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "rippled".to_string(),
                message,
            })?;
        let ledger = u64::from(acct.ledger_index);
        Ok(FinalitySnapshot {
            latest_block: ledger,
            finalized_block: ledger,
        })
    }

    /// Deposit observation is handled by a future dedicated observer (wiring),
    /// which holds cursor state and decodes amounts/depositor. Mirrors Solana.
    async fn observe_event(
        &self,
        from_block: u64,
    ) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        let _ = from_block;
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KAT: &str = include_str!("testdata/xrp_kat.json");

    fn kat_addr(field: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(KAT).unwrap();
        v["payment"][field].as_str().unwrap().to_string()
    }

    fn ids() -> ([u8; 20], [u8; 20], [u8; 32]) {
        let sender = address::account_id_from_classic_address(&kat_addr("account")).unwrap();
        let dest = address::account_id_from_classic_address(&kat_addr("destination")).unwrap();
        (sender, dest, [0xea; 32])
    }

    fn acct(seq: u32, bal: u128) -> XrpAccountInfo {
        XrpAccountInfo {
            exists: true,
            sequence: seq,
            balance_drops: bal,
            ledger_index: 9_000_000,
        }
    }

    #[test]
    fn decode_recipient_rejects_bad_address() {
        assert!(matches!(
            decode_recipient("not-an-address"),
            Err(ChainAdapterError::InvalidPayload(_))
        ));
        // A valid KAT address decodes.
        assert!(decode_recipient(&kat_addr("destination")).is_ok());
    }

    #[test]
    fn checked_drops_u64_rejects_overflow() {
        assert_eq!(checked_drops_u64(1_000_000).unwrap(), 1_000_000);
        assert!(checked_drops_u64(u128::from(u64::MAX) + 1).is_err());
    }

    #[test]
    fn build_payment_happy_path_sets_sequence_and_lls() {
        let (s, d, pk) = ids();
        let p =
            build_withdrawal_payment(s, d, &pk, &acct(42, 50_000_000), 1_000_000, 1_000_000, Some(99))
                .unwrap();
        assert_eq!(p.sequence, 42);
        assert_eq!(p.last_ledger_sequence, 9_000_000 + LAST_LEDGER_BUFFER);
        assert_eq!(p.fee_drops, XRP_FEE_DROPS);
        assert_eq!(p.destination_tag, Some(99));
        assert_eq!(p.signing_pub_key[0], 0xED);
    }

    #[test]
    fn build_payment_rejects_zero_amount() {
        let (s, d, pk) = ids();
        assert!(matches!(
            build_withdrawal_payment(s, d, &pk, &acct(1, 50_000_000), 1_000_000, 0, None),
            Err(ChainAdapterError::InvalidPayload(_))
        ));
    }

    #[test]
    fn build_payment_respects_reserve_and_fee() {
        let (s, d, pk) = ids();
        // balance 1_500_000; reserve 1_000_000; fee 20; amount 600_000 -> over budget
        assert!(matches!(
            build_withdrawal_payment(s, d, &pk, &acct(1, 1_500_000), 1_000_000, 600_000, None),
            Err(ChainAdapterError::InvalidPayload(_))
        ));
        // Exactly affordable: balance == amount + fee + reserve.
        let need = 600_000u128 + u128::from(XRP_FEE_DROPS) + 1_000_000;
        assert!(build_withdrawal_payment(
            s,
            d,
            &pk,
            &acct(1, need),
            1_000_000,
            600_000,
            None
        )
        .is_ok());
    }

    #[test]
    fn build_payment_rejects_unfunded() {
        let (s, d, pk) = ids();
        let unfunded = XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index: 9_000_000,
        };
        assert!(matches!(
            build_withdrawal_payment(s, d, &pk, &unfunded, 1_000_000, 1_000_000, None),
            Err(ChainAdapterError::InvalidPayload(_))
        ));
    }

    #[test]
    fn build_payment_guards_max_drops() {
        let (s, d, pk) = ids();
        let over = (MAX_DROPS + 1) as u64;
        // balance large enough that only the MAX_DROPS guard can trip.
        let a = XrpAccountInfo {
            exists: true,
            sequence: 1,
            balance_drops: u128::MAX,
            ledger_index: 9_000_000,
        };
        assert!(matches!(
            build_withdrawal_payment(s, d, &pk, &a, 0, over, None),
            Err(ChainAdapterError::InvalidPayload(_))
        ));
    }

    #[test]
    fn chain_id_round_trips() {
        let a = XrpAdapter::new(super::super::config::XRP_CHAIN_ID);
        assert_eq!(a.chain_id(), super::super::config::XRP_CHAIN_ID);
    }
}
