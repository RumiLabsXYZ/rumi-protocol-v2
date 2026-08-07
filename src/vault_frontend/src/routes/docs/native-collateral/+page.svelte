<script lang="ts">
  // Static reference page: no live canister reads. The custody model described
  // here doesn't change per-request; live per-collateral numbers (ratios, fees,
  // debt ceilings) already live on /docs/before-you-borrow and /docs/parameters.
</script>

<svelte:head><title>Native XRP & SOL Collateral | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Native XRP &amp; SOL Collateral</h1>

  <section class="doc-section">
    <h2 class="doc-heading">Why This Is Different From Other Collateral</h2>
    <p>Most Rumi collateral (ICP, ckBTC, ckETH, ckXAUT, nICP) is an ICRC-1/ICRC-2 token that lives on the Internet Computer. Depositing it is a normal token transfer to the protocol, and the protocol can send it back the same way.</p>
    <p>XRP and SOL are not IC tokens. They live on their own chains (the XRP Ledger and Solana), and the protocol holds them there, not on the IC. This page explains how that custody works, what it means for you as a vault owner, and where it differs from every other collateral type.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Per-Vault Custody Addresses</h2>
    <p>When you open a native XRP or SOL vault, the protocol does not ask you to approve and transfer a token. Instead it derives a brand-new address on that chain, unique to your vault, using threshold signatures (threshold Ed25519, the same chain-key technology behind ckBTC and ckETH). No single party, including Rumi, holds a private key for this address; the IC subnet computes signatures collectively when a payout needs to be signed.</p>
    <p>You fund the vault by sending XRP or SOL from any wallet on that chain (Xaman for XRP, Phantom or similar for SOL) directly to the address the protocol shows you. This is an off-chain transfer on XRPL or Solana; the IC only learns about it once you ask the protocol to check.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Open, Then Verify</h2>
    <p>Opening a native vault is a two-step, user-driven flow, not an automated deposit watcher:</p>
    <ol class="doc-list doc-list-numbered">
      <li><strong>Open the vault.</strong> The protocol reserves a vault id and derives your custody address. No collateral is credited yet, and no icUSD can be minted.</li>
      <li><strong>Send the deposit, then confirm.</strong> After sending XRP or SOL to the address, you click confirm. The protocol reads the live on-chain balance at that address and credits your vault with what it finds there, minus the reserve described below.</li>
    </ol>
    <p>There is no background polling and no automatic crediting. If you send funds and never click confirm, the deposit sits at the address, recoverable, until you do.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">The Reserve</h2>
    <p>Both XRPL and Solana require an account to hold a minimum balance to exist on-chain:</p>
    <ul class="doc-list">
      <li><strong>XRP:</strong> the XRPL base reserve for a new account (currently 1 XRP), read live from the network rather than hardcoded.</li>
      <li><strong>SOL:</strong> the Solana rent-exempt minimum for a bare system account (a few hundred thousand lamports, worth a fraction of a cent), also read live rather than hardcoded.</li>
    </ul>
    <p>When you deposit, the protocol nets this reserve out of what it credits as usable collateral: your vault's collateral balance is your deposit minus the reserve, not the full amount you sent. The reserve itself stays locked at the custody address permanently.</p>
    <p>Consequence: even after you withdraw everything and repay all debt, a native XRP or SOL vault does not close and disappear the way an ICRC-collateral vault does. The custody address is still alive on-chain (holding the reserve), so the vault stays open and ready. You are not charged the reserve again if you use that same vault later. Sweeping the address to zero and letting the chain deallocate it would recover a very small amount and add real risk to the most sensitive part of the settlement code, so the protocol does not do it.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Claim-Based Settlement (Getting Collateral Back Out)</h2>
    <p>Ordinary ICRC collateral leaves your vault through a direct token transfer: you withdraw, and the tokens land in your IC wallet in the same operation. Native XRP and SOL never work this way. Collateral never leaves the protocol as a side effect of another call.</p>
    <p>Instead, every outflow (a partial withdrawal, a full withdraw-and-close, a liquidation payout, a Stability Pool payout) creates a <strong>claim</strong>: a record that says "this much XRP (or SOL) is owed to this principal." You then settle the claim separately, providing the destination address on that chain. Settling signs and broadcasts the actual on-chain payment (an XRPL Payment, or a Solana transfer), using the same threshold-signature custody as your deposit address.</p>
    <p>This two-step design (withdraw creates a claim, then you settle it) exists because moving real value on another chain is fundamentally different from an internal ledger update: it needs its own signed transaction, its own network fees, and its own confirmation. Splitting it into two steps keeps a stuck or slow settlement from blocking your vault operation, and keeps the claim visibly outstanding (in your vault panel, and on the manual liquidations page if you're a liquidator) until it is actually paid.</p>
    <p>A settlement can take a moment to confirm on the destination chain. If a first attempt does not confirm, you settle the same claim again; the protocol tracks this safely so you cannot be paid twice for the same claim.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">No Top-Ups, No Redemptions, No Bot Liquidation (For Now)</h2>
    <p>A few features that work for ICRC collateral are intentionally unavailable for native XRP and SOL vaults at launch:</p>
    <ul class="doc-list">
      <li><strong>Adding margin to an existing vault</strong> is not supported. If you want more XRP or SOL collateral, open a new vault. This avoids ambiguity between what the protocol has verified on-chain and what it has credited internally.</li>
      <li><strong>Redemptions</strong> (icUSD holders redeeming for collateral) do not draw on native XRP or SOL vaults.</li>
      <li><strong>Automated bot liquidation</strong> does not cover native XRP or SOL vaults. They can still be liquidated manually (see <a href="/docs/liquidation" class="doc-link">Liquidation Mechanics</a>) or absorbed by the <a href="/docs/stability-pool" class="doc-link">Stability Pool</a>, whose depositors can opt in to receive XRP or SOL payouts directly.</li>
    </ul>
    <p>Borrowing, repaying, partial withdrawal, and everything else about how debt and collateral ratios work is unchanged from the rest of the protocol; see <a href="/docs/before-you-borrow" class="doc-link">Before You Borrow</a> and <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for the numeric terms per collateral.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Address Validation</h2>
    <p>Whenever you type a destination address (settling a claim, or setting up a Stability Pool payout address), the app checks it looks structurally valid before submitting. This catches obvious typos, but it is not the final check: the protocol itself validates the address in full, including a cryptographic curve check for Solana addresses, before it ever signs a payment. A destination that only exists as a valid-looking string but isn't a real, spendable address is rejected by the protocol, not silently paid.</p>
  </section>
</article>

<style>
  .doc-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.35rem;
    margin: 0.5rem 0;
  }
  .doc-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
  .doc-list-numbered { list-style: decimal; }
</style>
