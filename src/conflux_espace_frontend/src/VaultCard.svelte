<script lang="ts">
  import { statusName, type ChainVault } from "./backend";
  import { fmtCfx, fmtIcusd, parseEther } from "./evm";

  let { vault, busy, onAction }: {
    vault: ChainVault;
    busy: string | null;
    onAction: (kind: string, vault: ChainVault, amount?: bigint) => void;
  } = $props();

  const status = $derived(statusName(vault.status));
  const custody = $derived(vault.custody_address);

  let borrowAmt = $state("0.1");
  let repayAmt = $state("");
  let withdrawAmt = $state("");

  const toE8s = (s: string) => BigInt(Math.round((parseFloat(s) || 0) * 1e8));

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
  <div class="kv"><span class="k">Custody</span>
    <span class="v mono copy" role="button" tabindex="0" onclick={copy} onkeydown={copy} title="copy">{custody.slice(0, 14)}…{custody.slice(-6)}</span>
  </div>

  {#if status === "AwaitingDeposit"}
    <div class="notice info" style="margin-top:14px">
      Send <b>{fmtCfx(vault.collateral_amount_e18)} CFX</b> to the custody address, then it mints automatically.
    </div>
    <div class="row" style="margin-top:12px">
      <button class="primary" disabled={!!busy} onclick={() => onAction("deposit", vault)}>
        Send {fmtCfx(vault.collateral_amount_e18)} CFX
      </button>
      <span class="muted" style="font-size:12px">(or send manually from any wallet)</span>
    </div>
  {:else if status === "MintPending"}
    <div class="notice info" style="margin-top:14px"><span class="spin"></span>Deposit detected — minting at finality (eSpace ~ a few min).</div>
  {:else if status === "Open"}
    <div class="divider"></div>
    <div class="field">
      <label for="borrow-{vault.vault_id}">Borrow more icUSD</label>
      <div class="row">
        <input id="borrow-{vault.vault_id}" type="number" min="0" step="0.1" bind:value={borrowAmt} style="flex:1" />
        <button disabled={!!busy} onclick={() => onAction("borrow", vault, toE8s(borrowAmt))}>Borrow</button>
      </div>
    </div>
    <div class="field">
      <label for="repay-{vault.vault_id}">Repay icUSD (on-chain burn)</label>
      <div class="row">
        <input id="repay-{vault.vault_id}" type="number" min="0" step="0.1" placeholder={fmtIcusd(vault.debt_e8s)} bind:value={repayAmt} style="flex:1" />
        <button disabled={!!busy || vault.debt_e8s === 0n}
          onclick={() => onAction("repay", vault, repayAmt ? toE8s(repayAmt) : vault.debt_e8s)}>Repay</button>
      </div>
    </div>
    <div class="field">
      <label for="withdraw-{vault.vault_id}">Withdraw CFX collateral</label>
      <div class="row">
        <input id="withdraw-{vault.vault_id}" type="number" min="0" step="0.1" bind:value={withdrawAmt} style="flex:1" />
        <button disabled={!!busy || !withdrawAmt}
          onclick={() => onAction("withdraw", vault, parseEther((parseFloat(withdrawAmt) || 0).toFixed(6)))}>Withdraw</button>
      </div>
    </div>
    <div class="row">
      <button class="danger" disabled={!!busy || vault.debt_e8s !== 0n}
        onclick={() => onAction("close", vault)}>Close vault (repay first)</button>
    </div>
  {:else}
    <div class="notice info" style="margin-top:14px">Vault is {status.toLowerCase()}.</div>
  {/if}
</div>
