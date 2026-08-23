<script lang="ts">
  import { CANARY_DEBT_E8S } from "./config";
  import type { CanaryPhase } from "./canaryState";
  import { statusName, type ChainVault } from "./backend";
  import { addressUrl, fmtCfx, fmtIcusd, toE8s, toWei } from "./evm";

  let { vault, busy, productionCanary, productionPublic, riskWritesEnabled, riskWriteDisabledReason, recoveryWritesEnabled, recoveryWriteDisabledReason, isCanaryVault, canaryPhase, onAction }: {
    vault: ChainVault;
    busy: string | null;
    productionCanary: boolean;
    productionPublic: boolean;
    riskWritesEnabled: boolean;
    riskWriteDisabledReason: string | null;
    recoveryWritesEnabled: boolean;
    recoveryWriteDisabledReason: string | null;
    isCanaryVault: boolean;
    canaryPhase: CanaryPhase | null;
    onAction: (kind: string, vault: ChainVault, amount?: bigint) => void;
  } = $props();

  const status = $derived(statusName(vault.status));
  const custody = $derived(vault.custody_address);
  const mainnet = $derived(productionCanary || productionPublic);

  let borrowAmt = $state("0.1");
  let repayAmt = $state("");
  let withdrawAmt = $state("");

  function copy() { navigator.clipboard?.writeText(custody); }
</script>

<div class="card">
  <div class="row spread">
    <h2>Vault #{vault.vault_id}</h2>
    <span class="pill {status}">{status}</span>
  </div>

  <div class="kv"><span class="k">Debt</span><span class="v">{fmtIcusd(vault.debt_e8s)} icUSD</span></div>
  <div class="kv"><span class="k">Collateral</span><span class="v">{fmtCfx(vault.collateral_amount_e18)} CFX</span></div>
  {#if vault.pending_mint_e8s > 0n}
    <div class="kv"><span class="k">Pending mint</span><span class="v">{fmtIcusd(vault.pending_mint_e8s)} icUSD</span></div>
  {/if}
  <div class="kv custody-row"><span class="k">Custody</span>
    <span class="v custody-actions">
      <a class="mono" href={addressUrl(custody)} target="_blank" rel="noreferrer">{mainnet ? custody : `${custody.slice(0, 14)}…${custody.slice(-6)}`} ↗</a>
      <button class="ghost sm" onclick={copy} title="Copy custody address">Copy</button>
    </span>
  </div>

  {#if productionCanary && !isCanaryVault}
    <div class="notice err" style="margin-top:14px">Read-only: this vault is not bound to this browser's persisted canary lifecycle.</div>
  {:else if status === "AwaitingDeposit"}
    <div class="notice info" style="margin-top:14px">
      Verify the custody address above, then send <b>{fmtCfx(vault.collateral_amount_e18)} CFX</b>. The mint begins only after the deposit is observed.
    </div>
    {#if productionCanary && (canaryPhase === "deposit-authorizing" || canaryPhase === "deposit-submitted" || canaryPhase === "deposit-observed" || canaryPhase === "deposit-replaced")}
      <div class="notice info"><span class="spin"></span>Deposit authorization/submission is locked — waiting for wallet receipt or backend observation. Do not repeat it.</div>
    {:else}
      {#if productionCanary && canaryPhase === "deposit-failed"}
        <div class="notice err">The prior deposit reverted or was cancelled. Verify its explorer entry before retrying.</div>
      {/if}
      <div class="row" style="margin-top:12px">
        <button class="primary" disabled={!!busy || !riskWritesEnabled} onclick={() => onAction("deposit", vault)}>
          {canaryPhase === "deposit-failed" ? "Retry" : "Confirm"} {fmtCfx(vault.collateral_amount_e18)} CFX deposit
        </button>
        {#if !mainnet}<span class="muted" style="font-size:12px">(or send manually from any wallet)</span>{/if}
      </div>
      {#if productionPublic && !riskWritesEnabled && riskWriteDisabledReason}
        <div class="notice err">Deposit paused: {riskWriteDisabledReason}</div>
      {/if}
    {/if}
  {:else if status === "MintPending"}
    <div class="notice info" style="margin-top:14px"><span class="spin"></span>Deposit detected — waiting for finality and mint.</div>
  {:else if status === "Open"}
    <div class="divider"></div>
    {#if productionCanary}
      {#if vault.debt_e8s > 0n}
        {#if canaryPhase === "burn-authorizing" || canaryPhase === "burn-submitted" || canaryPhase === "burn-replaced"}
          <div class="notice info"><span class="spin"></span>Exact burn authorization/submission is locked — waiting for wallet receipt or backend observation. Do not repeat it.</div>
        {:else if vault.debt_e8s !== CANARY_DEBT_E8S || vault.pending_mint_e8s !== 0n || vault.pending_interest_mint_e8s !== 0n}
          <div class="notice err">Fail-closed: burn requires exactly 0.10 icUSD current debt and no pending mint or interest.</div>
        {:else if canaryPhase === "mint-observed" || canaryPhase === "burn-failed"}
          <div class="notice info">Mint observed. Confirm the one exact <b>0.10 icUSD</b> burn below. No other amount is available in this build.</div>
          {#if canaryPhase === "burn-failed"}<div class="notice err">The prior burn reverted or was cancelled. Verify its explorer entry before retrying.</div>{/if}
          <div class="row" style="margin-top:12px">
            <button disabled={!!busy} onclick={() => onAction("repay", vault, CANARY_DEBT_E8S)}>{canaryPhase === "burn-failed" ? "Retry" : "Confirm"} exact 0.10 icUSD burn</button>
          </div>
        {:else}
          <div class="notice err">Fail-closed: persisted lifecycle state does not permit a burn.</div>
        {/if}
      {:else}
        {#if canaryPhase === "close-authorizing" || canaryPhase === "close-submitted"}
          <div class="notice info"><span class="spin"></span>Close authorization/submission is locked — waiting for backend observation. Do not repeat it.</div>
        {:else if canaryPhase === "burn-observed"}
          <div class="notice ok">Zero debt observed. The vault is ready to close.</div>
          <div class="row" style="margin-top:12px">
            <button class="danger" disabled={!!busy} onclick={() => onAction("close", vault)}>Sign & close vault</button>
          </div>
        {:else}
          <div class="notice err">Fail-closed: persisted lifecycle state does not permit close.</div>
        {/if}
      {/if}
    {:else}
      <div class="field">
        <label for="borrow-{vault.vault_id}">Borrow more icUSD</label>
        <div class="row">
          <input id="borrow-{vault.vault_id}" type="number" min="0" step="0.1" bind:value={borrowAmt} style="flex:1" />
          <button disabled={!!busy || !riskWritesEnabled || toE8s(borrowAmt) === 0n} onclick={() => onAction("borrow", vault, toE8s(borrowAmt))}>Borrow</button>
        </div>
      </div>
      <div class="field">
        <label for="repay-{vault.vault_id}">Repay icUSD (on-chain burn)</label>
        <div class="row">
          <input id="repay-{vault.vault_id}" type="number" min="0" step="0.1" placeholder={fmtIcusd(vault.debt_e8s)} bind:value={repayAmt} style="flex:1" />
          <button disabled={!!busy || !recoveryWritesEnabled || vault.debt_e8s === 0n || (repayAmt !== "" && toE8s(repayAmt) === 0n)}
            onclick={() => onAction("repay", vault, repayAmt ? toE8s(repayAmt) : vault.debt_e8s)}>Repay</button>
        </div>
      </div>
      <div class="field">
        <label for="withdraw-{vault.vault_id}">Withdraw CFX collateral</label>
        <div class="row">
          <input id="withdraw-{vault.vault_id}" type="number" min="0" step="0.1" bind:value={withdrawAmt} style="flex:1" />
          <button disabled={!!busy || !riskWritesEnabled || toWei(withdrawAmt) === 0n}
            onclick={() => onAction("withdraw", vault, toWei(withdrawAmt))}>Withdraw</button>
        </div>
      </div>
      <div class="row">
        <button class="danger" disabled={!!busy || !recoveryWritesEnabled || vault.debt_e8s !== 0n}
          onclick={() => onAction("close", vault)}>Close vault (repay first)</button>
      </div>
    {/if}
    {#if productionPublic && !riskWritesEnabled && riskWriteDisabledReason}
      <div class="notice err">Open, deposit, borrow, and withdraw are paused: {riskWriteDisabledReason}</div>
    {/if}
    {#if productionPublic && !recoveryWritesEnabled && recoveryWriteDisabledReason}
      <div class="notice err">Repay and debt-free close are unavailable: {recoveryWriteDisabledReason}</div>
    {/if}
  {:else if status === "Closing"}
    <div class="notice info" style="margin-top:14px"><span class="spin"></span>Close submitted — waiting for collateral return and Closed status.</div>
  {:else}
    <div class="notice info" style="margin-top:14px">Vault is {status.toLowerCase()}.</div>
  {/if}
</div>
