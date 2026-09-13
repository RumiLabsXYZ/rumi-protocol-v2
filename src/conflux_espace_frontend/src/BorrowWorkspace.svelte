<script lang="ts">
  import type { BorrowViewProps } from "./borrowView";
  import { trackGradient } from "./borrowView";

  let {
    collateral,
    debt,
    onCollateral,
    onDebt,
    inputsDisabled,
    collateralValue,
    priceLabel,
    walletBalanceLabel = "",
    walletBalanceLoading = false,
    maxCollateralDisabled = true,
    onMaxCollateral = () => {},
    walletBalanceNote = "",
    feeLabel,
    feeAmount,
    interestLabel,
    receivedAmount,
    ratioLabel,
    health,
    tone,
    ratioPosition,
    minPosition,
    liquidationPosition,
    safePosition,
    minLabel,
    liquidationLabel,
    liquidationPrice,
    positionNote,
    protocolRows,
    composition,
    protocolNote,
    connected,
    onConnect,
    onOpen,
    openDisabled,
    openLabel,
    blocker,
    actionContent,
  }: BorrowViewProps = $props();

  const inputValue = (event: Event) => (event.currentTarget as HTMLInputElement).value;
  const clampPosition = (position: number | null): number | null =>
    position === null || !Number.isFinite(position) ? null : Math.min(100, Math.max(0, position));

  const dotColors = (members: { color: string }[]): string[] => [...new Set(members.map(m => m.color))].slice(0, 3);
</script>

<div class="borrow-workspace">
  <section class="protocol-panel" aria-labelledby="protocol-overview-title">
    <div class="panel-heading">
      <h2 id="protocol-overview-title">Protocol overview</h2>
      <p>Across Rumi Protocol</p>
    </div>

    <div class="overview-rows">
      {#each protocolRows as row (row.label)}
        <div class="overview-row">
          <span>{row.label}</span>
          <strong>{row.value}</strong>
        </div>
      {/each}
    </div>
    {#if protocolNote}
      <p class="protocol-note">{protocolNote}</p>
    {/if}

    <details class="composition-details">
      <summary>
        <span class="summary-copy">
          <strong>Collateral composition</strong>
          <span>View the assets backing the protocol</span>
        </span>
        <img src="/brand/chevron-right.svg" alt="" aria-hidden="true" />
      </summary>

      <div class="composition-content">
        {#if composition === null}
          <p class="composition-empty">Unavailable</p>
        {:else if composition.length === 0}
          <p class="composition-empty">No collateral returned</p>
        {:else}
          {#each composition as group (group.label)}
            <div class="composition-group">
              {#if group.label === 'ICP Ecosystem' || group.members.length > 1}
                <details class="composition-nested">
                  <summary>
                    <span class="composition-label">
                      <span class="dot-cluster">
                        {#each dotColors(group.members) as color}
                          <i class="composition-dot" style="background:{color};"></i>
                        {/each}
                      </span>
                      {group.label}
                    </span>
                    <strong>{group.value}</strong>
                    <img src="/brand/chevron-right.svg" alt="" aria-hidden="true" />
                  </summary>
                  <ul>
                    {#each group.members as member}
                      <li>
                        <i class="composition-dot" style="background:{member.color};"></i>
                        <span>{member.amount} {member.symbol}</span>
                        <span>{member.value}</span>
                      </li>
                    {/each}
                  </ul>
                </details>
              {:else}
                <div class="composition-row">
                  <span class="composition-label">
                    <i class="composition-dot" style="background:{group.members[0]?.color ?? '#94A3B8' };"></i>
                    {group.label}
                  </span>
                  <strong>{group.value}</strong>
                </div>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    </details>

    <div class="parameter-block">
      <h3>CFX parameters</h3>
      <div class="parameter-row">
        <span>Minimum collateral ratio</span>
        <strong>{minLabel}</strong>
      </div>
      <div class="parameter-row">
        <span>Liquidation ratio</span>
        <strong>{liquidationLabel}</strong>
      </div>
      <a class="text-link" href="https://app.rumiprotocol.com/transparency" target="_blank" rel="noreferrer">
        Protocol transparency
        <img src="/brand/arrow-up-right.svg" alt="" aria-hidden="true" />
      </a>
    </div>
  </section>

  <section class="transaction-panel" aria-labelledby="transaction-title">
    <h2 id="transaction-title">CFX collateral</h2>

    <div class="asset-field">
      <div class="field-top">
        <label class="sr-only" for="borrow-collateral">CFX collateral</label>
        {#if walletBalanceLabel || walletBalanceLoading}
          <div class="wallet-balance" aria-live="polite">
            <span>{walletBalanceLoading ? "Balance: Loading..." : walletBalanceLabel}</span>
            <button
              type="button"
              class="max-button"
              disabled={maxCollateralDisabled || walletBalanceLoading || inputsDisabled}
              onclick={onMaxCollateral}
            >Max</button>
          </div>
        {/if}
      </div>
      <div class="input-shell">
        <input
          id="borrow-collateral"
          type="text"
          inputmode="decimal"
          autocomplete="off"
          spellcheck="false"
          value={collateral}
          disabled={inputsDisabled}
          aria-label="CFX collateral amount"
          oninput={(event) => onCollateral(inputValue(event))}
        />
        <span class="asset-symbol">
          <img src="/brand/cfx.svg" alt="" aria-hidden="true" />
          <strong>CFX</strong>
        </span>
      </div>
      <div class="field-caption">
        <span>{collateralValue}</span>
        <span>{priceLabel}</span>
      </div>
      {#if walletBalanceNote}
        <p class="wallet-balance-note">{walletBalanceNote}</p>
      {/if}
    </div>

    <div class="asset-field debt-field">
      <label for="borrow-debt">icUSD to borrow</label>
      <div class="input-shell">
        <input
          id="borrow-debt"
          type="text"
          inputmode="decimal"
          autocomplete="off"
          spellcheck="false"
          value={debt}
          disabled={inputsDisabled}
          aria-label="icUSD amount to borrow"
          oninput={(event) => onDebt(inputValue(event))}
        />
        <span class="asset-symbol">
          <img src="/brand/icusd.svg" alt="" aria-hidden="true" />
          <strong>icUSD</strong>
        </span>
      </div>
    </div>

    <div class="costs" aria-label="Borrow costs">
      <div class="cost-row"><span>{feeLabel}</span><strong>{feeAmount}</strong></div>
      <div class="cost-row"><span>Interest rate</span><strong>{interestLabel}</strong></div>
      <div class="cost-row received"><span>You receive</span><strong>{receivedAmount}</strong></div>
    </div>

    <div class="health-card" class:tone-safe={tone === "safe" } class:tone-caution={tone === "caution" } class:tone-danger={tone === "danger" } class:tone-unavailable={tone === "unavailable" }>
      <div class="health-heading">
        <strong>Projected position</strong>
        <span>{ratioLabel} · {health}</span>
      </div>

      <div class="meter" aria-label="Projected collateral ratio">
        <div class="meter-track" aria-hidden="true" style="background:{trackGradient(liquidationPosition, safePosition)};">
          {#if liquidationPosition !== null && clampPosition(liquidationPosition) !== null}
            <span class="meter-tick" style={`left: ${clampPosition(liquidationPosition)}%`}></span>
          {/if}
          {#if minPosition !== null && clampPosition(minPosition) !== null}
            <span class="meter-tick" style={`left: ${clampPosition(minPosition)}%`}></span>
          {/if}
          {#if ratioPosition !== null && clampPosition(ratioPosition) !== null}
            <span class="meter-marker" style={`left: ${clampPosition(ratioPosition)}%`}></span>
          {/if}
        </div>
        <div class="meter-axis">
          <span>100%</span>
          <span>300%</span>
        </div>
        <div class="threshold-legend" aria-label="Collateral ratio thresholds">
          {#if liquidationPosition !== null}
            <span><i class="threshold-swatch liquidation"></i>Liquidation · {liquidationLabel}</span>
          {/if}
          {#if minPosition !== null}
            <span><i class="threshold-swatch minimum"></i>Minimum · {minLabel}</span>
          {/if}
        </div>
      </div>

      <div class="liquidation-row">
        <span>Estimated liquidation price</span>
        <strong>{liquidationPrice}</strong>
      </div>
      <div class="position-note">
        <img src="/brand/information-circle.svg" alt="" aria-hidden="true" />
        <span>{positionNote}</span>
        <a href="https://app.rumiprotocol.com/docs" target="_blank" rel="noreferrer">
          How liquidation works
          <img src="/brand/arrow-up-right.svg" alt="" aria-hidden="true" />
        </a>
      </div>
    </div>

    {#if blocker}
      <p class="blocker" role="alert">
        <img src="/brand/information-circle.svg" alt="" aria-hidden="true" />
        <span>{blocker}</span>
      </p>
    {/if}

    {#if connected}
      <button class="action-button" type="button" disabled={openDisabled} onclick={onOpen}>
        <span>{openLabel}</span>
      </button>
    {:else}
      <button class="action-button" type="button" onclick={onConnect}>
        <img src="/brand/wallet.svg" alt="" aria-hidden="true" />
        <span>Connect wallet to continue</span>
      </button>
    {/if}
    <p class="wallet-helper">MetaMask or Rabby</p>

    {#if actionContent}
      <div class="action-content">{@render actionContent()}</div>
    {/if}
  </section>
</div>

<style>
  .borrow-workspace {
    --workspace-bg: #080b16;
    --workspace-surface: #0e1222;
    --workspace-surface-raised: #11182b;
    --workspace-border: #223758;
    --workspace-text: #e8e4f0;
    --workspace-muted: #a09bb5;
    --workspace-blue: #35a7ff;
    --workspace-teal: #29e2b2;
    --workspace-pink: #ef69ad;
    --workspace-violet: #a478f6;
    display: grid;
    grid-template-columns: minmax(0, 40fr) minmax(0, 60fr);
    gap: 24px;
    color: var(--workspace-text);
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
    min-width: 0;
  }

  .protocol-panel,
  .transaction-panel {
    min-width: 0;
    border: 1px solid var(--workspace-border);
    border-radius: 11px;
    background: linear-gradient(145deg, rgba(14, 18, 34, .98), rgba(10, 15, 29, .98));
  }

  .protocol-panel { padding: 28px 30px 30px; }
  .transaction-panel { padding: 22px 30px 20px; }

  h2,
  h3,
  strong,
  summary {
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  }

  h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -.01em;
    line-height: 1.25;
  }

  .panel-heading p {
    margin: 5px 0 26px;
    color: #9c9fc9;
    font-size: 16px;
  }

  .overview-rows { border-bottom: 1px solid var(--workspace-border); }
  .protocol-note { margin: 12px 0 0; color: var(--workspace-muted); font-size: 13px; line-height: 1.5; }
  .overview-row,
  .parameter-row,
  .cost-row,
  .liquidation-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    min-width: 0;
    border-bottom: 1px solid rgba(34, 55, 88, .72);
    padding: 10px 0;
    color: #c2c1d6;
    font-size: 15px;
    line-height: 1.35;
  }

  .overview-row:last-child { border-bottom: 0; }
  .overview-row span,
  .parameter-row span,
  .cost-row span,
  .liquidation-row span { min-width: 0; overflow-wrap: anywhere; }
  .overview-row strong,
  .parameter-row strong,
  .cost-row strong,
  .liquidation-row strong { color: var(--workspace-text); font-weight: 600; text-align: right; overflow-wrap: anywhere; }

  .composition-details {
    margin: 26px 0 24px;
    border: 1px solid var(--workspace-border);
    border-radius: 10px;
    background: rgba(17, 24, 43, .72);
  }

  summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    min-height: 88px;
    padding: 17px 20px;
    cursor: pointer;
    list-style: none;
  }

  summary::-webkit-details-marker { display: none; }
  summary:focus-visible,
  .text-link:focus-visible,
  .position-note a:focus-visible,
  .action-button:focus-visible,
  .max-button:focus-visible,
  input:focus-visible { outline: 3px solid rgba(53, 167, 255, .72); outline-offset: 3px; }
  summary > img { width: 22px; height: 22px; flex: none; filter: brightness(0) invert(1); transition: transform .16s ease; }
  details[open] > summary > img { transform: rotate(90deg); }
  .summary-copy { display: grid; gap: 4px; min-width: 0; }
  .summary-copy strong { font-size: 17px; font-weight: 600; }
  .summary-copy span { color: var(--workspace-muted); font-size: 14px; }
  .composition-content { border-top: 1px solid var(--workspace-border); padding: 10px 20px 16px; }
  .composition-group { padding: 8px 0; }
  .composition-row,
  .composition-nested summary { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .composition-row { padding: 3px 0; font-size: 14px; }
  .composition-row strong,
  .composition-nested summary strong { font-weight: 600; text-align: right; }
  .composition-label { display: inline-flex; align-items: center; gap: 8px; min-width: 0; overflow-wrap: anywhere; }
  .composition-dot { display: inline-block; flex: none; width: 8px; height: 8px; border-radius: 50%; }
  .dot-cluster { display: inline-flex; align-items: center; }
  .dot-cluster .composition-dot + .composition-dot { margin-left: -3px; box-shadow: 0 0 0 2px rgba(17, 24, 43, .9); }
  .composition-nested { margin: 0; border: 0; background: transparent; }
  .composition-nested summary {
    min-height: 0;
    padding: 6px 0;
    font-size: 14px;
    list-style: none;
  }
  .composition-nested summary > img { width: 16px; height: 16px; }
  .composition-nested ul { margin: 2px 0 6px; padding-left: 0; list-style: none; color: var(--workspace-muted); font-size: 13px; line-height: 1.5; }
  .composition-nested li { display: flex; align-items: center; gap: 8px; padding: 4px 0 4px 16px; }
  .composition-nested li span:first-of-type { flex: 1; min-width: 0; overflow-wrap: anywhere; }
  .composition-empty { color: var(--workspace-muted); font-size: 13px; line-height: 1.5; margin: 10px 0 2px; }

  .parameter-block { border-top: 1px solid var(--workspace-border); padding-top: 24px; }
  h3 { margin: 0 0 9px; font-size: 17px; font-weight: 600; }
  .parameter-row:last-of-type { border-bottom: 0; }
  .text-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 13px;
    color: var(--workspace-blue);
    font-size: 15px;
    font-weight: 600;
    text-decoration: none;
  }
  .text-link img,
  .position-note a img { width: 15px; height: 15px; flex: none; filter: invert(58%) sepia(95%) saturate(1605%) hue-rotate(177deg) brightness(101%) contrast(105%); }
  .sr-only { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }

  .transaction-panel > h2 { margin: 0 0 10px; }
  .asset-field label {
    display: block;
    margin: 0 0 8px;
    color: var(--workspace-text);
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
    font-size: 17px;
    font-weight: 600;
  }
  .field-top { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; min-height: 8px; }
  .field-top label { margin: 0; }
  .wallet-balance { display: flex; align-items: center; gap: 10px; margin-left: auto; color: var(--workspace-muted); font-size: 13px; }
  .max-button {
    border: 1px solid var(--workspace-border);
    border-radius: 999px;
    padding: 3px 11px;
    background: rgba(53, 167, 255, .1);
    color: var(--workspace-blue);
    cursor: pointer;
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: .02em;
    text-transform: uppercase;
  }
  .max-button:hover:not(:disabled) { background: rgba(53, 167, 255, .18); }
  .max-button:disabled { cursor: not-allowed; opacity: .45; }
  .wallet-balance-note { margin: 8px 0 0; color: var(--workspace-muted); font-size: 12px; line-height: 1.4; }
  .debt-field { margin-top: 21px; }
  .debt-field label { margin: 0 0 8px; }
  .input-shell {
    display: flex;
    align-items: center;
    min-height: 80px;
    overflow: hidden;
    border: 1px solid var(--workspace-border);
    border-radius: 7px;
    background: rgba(17, 24, 43, .84);
  }
  .asset-field .input-shell { margin-top: 8px; }
  .input-shell:focus-within { border-color: var(--workspace-blue); box-shadow: 0 0 0 1px rgba(53, 167, 255, .22); }
  input {
    min-width: 0;
    flex: 1;
    border: 0;
    outline: 0;
    padding: 7px 20px;
    background: transparent;
    color: var(--workspace-text);
    font-family: Inter, ui-sans-serif, system-ui, sans-serif;
    font-size: clamp(28px, 3.2vw, 46px);
    font-weight: 700;
    line-height: 1;
  }
  input:disabled { cursor: not-allowed; opacity: .72; }
  .asset-symbol {
    display: inline-flex;
    align-items: center;
    gap: 12px;
    flex: none;
    min-width: 157px;
    margin: 10px 14px 10px 0;
    padding: 6px 0 6px 21px;
    border-left: 1px solid var(--workspace-border);
    font-size: 17px;
  }
  .asset-symbol img { width: 44px; height: 44px; object-fit: contain; }
  .field-caption {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    margin-top: 8px;
    color: #c1bed5;
    font-size: 15px;
    line-height: 1.35;
  }
  .field-caption span:last-child { text-align: right; overflow-wrap: anywhere; }
  .costs { margin-top: 20px; }
  .cost-row:first-child { border-top: 0; }
  .cost-row.received { padding-top: 11px; border-bottom: 0; color: var(--workspace-text); font-family: Inter, ui-sans-serif, system-ui, sans-serif; font-size: 17px; }

  .health-card { margin-top: 14px; border: 1px solid var(--workspace-border); border-radius: 10px; background: rgba(17, 24, 43, .78); padding: 17px 19px 14px; }
  .health-heading { display: flex; align-items: baseline; justify-content: space-between; gap: 14px; }
  .health-heading strong { font-size: 17px; font-weight: 600; }
  .health-heading span { color: var(--workspace-teal); font-family: Inter, ui-sans-serif, system-ui, sans-serif; font-size: clamp(19px, 2vw, 28px); font-weight: 700; text-align: right; overflow-wrap: anywhere; }
  .tone-caution .health-heading span { color: #f8c45b; }
  .tone-danger .health-heading span { color: #fa759e; }
  .tone-unavailable .health-heading span { color: var(--workspace-muted); }
  .meter { margin-top: 16px; }
  .meter-track { position: relative; height: 12px; overflow: visible; border-radius: 999px; background: rgba(160, 155, 181, .22); transition: background .2s ease; }
  .meter-tick { position: absolute; top: 12px; width: 1px; height: 8px; background: #d4cde4; }
  .meter-marker { position: absolute; top: 50%; width: 22px; height: 22px; transform: translate(-50%, -50%); border: 4px solid #dffcf6; border-radius: 50%; background: var(--workspace-teal); box-shadow: 0 0 0 1px rgba(8, 11, 22, .5); }
  .tone-caution .meter-marker { background: #f8c45b; }
  .tone-danger .meter-marker { background: var(--workspace-pink); }
  .tone-unavailable .meter-marker { background: var(--workspace-muted); }
  .meter-axis { display: flex; justify-content: space-between; min-height: 26px; padding-top: 9px; color: #b9b4ca; font-size: 13px; }
  .threshold-legend { display: flex; flex-wrap: wrap; gap: 5px 18px; padding-top: 1px; color: #c9c4d7; font-size: 12px; line-height: 1.45; }
  .threshold-legend > span { display: inline-flex; align-items: center; gap: 5px; }
  .threshold-swatch { width: 8px; height: 8px; flex: none; border-radius: 50%; }
  .threshold-swatch.liquidation { background: var(--workspace-pink); }
  .threshold-swatch.minimum { background: var(--workspace-violet); }
  .liquidation-row { margin-top: 5px; padding: 11px 0; }
  .position-note { display: grid; grid-template-columns: 19px minmax(0, 1fr) auto; align-items: center; gap: 9px; padding-top: 10px; color: var(--workspace-muted); font-size: 14px; line-height: 1.45; }
  .position-note > img { width: 19px; height: 19px; filter: brightness(0) invert(1); }
  .position-note a { display: inline-flex; align-items: center; gap: 5px; color: var(--workspace-blue); text-decoration: none; white-space: nowrap; }
  .blocker { display: flex; align-items: flex-start; gap: 8px; margin: 13px 0 0; border: 1px solid rgba(239, 105, 173, .48); border-radius: 7px; padding: 10px 12px; background: rgba(100, 27, 70, .24); color: #f6b2d4; font-size: 13px; line-height: 1.45; }
  .blocker img { width: 17px; height: 17px; flex: none; margin-top: 1px; filter: brightness(0) invert(1); }
  .action-button { display: flex; align-items: center; justify-content: center; gap: 10px; width: 100%; min-height: 53px; margin-top: 14px; border: 0; border-radius: 8px; padding: 12px 20px; background: var(--workspace-teal); color: #041a17; cursor: pointer; font-family: Inter, ui-sans-serif, system-ui, sans-serif; font-size: 17px; font-weight: 700; transition: filter .15s ease, transform .05s ease; }
  .action-button img { width: 21px; height: 21px; }
  .action-button:hover:not(:disabled) { filter: brightness(1.08); }
  .action-button:active:not(:disabled) { transform: translateY(1px); }
  .action-button:disabled { cursor: not-allowed; opacity: .52; }
  .wallet-helper { margin: 8px 0 0; color: var(--workspace-muted); font-size: 14px; text-align: center; }
  .action-content { margin-top: 12px; }

  @media (max-width: 980px) {
    .borrow-workspace { grid-template-columns: minmax(0, 1fr); gap: 16px; }
    .transaction-panel { order: 1; }
    .protocol-panel { order: 2; }
  }

  @media (max-width: 480px) {
    .protocol-panel,
    .transaction-panel { padding: 20px 16px; }
    .asset-symbol { min-width: 104px; gap: 7px; margin-right: 10px; padding-left: 11px; font-size: 15px; }
    .asset-symbol img { width: 36px; height: 36px; }
    input { padding-left: 13px; padding-right: 10px; font-size: 30px; }
    .position-note { grid-template-columns: 19px minmax(0, 1fr); }
    .position-note a { grid-column: 2; white-space: normal; }
    .meter-axis { font-size: 12px; }
    .threshold-legend { gap: 4px 10px; font-size: 11px; }
    .field-top { flex-wrap: wrap; }
  }
</style>
