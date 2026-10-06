//! Exact proof checks for collateral returned by the liquidation bot.
//!
//! A balance on a publicly derivable backend subaccount is not evidence that
//! the configured bot returned the claim collateral. Cancellation therefore
//! authenticates the exact ledger block before consolidating that account.

use candid::{CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;

/// Derive the bot's fee-buffered return credit and the exact backend
/// consolidation debit/credit for a claim generation.
pub fn fee_buffered_return_budget(collateral_amount: u64, fee: u64) -> Result<(u64, u64), String> {
    let escrow_credit = collateral_amount.checked_add(fee)
        .ok_or_else(|| "fee-buffered return credit overflows".to_string())?;
    let backend_credit = escrow_credit.checked_sub(fee)
        .ok_or_else(|| "return escrow cannot pay the pinned consolidation fee".to_string())?;
    if backend_credit != collateral_amount {
        return Err("fee-buffered return does not preserve claim collateral".into());
    }
    Ok((escrow_credit, backend_credit))
}

pub async fn verify_return_block(
    ledger: Principal,
    bot: Principal,
    backend: Principal,
    return_subaccount: [u8; 32],
    vault_id: u64,
    claim_timestamp: u64,
    collateral_amount: u64,
    expected_fee: u64,
    block_index: u64,
    created_at_time: u64,
    native_icp: bool,
) -> Result<(), String> {
    if created_at_time < claim_timestamp {
        return Err("return tuple timestamp predates its claim generation".into());
    }
    let memo = return_memo(vault_id, claim_timestamp);
    let destination = Account {
        owner: backend,
        subaccount: Some(return_subaccount),
    };
    if native_icp {
        let block = fetch_native_icp_block(ledger, block_index).await?;
        return validate_native_icp_block(
            ledger,
            bot,
            destination,
            collateral_amount,
            expected_fee,
            &memo,
            created_at_time,
            &block,
        )
        .await;
    }

    let decoded = crate::icrc3_proof::fetch_icrc3_block(ledger, block_index).await?;
    validate_icrc3_return_block(
        &decoded,
        bot,
        destination,
        collateral_amount,
        expected_fee,
        &memo,
        created_at_time,
    )
}

pub fn return_memo(vault_id: u64, claim_timestamp: u64) -> Vec<u8> {
    let mut memo = b"BOTRET02".to_vec();
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo.extend_from_slice(&claim_timestamp.to_be_bytes());
    memo
}

pub fn validate_icrc3_return_block(
    block: &crate::icrc3_proof::DecodedBlock,
    bot: Principal,
    destination: Account,
    collateral_amount: u64,
    expected_fee: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    if block.op != "transfer" && block.op != "xfer" {
        return Err("return receipt is not a transfer block".into());
    }
    if block.from.as_ref()
        != Some(&Account {
            owner: bot,
            subaccount: None,
        })
    {
        return Err("return receipt sender is not the configured bot default account".into());
    }
    if block.to.as_ref() != Some(&destination) {
        return Err("return receipt destination is not the exact claim return account".into());
    }
    if block.memo.as_deref() != Some(memo) {
        return Err("return receipt memo does not match this claim generation".into());
    }
    if block.created_at_time != Some(created_at_time) {
        return Err("return receipt timestamp differs from the persisted bot tuple".into());
    }
    let fee = block
        .fee
        .ok_or("return receipt does not expose its charged ledger fee")?;
    if block
        .transaction_fee
        .is_some_and(|requested| requested != fee)
    {
        return Err("return receipt requested and charged fees differ".into());
    }
    let expected_amount = collateral_amount.checked_add(expected_fee)
        .ok_or("expected return amount plus fee overflows")?;
    if fee != u128::from(expected_fee) || block.amount != u128::from(expected_amount) {
        return Err(
            "return receipt amount and charged fee do not match the fee-buffered claim return tuple".into(),
        );
    }
    Ok(())
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeTokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeTimestamp {
    timestamp_nanos: u64,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeGetBlocksArgs {
    start: u64,
    length: u64,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeOperation>,
    created_at_time: NativeTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeTransaction,
    timestamp: NativeTimestamp,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeOperation {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeTokens,
    },
    Mint {
        to: Vec<u8>,
        amount: NativeTokens,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeTokens,
        fee: NativeTokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: NativeTokens,
        expected_allowance: Option<NativeTokens>,
        fee: NativeTokens,
        expires_at: Option<NativeTimestamp>,
    },
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeBlockRange {
    blocks: Vec<NativeBlock>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
enum NativeArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}
type NativeArchiveResult = Result<NativeBlockRange, NativeArchiveError>;
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeArchivedRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<NativeGetBlocksArgs, NativeArchiveResult>,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
struct NativeQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<NativeBlock>,
    first_block_index: u64,
    archived_blocks: Vec<NativeArchivedRange>,
}

async fn fetch_native_icp_block(
    ledger: Principal,
    block_index: u64,
) -> Result<NativeBlock, String> {
    let request = NativeGetBlocksArgs {
        start: block_index,
        length: 1,
    };
    let (response,): (NativeQueryBlocksResponse,) =
        ic_cdk::call(ledger, "query_blocks", (request.clone(),))
            .await
            .map_err(|(code, message)| {
                format!("native ICP query_blocks failed: {code:?} {message}")
            })?;
    if response.blocks.len() > 1 {
        return Err("native ICP ledger returned multiple direct blocks".into());
    }
    if let Some(offset) = block_index
        .checked_sub(response.first_block_index)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset < response.blocks.len())
    {
        return Ok(response.blocks[offset].clone());
    }
    let mut covering = response.archived_blocks.iter().filter(|archive| {
        archive.length > 0
            && archive.start <= block_index
            && archive
                .start
                .checked_add(archive.length)
                .is_some_and(|end| block_index < end)
    });
    let archive = covering.next().ok_or_else(|| {
        format!("native ICP ledger returned no block/archive descriptor for {block_index}")
    })?;
    if covering.next().is_some() {
        return Err("native ICP ledger returned overlapping archive descriptors".into());
    }
    let (result,): (NativeArchiveResult,) = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (request,),
    )
    .await
    .map_err(|(code, message)| format!("native ICP archive call failed: {code:?} {message}"))?;
    match result {
        Ok(range) if range.blocks.len() == 1 => {
            Ok(range.blocks.into_iter().next().expect("one native block"))
        }
        Ok(_) => Err("native ICP archive did not return exactly one requested block".into()),
        Err(error) => Err(format!(
            "native ICP archive rejected block lookup: {error:?}"
        )),
    }
}

async fn validate_native_icp_block(
    ledger: Principal,
    bot: Principal,
    destination: Account,
    collateral_amount: u64,
    expected_fee: u64,
    memo: &[u8],
    created_at_time: u64,
    block: &NativeBlock,
) -> Result<(), String> {
    let operation = block
        .transaction
        .operation
        .as_ref()
        .ok_or("native ICP block operation is missing")?;
    let NativeOperation::Transfer {
        from,
        to,
        spender,
        amount,
        fee,
    } = operation
    else {
        return Err("native ICP return receipt is not a transfer".into());
    };
    if spender.is_some() || from.len() != 32 || to.len() != 32 {
        return Err(
            "native ICP return receipt has unexpected spender or malformed account IDs".into(),
        );
    }
    let (source_id,): (Vec<u8>,) = ic_cdk::call(
        ledger,
        "account_identifier",
        (Account {
            owner: bot,
            subaccount: None,
        },),
    )
    .await
    .map_err(|(code, message)| {
        format!("native ICP bot account lookup failed: {code:?} {message}")
    })?;
    let (destination_id,): (Vec<u8>,) = ic_cdk::call(ledger, "account_identifier", (destination,))
        .await
        .map_err(|(code, message)| {
            format!("native ICP return account lookup failed: {code:?} {message}")
        })?;
    if source_id.len() != 32
        || destination_id.len() != 32
        || *from != source_id
        || *to != destination_id
    {
        return Err(
            "native ICP return receipt source or destination differs from the claim tuple".into(),
        );
    }
    if block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err(
            "native ICP return receipt memo or timestamp differs from the persisted bot tuple"
                .into(),
        );
    }
    if fee.e8s != expected_fee
        || collateral_amount.checked_add(expected_fee) != Some(amount.e8s)
    {
        return Err(
            "native ICP return amount and fee do not match the fee-buffered claim tuple".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icrc3_proof::DecodedBlock;

    #[test]
    fn two_transfer_fee_buffer_preserves_exact_claim_collateral() {
        let collateral = 2_000_000u64;
        let fee = 10_000u64;
        let (escrow_credit, backend_credit) = fee_buffered_return_budget(collateral, fee).unwrap();
        let bot_debit = escrow_credit.checked_add(fee).unwrap();
        let consolidation_debit = escrow_credit;
        let consolidation_credit = consolidation_debit.checked_sub(fee).unwrap();
        assert_eq!(escrow_credit, collateral + fee);
        assert_eq!(bot_debit, collateral + 2 * fee);
        assert_eq!(consolidation_credit, collateral);
        assert_eq!(backend_credit, collateral);
    }

    #[test]
    fn return_receipt_requires_exact_fee_buffered_generation_return() {
        let bot = Principal::from_slice(&[1]);
        let backend = Principal::from_slice(&[2]);
        let destination = Account {
            owner: backend,
            subaccount: Some([3; 32]),
        };
        let memo = return_memo(7, 500);
        let valid = DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(Account {
                owner: bot,
                subaccount: None,
            }),
            to: Some(destination.clone()),
            spender: None,
            amount: 1_010,
            transaction_fee: Some(10),
            fee: Some(10),
            memo: Some(memo.clone()),
            created_at_time: Some(501),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(
            validate_icrc3_return_block(&valid, bot, destination.clone(), 1_000, 10, &memo, 501)
                .is_ok()
        );
        assert!(validate_icrc3_return_block(
            &valid,
            Principal::from_slice(&[4]),
            destination.clone(),
            1_000,
            10,
            &memo,
            501
        )
        .is_err());
        assert!(validate_icrc3_return_block(
            &valid,
            bot,
            destination.clone(),
            1_000,
            10,
            b"other",
            501
        )
        .is_err());
        assert!(
            validate_icrc3_return_block(&valid, bot, destination.clone(), 1_000, 10, &memo, 502)
                .is_err()
        );
        assert!(validate_icrc3_return_block(&valid, bot, destination.clone(), 1_001, 10, &memo, 501).is_err());
        assert!(validate_icrc3_return_block(&valid, bot, destination.clone(), 1_000, 9, &memo, 501).is_err());
        let mut donation_like = valid.clone();
        donation_like.amount = 1_000;
        assert!(validate_icrc3_return_block(&donation_like, bot, destination, 1_000, 10, &memo, 501).is_err());
    }
}
