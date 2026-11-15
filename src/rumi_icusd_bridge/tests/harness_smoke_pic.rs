//! BB-09a harness smoke test.
//!
//! Proves the plumbing later bridge scenario tests rely on, WITHOUT touching the
//! bridge library: a local anvil running the real `BridgedIcUSD` contract, a
//! PocketIC instance with an HTTPS-outcall relay canister wired to that anvil,
//! threshold ECDSA availability, and the real ICRC ledger with archiving.
//!
//! Prerequisites (all local):
//!   cargo build --offline --release --target wasm32-unknown-unknown -p evm_rpc_relay
//!   (cd foundry && ~/.foundry/bin/forge build --offline)     # only if foundry/out is missing
//!   POCKET_IC_BIN=<absolute path to pocket-ic server>
//!   anvil at $ANVIL_BIN or ~/.foundry/bin/anvil
//!
//! Run:
//!   POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge \
//!     --test harness_smoke_pic -- --nocapture --test-threads=1

mod support;

use candid::{Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use serde_json::json;
use support::anvil::{Anvil, ANVIL_CHAIN_ID};
use support::contract::{
    bytes32_from_u64, decode_bridge_mint, decode_bridge_out, logs_with_topic, BridgedIcUsd,
    DeployArgs, TOPIC_BRIDGE_MINT, TOPIC_BRIDGE_OUT, VERSION_STRING,
};
use support::ledger::{self, account, deploy_icrc_ledger, LedgerConfig, TRANSFER_FEE_E8S};
use support::outcalls::{FaultAction, ProviderRouter, Served, PROVIDER_A, PROVIDER_B, PROVIDER_C};
use support::pic::{build_pic, deploy_relay, test_principal};
use support::relay::{
    self, HttpOutcallError, ProviderError, RejectionCode, RequestCostResult, RequestResult,
    RpcError, RpcService,
};

/// Deploy BridgedIcUSD on a fresh anvil with distinct admin / minter / holder
/// accounts taken from anvil's unlocked defaults.
struct World {
    anvil: Anvil,
    token: BridgedIcUsd,
    admin: String,
    minter: String,
    holder: String,
}

fn world() -> World {
    let anvil = Anvil::start();
    assert!(
        anvil.accounts.len() >= 4,
        "anvil should expose >= 4 default accounts"
    );
    let (deployer, admin, minter, holder) = (
        anvil.accounts[0].clone(),
        anvil.accounts[1].clone(),
        anvil.accounts[2].clone(),
        anvil.accounts[3].clone(),
    );
    let token = BridgedIcUsd::deploy(&anvil, &deployer, &DeployArgs::permissive(&admin, &minter));
    World {
        anvil,
        token,
        admin,
        minter,
        holder,
    }
}

// ---------------------------------------------------------------------------
// (a) anvil boots; `finalized` / `safe` tag semantics measured
// ---------------------------------------------------------------------------

#[test]
fn anvil_boots_and_finalized_tag_semantics() {
    let anvil = Anvil::start();
    assert_eq!(anvil.rpc("eth_chainId", json!([])), json!("0x7a69"));
    assert_eq!(anvil.chain_id(), ANVIL_CHAIN_ID);

    // Measure the relationship at several heights, including the saturating
    // region near genesis.
    let mut table = Vec::new();
    let read = |anvil: &Anvil| {
        (
            anvil.block_number("latest"),
            anvil.block_number("safe"),
            anvil.block_number("finalized"),
        )
    };
    table.push(read(&anvil));
    for _ in 0..8 {
        anvil.mine(1);
        table.push(read(&anvil));
    }
    anvil.mine(5);
    table.push(read(&anvil));
    println!("anvil (latest, safe, finalized) as the chain grows:");
    for (l, s, f) in &table {
        println!("  latest={l:<3} safe={s:<3} finalized={f:<3}");
    }
    // With --slots-in-an-epoch 1: safe = latest - 1, finalized = latest - 2 (saturating at 0).
    for (latest, safe, finalized) in &table {
        assert_eq!(
            *safe,
            latest.saturating_sub(1),
            "safe = latest - slots_in_an_epoch"
        );
        assert_eq!(
            *finalized,
            latest.saturating_sub(2),
            "finalized = latest - 2 * slots_in_an_epoch"
        );
    }
    let (latest, safe, finalized) = *table.last().unwrap();
    assert_eq!(latest, 13, "genesis + 8 + 5 mined blocks");
    assert_eq!((safe, finalized), (12, 11));
}

// ---------------------------------------------------------------------------
// (b) BridgedIcUSD deploys with distinct admin / minter
// ---------------------------------------------------------------------------

#[test]
fn bridged_icusd_deploys_with_distinct_admin_and_minter() {
    let w = world();
    assert_ne!(w.admin, w.minter);
    assert_eq!(w.token.version(&w.anvil), VERSION_STRING);
    assert_eq!(w.token.version(&w.anvil), "rumi-base-icusd-bridge-v1");
    assert_eq!(w.token.total_supply(&w.anvil), 0);
    assert_eq!(w.token.bridge_out_count(&w.anvil), 0);
    assert!(!w.token.mint_consumed(&w.anvil, bytes32_from_u64(1)));
    println!(
        "BridgedIcUSD deployed at {} (tx {})",
        w.token.address, w.token.deploy_tx
    );
}

// ---------------------------------------------------------------------------
// (c) relay canister + outcall pump: real passthrough and fault injection
// ---------------------------------------------------------------------------

#[test]
fn relay_passthrough_to_anvil_and_fault_injection() {
    let anvil = Anvil::start();
    let pic = build_pic();
    let relay_id = deploy_relay(&pic);
    let mut router = ProviderRouter::new(anvil.endpoint());
    let chain_id_req = relay::jsonrpc("eth_chainId", json!([]));

    // REAL passthrough: provider-a -> anvil.
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_A),
        &chain_id_req,
        2000,
    );
    let RequestResult::Ok(body) = res else {
        panic!("expected Ok, got {res:?}")
    };
    let v: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(v["result"], "0x7a69", "real anvil answer: {body}");
    assert_eq!(router.count(PROVIDER_A, "eth_chainId"), 1);
    assert_eq!(router.log().last().unwrap().served, Served::Real);

    // MOCKED fault: provider-b answers with an injected body, anvil untouched.
    router.set_fault(|provider, _req| {
        if provider == PROVIDER_B {
            FaultAction::ReplaceBody(r#"{"jsonrpc":"2.0","id":1,"result":"0xdead"}"#.to_string())
        } else {
            FaultAction::Pass
        }
    });
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_B),
        &chain_id_req,
        2000,
    );
    assert_eq!(
        res,
        RequestResult::Ok(r#"{"jsonrpc":"2.0","id":1,"result":"0xdead"}"#.to_string()),
        "MOCKED provider-b body must reach the caller"
    );
    assert_eq!(router.log().last().unwrap().served, Served::MockedBody);
    // ...while provider-c still passes through to the real chain.
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_C),
        &chain_id_req,
        2000,
    );
    assert!(
        matches!(&res, RequestResult::Ok(b) if b.contains("0x7a69")),
        "{res:?}"
    );

    // HTTP status fault -> InvalidHttpJsonRpcResponse with that status.
    router.set_fault(|_p, _r| FaultAction::HttpStatus(503));
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_A),
        &chain_id_req,
        2000,
    );
    match res {
        RequestResult::Err(RpcError::HttpOutcallError(
            HttpOutcallError::InvalidHttpJsonRpcResponse(r),
        )) => {
            assert_eq!(r.status, 503);
            println!("HttpStatus(503) fault surfaces as InvalidHttpJsonRpcResponse {{ status: {}, body: {:?} }}", r.status, r.body);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Reject fault -> IcError.
    router.set_fault(|_p, _r| FaultAction::Reject);
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_A),
        &chain_id_req,
        2000,
    );
    match res {
        RequestResult::Err(RpcError::HttpOutcallError(HttpOutcallError::IcError(e))) => {
            println!(
                "Reject fault surfaces as IcError {{ code: {:?}, message: {:?} }}",
                e.code, e.message
            );
            assert!(matches!(
                e.code,
                RejectionCode::SysTransient
                    | RejectionCode::SysFatal
                    | RejectionCode::CanisterReject
                    | RejectionCode::Unknown
            ));
        }
        other => panic!("unexpected {other:?}"),
    }

    // DelayOnce: the outcall stays parked for a round, then is answered for real.
    router.clear_log();
    let first_seen = std::cell::Cell::new(true);
    router.set_fault(move |_p, _r| {
        if first_seen.replace(false) {
            FaultAction::DelayOnce
        } else {
            FaultAction::Pass
        }
    });
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_A),
        &chain_id_req,
        2000,
    );
    assert!(
        matches!(&res, RequestResult::Ok(b) if b.contains("0x7a69")),
        "{res:?}"
    );
    let served: Vec<_> = router.log().iter().map(|r| r.served.clone()).collect();
    assert_eq!(
        served.first(),
        Some(&Served::Delayed),
        "first sighting is withheld: {served:?}"
    );
    assert_eq!(
        served.last(),
        Some(&Served::Real),
        "then answered by anvil: {served:?}"
    );
    router.clear_fault();

    // ForwardThenReject: the request REALLY executes on anvil but the answer is lost.
    let before = anvil.block_number("latest");
    router.set_fault(|_p, _r| FaultAction::ForwardThenReject);
    let mine_req = relay::jsonrpc("anvil_mine", json!(["0x1"]));
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_A),
        &mine_req,
        2000,
    );
    assert!(
        matches!(
            res,
            RequestResult::Err(RpcError::HttpOutcallError(HttpOutcallError::IcError(_)))
        ),
        "{res:?}"
    );
    assert_eq!(
        anvil.block_number("latest"),
        before + 1,
        "forwarded request executed on anvil despite the lost answer"
    );
    router.clear_fault();

    // Unknown provider URL -> 404 from the router.
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom("https://nope.test/"),
        &chain_id_req,
        2000,
    );
    assert!(
        matches!(res, RequestResult::Err(RpcError::HttpOutcallError(HttpOutcallError::InvalidHttpJsonRpcResponse(ref r))) if r.status == 404)
    );

    // Non-Custom services are not served; requestCost is zero for Custom.
    let res = relay::request(
        &pic,
        relay_id,
        &mut router,
        RpcService::Provider(1),
        &chain_id_req,
        2000,
    );
    assert_eq!(
        res,
        RequestResult::Err(RpcError::ProviderError(ProviderError::ProviderNotFound))
    );
    assert_eq!(
        relay::request_cost(
            &pic,
            relay_id,
            RpcService::custom(PROVIDER_A),
            &chain_id_req,
            2000
        ),
        RequestCostResult::Ok(Nat::from(0u64))
    );
}

#[test]
fn outcall_pump_handles_sequential_outcalls_and_pump_until_idle() {
    let anvil = Anvil::start();
    let pic = build_pic();
    let relay_id = deploy_relay(&pic);
    let mut router = ProviderRouter::new(anvil.endpoint());

    // ONE update call making three outcalls in sequence, all REAL anvil execution.
    let retries_before = support::outcalls::still_running_retries();
    let results = relay::request_seq(
        &pic,
        relay_id,
        &mut router,
        RpcService::custom(PROVIDER_B),
        vec![
            relay::jsonrpc("eth_chainId", json!([])),
            relay::jsonrpc("anvil_mine", json!(["0x3"])),
            relay::jsonrpc("eth_blockNumber", json!([])),
        ],
        2000,
    );
    assert_eq!(results.len(), 3);
    assert!(
        matches!(&results[0], RequestResult::Ok(b) if b.contains("0x7a69")),
        "{results:?}"
    );
    assert!(matches!(&results[1], RequestResult::Ok(_)), "{results:?}");
    assert!(
        matches!(&results[2], RequestResult::Ok(b) if b.contains("0x3")),
        "{results:?}"
    );
    assert_eq!(anvil.block_number("latest"), 3);
    assert_eq!(router.count(PROVIDER_B, "eth_chainId"), 1);
    assert_eq!(router.count(PROVIDER_B, "anvil_mine"), 1);
    assert_eq!(router.count(PROVIDER_B, "eth_blockNumber"), 1);

    // The default quiet-tick threshold never needs the recovery path...
    assert_eq!(
        support::outcalls::still_running_retries(),
        retries_before,
        "the default threshold must not hit the await_call still-running panic"
    );
    // ...but if await_call IS attempted too early (threshold 0) the pump catches
    // pocket-ic's "Failed to answer to ingress" panic, keeps pumping, and the
    // call still completes with correct results.
    let arg = candid::encode_args((
        RpcService::custom(PROVIDER_A),
        vec![
            relay::jsonrpc("eth_chainId", json!([])),
            relay::jsonrpc("eth_blockNumber", json!([])),
        ],
        2000u64,
    ))
    .unwrap();
    let id = support::outcalls::submit(&pic, relay_id, Principal::anonymous(), "request_seq", arg)
        .unwrap();
    let bytes = support::outcalls::await_with_outcalls_opts(&pic, id, &mut router, 0)
        .expect("pump recovers");
    let early: Vec<RequestResult> = candid::decode_one(&bytes).unwrap();
    assert_eq!(early.len(), 2);
    assert!(
        matches!(&early[0], RequestResult::Ok(b) if b.contains("0x7a69")),
        "{early:?}"
    );
    let recoveries = support::outcalls::still_running_retries() - retries_before;
    println!("early-await recoveries (caught pocket-ic panics): {recoveries}");
    assert!(
        recoveries >= 1,
        "threshold 0 must have exercised the recovery path"
    );

    // pump_until_idle drives a submitted-but-unawaited call to completion; the
    // subsequent await_call returns immediately.
    let arg = candid::encode_args((
        RpcService::custom(PROVIDER_C),
        relay::jsonrpc("eth_chainId", json!([])),
        2000u64,
    ))
    .unwrap();
    let id =
        support::outcalls::submit(&pic, relay_id, Principal::anonymous(), "request", arg).unwrap();
    let rounds = support::outcalls::pump_until_idle(&pic, &mut router, 200);
    println!("pump_until_idle finished after {rounds} rounds");
    assert!(rounds < 200);
    match pic.await_call(id).expect("await_call") {
        pocket_ic::WasmResult::Reply(b) => {
            let r: RequestResult = candid::decode_one(&b).unwrap();
            assert!(
                matches!(r, RequestResult::Ok(ref body) if body.contains("0x7a69")),
                "{r:?}"
            );
        }
        pocket_ic::WasmResult::Reject(m) => panic!("rejected: {m}"),
    }
    assert_eq!(router.count(PROVIDER_C, "eth_chainId"), 1);

    // On an idle instance it stops after exactly the idle window.
    assert_eq!(
        support::outcalls::pump_until_idle(&pic, &mut router, 200),
        8
    );
}

// ---------------------------------------------------------------------------
// (d) threshold ECDSA availability in this PocketIC build
// ---------------------------------------------------------------------------

#[test]
fn tecdsa_key_names_available() {
    let pic = build_pic();
    let relay_id = deploy_relay(&pic);
    let mut outcome = Vec::new();
    for key in ["test_key_1", "key_1", "dfx_test_key"] {
        let r = relay::probe_ecdsa(&pic, relay_id, key, vec![b"probe".to_vec()]);
        match &r {
            Ok(pk) => println!(
                "tECDSA key {key:<14} OK  ({} bytes, 0x{})",
                pk.len(),
                hex::encode(pk)
            ),
            Err(e) => println!("tECDSA key {key:<14} ERR {e}"),
        }
        outcome.push((key, r));
    }
    let test_key_1 = &outcome[0].1;
    let pk = test_key_1.as_ref().expect("test_key_1 must be available");
    assert_eq!(pk.len(), 33, "SEC1 compressed secp256k1 key");
    assert!(pk[0] == 0x02 || pk[0] == 0x03, "compressed point prefix");
    // Derivation is deterministic and path-dependent.
    let again = relay::probe_ecdsa(&pic, relay_id, "test_key_1", vec![b"probe".to_vec()]).unwrap();
    let other = relay::probe_ecdsa(&pic, relay_id, "test_key_1", vec![b"other".to_vec()]).unwrap();
    assert_eq!(&again, pk);
    assert_ne!(&other, pk);
}

// ---------------------------------------------------------------------------
// (e) ICRC ledger: transfer, approve, icrc3_get_blocks, archiving
// ---------------------------------------------------------------------------

#[test]
fn icrc_ledger_transfer_approve_blocks_and_archive() {
    let pic = build_pic();
    let minter = test_principal(1);
    let alice = test_principal(2);
    let bob = test_principal(3);
    let spender = test_principal(4);
    let ledger_id = deploy_icrc_ledger(
        &pic,
        &LedgerConfig::new(
            account(minter),
            minter,
            vec![(account(alice), 1_000_000_000_000)],
        ),
    );

    assert_eq!(
        ledger::icrc1_fee(&pic, ledger_id),
        Nat::from(TRANSFER_FEE_E8S)
    );
    assert_eq!(
        ledger::icrc1_total_supply(&pic, ledger_id),
        Nat::from(1_000_000_000_000u64)
    );
    assert_eq!(
        ledger::icrc1_balance_of(&pic, ledger_id, &account(alice)),
        Nat::from(1_000_000_000_000u64)
    );

    // Transfer.
    let idx = ledger::transfer(&pic, ledger_id, alice, account(bob), 5_000_000).expect("transfer");
    println!("first transfer block index = {idx}");
    assert_eq!(
        ledger::icrc1_balance_of(&pic, ledger_id, &account(bob)),
        Nat::from(5_000_000u64)
    );
    assert_eq!(
        ledger::icrc1_balance_of(&pic, ledger_id, &account(alice)),
        Nat::from(1_000_000_000_000u64 - 5_000_000 - TRANSFER_FEE_E8S)
    );

    // ICRC-2 approve (feature flag on).
    let approve_idx = ledger::icrc2_approve(
        &pic,
        ledger_id,
        alice,
        ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: spender,
                subaccount: None,
            },
            amount: Nat::from(123_456_789u64),
            expected_allowance: None,
            expires_at: None,
            fee: Some(Nat::from(TRANSFER_FEE_E8S)),
            memo: None,
            created_at_time: None,
        },
    )
    .expect("icrc2_approve");
    println!("approve block index = {approve_idx}");

    // Blocks before archiving: genesis mint (0), transfer (1), approve (2); nothing archived.
    let early = ledger::icrc3_get_blocks(&pic, ledger_id, 0, 10);
    assert_eq!(early.log_length, Nat::from(3u64));
    assert_eq!(early.blocks.len(), 3);
    assert!(
        early.archived_blocks.is_empty(),
        "no archive below the threshold"
    );
    println!(
        "pre-archive icrc3_get_blocks(0,10): log_length={}, blocks={}, archived_blocks={}",
        early.log_length,
        early.blocks.len(),
        early.archived_blocks.len()
    );

    // Push past the archive threshold (50 blocks held) with small transfers.
    for i in 0..70u128 {
        ledger::transfer(&pic, ledger_id, alice, account(bob), 1_000 + i).expect("bulk transfer");
    }
    // Let the ledger spawn the archive canister and move blocks over.
    for _ in 0..30 {
        pic.tick();
    }
    pic.advance_time(std::time::Duration::from_secs(5));
    for _ in 0..30 {
        pic.tick();
    }

    let raw = ledger::icrc3_get_blocks_raw(&pic, ledger_id, 0, 10);
    println!(
        "post-archive icrc3_get_blocks(0,10) raw candid:\n{}",
        ledger::describe_candid(&raw)
    );
    let res: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
        candid::decode_one(&raw).expect("decode");
    println!(
        "post-archive: log_length={}, inline blocks={}, archived_blocks entries={}",
        res.log_length,
        res.blocks.len(),
        res.archived_blocks.len()
    );
    assert_eq!(
        res.log_length,
        Nat::from(73u64),
        "3 early blocks + 70 bulk transfers"
    );
    assert!(
        !res.archived_blocks.is_empty(),
        "blocks 0.. must now live in an archive"
    );
    let ab = &res.archived_blocks[0];
    println!(
        "archived_blocks[0]: args={:?} callback=(canister {}, method {:?})",
        ab.args, ab.callback.canister_id, ab.callback.method
    );
    assert_ne!(
        ab.callback.canister_id, ledger_id,
        "callback points at an archive canister"
    );
    assert_eq!(ab.callback.method, "icrc3_get_blocks");

    println!(
        "icrc3_get_archives raw candid:\n{}",
        ledger::describe_candid(&ledger::icrc3_get_archives_raw(&pic, ledger_id))
    );
    let span = ledger::icrc3_get_blocks_raw(&pic, ledger_id, 0, 80);
    println!("icrc3_get_blocks(0,80) spanning archive + inline blocks, raw candid (block bodies elided below):");
    let span_res: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
        candid::decode_one(&span).expect("decode span");
    println!(
        "  log_length={}, inline blocks ids={:?}, archived_blocks args={:?}",
        span_res.log_length,
        span_res
            .blocks
            .iter()
            .map(|b| b.id.to_string())
            .collect::<Vec<_>>(),
        span_res
            .archived_blocks
            .iter()
            .map(|a| a.args.clone())
            .collect::<Vec<_>>()
    );

    // The callback really serves the archived blocks.
    let archived = pic
        .query_call(
            ab.callback.canister_id,
            Principal::anonymous(),
            &ab.callback.method,
            candid::encode_one(&ab.args).unwrap(),
        )
        .expect("archive query");
    let support_bytes = match archived {
        pocket_ic::WasmResult::Reply(b) => b,
        pocket_ic::WasmResult::Reject(m) => panic!("archive rejected: {m}"),
    };
    let archived_res: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
        candid::decode_one(&support_bytes).expect("decode archive reply");
    assert!(!archived_res.blocks.is_empty(), "archive returned blocks");
    assert_eq!(archived_res.blocks[0].id, Nat::from(0u64));
    println!(
        "archive callback returned {} blocks starting at id {}",
        archived_res.blocks.len(),
        archived_res.blocks[0].id
    );
}

// ---------------------------------------------------------------------------
// (f) real transactions on anvil: mint, bridgeOut, logs
// ---------------------------------------------------------------------------

#[test]
fn anvil_mint_and_bridge_out_emit_expected_events() {
    let w = world();
    let (anvil, token) = (&w.anvil, &w.token);

    // MINTER_ROLE mint to the holder.
    let mint_id = bytes32_from_u64(0xA1);
    let tx = token.mint_from_unlocked(anvil, &w.minter, &w.holder, 1_000_000, mint_id);
    let rc = anvil.receipt(&tx).expect("mint receipt");
    assert_eq!(rc["status"], "0x1", "{rc}");
    let mint_logs = logs_with_topic(&rc, &token.address, TOPIC_BRIDGE_MINT);
    assert_eq!(mint_logs.len(), 1, "exactly one BridgeMint log");
    let (id, to, amount) = decode_bridge_mint(mint_logs[0]);
    assert_eq!(
        (id, to.as_str(), amount),
        (mint_id, w.holder.as_str(), 1_000_000)
    );
    assert_eq!(token.balance_of(anvil, &w.holder), 1_000_000);
    assert_eq!(token.total_supply(anvil), 1_000_000);
    assert!(token.mint_consumed(anvil, mint_id));

    // A non-minter cannot mint: anvil mines the call as a FAILED receipt (status 0x0).
    let bad = anvil.send_tx_from_unlocked(
        &w.holder,
        Some(&token.address),
        &support::contract::encode_mint(&w.holder, 1, bytes32_from_u64(0xA2)),
        0,
    );
    assert!(!anvil.tx_succeeded(&bad), "non-minter mint must revert");
    assert_eq!(token.total_supply(anvil), 1_000_000);
    assert!(!token.mint_consumed(anvil, bytes32_from_u64(0xA2)));

    // bridgeOut burns and emits BridgeOut with sequence 1.
    let principal = test_principal(9);
    let subaccount = [7u8; 32];
    let nonce = bytes32_from_u64(1);
    let tx = token.bridge_out(
        anvil,
        &w.holder,
        400_000,
        principal.as_slice(),
        subaccount,
        nonce,
    );
    let rc = anvil.receipt(&tx).expect("bridgeOut receipt");
    assert_eq!(rc["status"], "0x1", "{rc}");
    let out_logs = logs_with_topic(&rc, &token.address, TOPIC_BRIDGE_OUT);
    assert_eq!(out_logs.len(), 1, "exactly one BridgeOut log");
    let ev = decode_bridge_out(out_logs[0]);
    assert_eq!(ev.sequence, 1);
    assert_eq!(ev.amount, 400_000);
    assert_eq!(ev.from, w.holder);
    assert_eq!(ev.icp_principal, principal.as_slice());
    assert_eq!(ev.icp_subaccount, subaccount);
    assert_eq!(
        ev.transfer_id,
        token.compute_transfer_id(anvil, &w.holder, nonce)
    );
    assert_eq!(token.balance_of(anvil, &w.holder), 600_000);
    assert_eq!(token.total_supply(anvil), 600_000, "bridgeOut burns");
    assert_eq!(token.bridge_out_count(anvil), 1);

    // Replaying the same (sender, nonce) reverts (BridgeOutAlreadyUsed): failed receipt, state untouched.
    let replay = token.bridge_out(anvil, &w.holder, 1, principal.as_slice(), subaccount, nonce);
    assert!(
        !anvil.tx_succeeded(&replay),
        "replayed (sender, nonce) must revert"
    );
    assert_eq!(token.bridge_out_count(anvil), 1);
    assert_eq!(token.balance_of(anvil, &w.holder), 600_000);
    // A second distinct bridgeOut gets sequence 2.
    let tx2 = token.bridge_out(
        anvil,
        &w.holder,
        100_000,
        principal.as_slice(),
        [0u8; 32],
        bytes32_from_u64(2),
    );
    let ev2 = decode_bridge_out(
        logs_with_topic(
            &anvil.receipt(&tx2).unwrap(),
            &token.address,
            TOPIC_BRIDGE_OUT,
        )[0],
    );
    assert_eq!(ev2.sequence, 2);
}

// ---------------------------------------------------------------------------
// (g) snapshot / revert removes a mined bridgeOut (reorg simulation)
// ---------------------------------------------------------------------------

#[test]
fn anvil_snapshot_revert_removes_bridge_out_block() {
    let w = world();
    let (anvil, token) = (&w.anvil, &w.token);
    token.mint_from_unlocked(anvil, &w.minter, &w.holder, 1_000_000, bytes32_from_u64(1));

    let snap = anvil.snapshot();
    let height_before = anvil.block_number("latest");

    let tx = token.bridge_out(
        anvil,
        &w.holder,
        250_000,
        test_principal(5).as_slice(),
        [0u8; 32],
        bytes32_from_u64(77),
    );
    anvil.mine(2);
    let rc = anvil.receipt(&tx).expect("receipt exists before the reorg");
    assert_eq!(rc["status"], "0x1");
    assert_eq!(token.bridge_out_count(anvil), 1);
    assert_eq!(token.balance_of(anvil, &w.holder), 750_000);
    assert_eq!(anvil.block_number("latest"), height_before + 3);

    assert!(anvil.revert(&snap), "evm_revert reports success");

    assert!(
        anvil.receipt(&tx).is_none(),
        "receipt must disappear after the revert"
    );
    assert_eq!(
        anvil.block_number("latest"),
        height_before,
        "mined blocks are gone"
    );
    assert_eq!(
        token.bridge_out_count(anvil),
        0,
        "contract state rolled back"
    );
    assert_eq!(
        token.balance_of(anvil, &w.holder),
        1_000_000,
        "burn rolled back"
    );
    let tx_lookup = anvil.rpc("eth_getTransactionByHash", json!([tx]));
    assert!(
        tx_lookup.is_null(),
        "the transaction itself is gone: {tx_lookup}"
    );

    // The chain keeps working and the same bridgeOut can be re-mined (possibly in a new block).
    let tx_again = token.bridge_out(
        anvil,
        &w.holder,
        250_000,
        test_principal(5).as_slice(),
        [0u8; 32],
        bytes32_from_u64(77),
    );
    assert!(anvil.receipt(&tx_again).is_some());
    assert_eq!(token.bridge_out_count(anvil), 1);
}
