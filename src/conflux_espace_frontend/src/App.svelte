<script lang="ts">
  import { ACTION, MIN_CR, MIN_DEBT_E8S, ICUSD_CONTRACT, BACKEND_CANISTER_ID } from "./config";
  import { backend, errText, type ChainVault } from "./backend";
  import { signIntent, toCandidIntent, type VaultIntentInput } from "./eip712";
  import {
    connectMetaMask, connectDevKey, hasMetaMask, sendDeposit, burnIcusd, icusdBalance, cfxBalance,
    fmtCfx, fmtIcusd, parseEther, toE8s, txUrl, type Wallet,
  } from "./evm";
  import VaultCard from "./VaultCard.svelte";

  let wallet = $state<Wallet | null>(null);
  let vaults = $state<ChainVault[]>([]);
  let cfx = $state(0n);
  let icusd = $state(0n);
  let nonce = $state(0n); // next per-owner nonce; auto-synced on a bad-nonce reject

  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);
  let ok = $state<string | null>(null);

  // open form
  let debtInput = $state("0.2");
  let cfxPrice = $state("0.15"); // UX hint only — the real CR check is server-side
  let showDevKey = $state(false);
  let devKey = $state("0x" + "00".repeat(31) + "01"); // scalar=1 demo key

  const owned = $derived(
    wallet
      ? vaults.filter((v) => v.owner_evm.length && v.owner_evm[0]!.toLowerCase() === wallet!.address.toLowerCase())
      : []
  );
  const pendingExists = $derived(owned.some((v) => {
    const s = Object.keys(v.status)[0];
    return s === "AwaitingDeposit" || s === "MintPending" || s === "Closing";
  }));

  const debtE8s = $derived(toE8s(debtInput));
  const requiredCfxWei = $derived.by(() => {
    const d = parseFloat(debtInput) || 0;
    const p = parseFloat(cfxPrice) || 0;
    if (d <= 0 || p <= 0) return 0n;
    const cfxNeeded = (d * MIN_CR / p) * 1.02; // +2% buffer over the floor
    return parseEther(cfxNeeded.toFixed(6));
  });

  function reset() { err = null; ok = null; }

  async function refresh() {
    if (!wallet) return;
    try {
      const be = await backend();
      vaults = await be.list_chain_vaults(71);
      cfx = await cfxBalance(wallet.address);
      icusd = await icusdBalance(wallet.address);
    } catch (e: any) { err = `Refresh failed: ${e?.message ?? e}`; }
  }

  async function connectMM() {
    reset(); busy = "Connecting…";
    try { wallet = await connectMetaMask(); await refresh(); }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }
  async function connectDev() {
    reset(); busy = "Loading dev key…";
    try { wallet = connectDevKey(devKey.trim()); await refresh(); }
    catch (e: any) { err = `Bad key: ${e?.message ?? e}`; }
    finally { busy = null; }
  }
  function disconnect() { wallet = null; vaults = []; reset(); }

  /** Sign + submit an intent, auto-syncing the per-owner nonce on a bad-nonce reject. */
  async function submit(
    action: number, vaultId: bigint, collateralWei: bigint, debt: bigint,
    call: (be: Awaited<ReturnType<typeof backend>>, i: any, sig: Uint8Array) => Promise<{ Ok?: unknown; Err?: any }>
  ): Promise<{ Ok?: unknown; Err?: any }> {
    const be = await backend();
    const w = wallet!;
    for (let attempt = 0; attempt < 2; attempt++) {
      const input: VaultIntentInput = {
        action, owner: w.address, vaultId, collateralWei, debtE8s: debt,
        nonce, deadlineSecs: BigInt(Math.floor(Date.now() / 1000) + 3600),
      };
      const sig = await signIntent(w.client, w.account as any, input);
      const res = await call(be, toCandidIntent(input), sig);
      if ("Ok" in res) { nonce += 1n; return res; }
      const msg = errText(res.Err);
      const m = msg.match(/expected (\d+)/);
      if (m) {
        nonce = BigInt(m[1]); // keep the local nonce fresh for the next action
        if (attempt === 0) continue; // retry once with the corrected nonce
      }
      return res;
    }
    return { Err: { GenericError: "nonce sync failed" } };
  }

  async function doOpen() {
    reset();
    if (debtE8s < MIN_DEBT_E8S) { err = `Minimum debt is ${fmtIcusd(MIN_DEBT_E8S)} icUSD`; return; }
    if (requiredCfxWei <= 0n) { err = "Enter a debt and CFX price"; return; }
    busy = "Sign the Open intent in your wallet…";
    try {
      const res = await submit(ACTION.Open, 0n, requiredCfxWei, debtE8s,
        (be, i, sig) => be.open_chain_vault_evm(i, sig));
      if ("Ok" in res) { ok = `Vault #${res.Ok} opened — send the CFX deposit below.`; await refresh(); }
      else err = errText(res.Err);
    } catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }

  // actions invoked by VaultCard
  async function onAction(kind: string, vault: ChainVault, amountE8s?: bigint) {
    reset();
    const w = wallet!;
    try {
      if (kind === "deposit") {
        busy = "Confirm the CFX deposit in your wallet…";
        const hash = await sendDeposit(w, vault.custody_address as `0x${string}`, vault.collateral_amount_e18);
        ok = `Deposit sent (${hash.slice(0, 12)}…) — watch the status flip to Open.`;
        console.log("deposit tx", txUrl(hash));
      } else if (kind === "repay") {
        busy = "Confirm the on-chain burn in your wallet…";
        const amt = amountE8s ?? vault.debt_e8s;
        await burnIcusd(w, amt, vault.vault_id);
        ok = `Repaid ${fmtIcusd(amt)} icUSD on-chain — the observer will decrement the vault debt.`;
      } else if (kind === "borrow") {
        busy = "Sign the Borrow intent…";
        const res = await submit(ACTION.Borrow, vault.vault_id, 0n, amountE8s ?? 0n,
          (be, i, sig) => be.borrow_chain_vault_evm(i, sig));
        if ("Ok" in res) ok = "Borrow signed — the mint will land shortly."; else err = errText(res.Err);
      } else if (kind === "withdraw") {
        busy = "Sign the Withdraw intent…";
        const res = await submit(ACTION.WithdrawCollateral, vault.vault_id, amountE8s ?? 0n, 0n,
          (be, i, sig) => be.withdraw_chain_collateral_evm(i, sig));
        if ("Ok" in res) ok = "Withdraw signed."; else err = errText(res.Err);
      } else if (kind === "close") {
        busy = "Sign the Close intent…";
        const res = await submit(ACTION.Close, vault.vault_id, 0n, 0n,
          (be, i, sig) => be.close_chain_vault_evm(i, sig));
        if ("Ok" in res) ok = "Close signed — collateral returns to your wallet."; else err = errText(res.Err);
      }
      await refresh();
    } catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }

  // poll while anything is in flight
  $effect(() => {
    if (!wallet || !pendingExists) return;
    const id = setInterval(refresh, 8000);
    return () => clearInterval(id);
  });
</script>

<div class="wrap">
  <header class="top">
    <div class="brand">
      <div class="logo">R</div>
      <div>
        <h1>icUSD on Conflux eSpace</h1>
        <div class="sub">Self-serve CDP · sign with your EVM wallet</div>
      </div>
    </div>
    <span class="badge testnet">eSpace testnet · chain 71 · staging</span>
  </header>

  {#if !wallet}
    <div class="card">
      <h2>Connect</h2>
      <p class="hint">Open a CFX-collateralized icUSD vault by signing an EIP-712 intent — no IC login.
        Your wallet is the only identity; the canister verifies the signature.</p>
      <div class="row">
        <button class="primary" onclick={connectMM} disabled={!!busy || !hasMetaMask()}>
          {hasMetaMask() ? "Connect MetaMask" : "MetaMask not detected"}
        </button>
        <button class="ghost sm" onclick={() => (showDevKey = !showDevKey)}>{showDevKey ? "Hide" : "Use a dev key"}</button>
      </div>
      {#if showDevKey}
        <div class="divider"></div>
        <label for="devkey">Private key (testnet only — for the no-wallet demo path)</label>
        <div class="field"><div class="row">
          <input id="devkey" class="mono" bind:value={devKey} spellcheck="false" />
          <button onclick={connectDev} disabled={!!busy}>Load</button>
        </div></div>
        <p class="hint">Pre-filled with the scalar=1 demo key (<span class="mono">0x7e5f…95bdf</span>) — the same one the staging round-trip used.</p>
      {/if}
    </div>
  {:else}
    <div class="card">
      <div class="row spread">
        <h2>Wallet</h2>
        <button class="ghost sm" onclick={disconnect}>Disconnect</button>
      </div>
      <div class="kv"><span class="k">Address</span><span class="v mono">{wallet.address}</span></div>
      <div class="kv"><span class="k">CFX</span><span class="v">{fmtCfx(cfx)}</span></div>
      <div class="kv"><span class="k">icUSD</span><span class="v">{fmtIcusd(icusd)}</span></div>
      <div class="kv"><span class="k">Signer</span><span class="v">{wallet.kind === "metamask" ? "MetaMask" : "dev key"}</span></div>
    </div>

    <div class="card">
      <h2>Open a vault</h2>
      <p class="hint">Enter the icUSD you want to mint. Required CFX is the {Math.round(MIN_CR * 100)}% min-CR
        floor (+2% buffer) at your price — the real CR check runs on the canister.</p>
      <div class="row">
        <div class="field" style="flex:1">
          <label for="debt">icUSD debt</label>
          <input id="debt" type="number" min="0.1" step="0.1" bind:value={debtInput} />
        </div>
        <div class="field" style="flex:1">
          <label for="cfxprice">CFX price (USD, hint)</label>
          <input id="cfxprice" type="number" min="0" step="0.01" bind:value={cfxPrice} />
        </div>
      </div>
      <div class="kv"><span class="k">Required CFX (≈)</span><span class="v">{fmtCfx(requiredCfxWei)}</span></div>
      <div class="row" style="margin-top:14px">
        <button class="primary" onclick={doOpen} disabled={!!busy}>Sign & open</button>
      </div>
    </div>

    {#each owned as v (v.vault_id)}
      <VaultCard vault={v} busy={busy} {onAction} />
    {/each}
    {#if owned.length === 0}
      <div class="card"><p class="hint" style="margin:0">No vaults yet for this address. Open one above.</p></div>
    {/if}
  {/if}

  {#if busy}<div class="notice info"><span class="spin"></span>{busy}</div>{/if}
  {#if err}<div class="notice err">{err}</div>{/if}
  {#if ok}<div class="notice ok">{ok}</div>{/if}

  <div class="foot">
    Backend <span class="mono">{BACKEND_CANISTER_ID}</span> (staging) · IcUSD <span class="mono">{ICUSD_CONTRACT.slice(0, 10)}…</span><br />
    Testnet only. The chains rail is experimental — not on production.
  </div>
</div>
