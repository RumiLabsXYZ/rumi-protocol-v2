<script lang="ts">
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import CRDial from '$components/explorer/entity/CRDial.svelte';
  import VaultCRChart from '$components/explorer/entity/VaultCRChart.svelte';
  import MiniAreaChart from '$components/explorer/MiniAreaChart.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import {
    fetchVault, fetchVaultInterestRate, fetchCollateralConfigs,
    fetchCollateralPrices, fetchVaultHistory, fetchVaultsByOwner,
    type VaultHistoryEntry,
  } from '$services/explorer/explorerService';
  import { fetchPriceSeries } from '$services/explorer/analyticsService';
  import {
    formatE8s, formatUsdRaw, formatPercent, getTokenSymbol, getTokenDecimals,
  } from '$utils/explorerHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';

  const vaultId = $derived(Number($page.params.id));

  let vault = $state<any>(null);
  let interestRate = $state<number | null>(null);
  let collateralConfigs = $state<any[]>([]);
  let collateralPrices = $state<Map<string, number>>(new Map());
  let history = $state<VaultHistoryEntry[]>([]);
  let ownerVaults = $state<any[]>([]);
  let priceSeries = $state<{ timestamp_ns: bigint; prices: Array<[any, number]> }[]>([]);

  let loadingVault = $state(true);
  let loadingCore = $state(true);
  let loadingHistory = $state(true);
  let loadError = $state<string | null>(null);

  const collateralPrincipalStr = $derived(
    vault?.collateral_type ? (vault.collateral_type.toString?.() ?? vault.collateral_type.toText?.() ?? String(vault.collateral_type)) : ''
  );

  const config = $derived.by(() => {
    if (!vault || collateralConfigs.length === 0) return null;
    return collateralConfigs.find((c: any) => {
      const cPrincipal = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id ?? '');
      return cPrincipal === collateralPrincipalStr;
    }) ?? null;
  });

  const decimals = $derived(collateralPrincipalStr ? getTokenDecimals(collateralPrincipalStr) : 8);
  const tokenSymbol = $derived(collateralPrincipalStr ? getTokenSymbol(collateralPrincipalStr) : 'tokens');

  const price = $derived.by(() => {
    if (!vault || collateralPrices.size === 0) return 0;
    return collateralPrices.get(collateralPrincipalStr) ?? 0;
  });

  const ownerStr = $derived(vault?.owner?.toString?.() ?? vault?.owner?.toText?.() ?? '');

  const collateralAmount = $derived(vault ? BigInt(vault.collateral_amount ?? 0) : 0n);
  const collateralHuman = $derived(Number(collateralAmount) / 10 ** decimals);
  const collateralValueUsd = $derived(collateralHuman * price);

  const borrowedAmount = $derived(vault ? BigInt(vault.borrowed_icusd_amount ?? 0) : 0n);
  const accruedInterest = $derived(vault ? BigInt(vault.accrued_interest ?? 0) : 0n);
  const totalDebt = $derived(borrowedAmount + accruedInterest);
  const totalDebtHuman = $derived(Number(totalDebt) / 1e8);

  const cr = $derived(totalDebtHuman > 0 ? collateralValueUsd / totalDebtHuman : Infinity);
  const crPct = $derived(cr === Infinity ? Infinity : cr * 100);

  function decodeField(field: any, fallback: number = 0): number {
    if (!field) return fallback;
    if (field instanceof Uint8Array || Array.isArray(field)) return decodeRustDecimal(field);
    return Number(field) || fallback;
  }

  const liquidationRatio = $derived(decodeField(config?.liquidation_ratio, 1.1));
  const redemptionTier = $derived(config?.redemption_tier ?? null);

  const liquidationPrice = $derived.by(() => {
    if (collateralHuman <= 0 || totalDebtHuman <= 0) return 0;
    return (totalDebtHuman * liquidationRatio) / collateralHuman;
  });

  const headroomUsd = $derived.by(() => {
    if (cr === Infinity || liquidationPrice <= 0) return 0;
    return collateralValueUsd - totalDebtHuman * liquidationRatio;
  });

  const isClosed = $derived(
    vault != null && collateralAmount === 0n && totalDebt === 0n && history.length > 0,
  );

  const vaultCollateralMap = $derived(
    vault ? new Map([[Number(vault.vault_id), collateralPrincipalStr]]) : new Map<number, string>(),
  );

  const sortedHistory = $derived([...history].reverse());

  const creationTimestamp = $derived.by(() => {
    if (!history.length) return null;
    const first = history[0]?.event;
    const eventType = first?.event_type ?? first;
    const key = Object.keys(eventType)[0];
    const data = key ? eventType[key] : null;
    if (data?.timestamp) {
      const ts = Array.isArray(data.timestamp) ? data.timestamp[0] : data.timestamp;
      return ts;
    }
    if (data?.vault?.last_accrual_time) return data.vault.last_accrual_time;
    return null;
  });

  const lastActivityTimestamp = $derived.by(() => {
    if (!history.length) return null;
    const last = history[history.length - 1]?.event;
    const eventType = last?.event_type ?? last;
    const key = Object.keys(eventType)[0];
    const data = key ? eventType[key] : null;
    if (data?.timestamp) {
      const ts = Array.isArray(data.timestamp) ? data.timestamp[0] : data.timestamp;
      return ts;
    }
    return null;
  });

  /** Extract every principal referenced by history events (excluding owner). */
  const touchedBy = $derived.by(() => {
    const seen = new Map<string, Set<string>>();
    for (const entry of history) {
      const evt = entry?.event;
      const eventType = evt?.event_type ?? evt;
      const key = Object.keys(eventType)[0];
      if (!key) continue;
      const data = eventType[key];
      if (!data) continue;
      for (const field of ['liquidator', 'redeemer', 'caller']) {
        const raw = data[field];
        let principal: string | null = null;
        if (raw && typeof raw === 'object' && typeof raw.toText === 'function') {
          principal = raw.toText();
        } else if (Array.isArray(raw) && raw.length > 0) {
          const inner = raw[0];
          if (inner && typeof inner === 'object' && typeof inner.toText === 'function') principal = inner.toText();
        }
        if (!principal || principal === ownerStr) continue;
        const roles = seen.get(principal) ?? new Set<string>();
        roles.add(field === 'caller' ? 'caller' : field);
        seen.set(principal, roles);
      }
    }
    return Array.from(seen.entries()).map(([principal, roles]) => ({ principal, roles: Array.from(roles) }));
  });

  const otherOwnerVaults = $derived(
    ownerVaults.filter((v: any) => Number(v.vault_id) !== vaultId),
  );

  /**
   * Decorate each of the owner's other vaults with its CR + liquidation ratio
   * so we can render a small CRDial per chip. Falls back to Infinity when debt
   * is zero or price is missing.
   */
  const otherOwnerVaultsAnnotated = $derived.by(() => {
    return otherOwnerVaults.map((v: any) => {
      const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
      const dec = getTokenDecimals(ct);
      const coll = Number(BigInt(v.collateral_amount ?? 0)) / 10 ** dec;
      const debt = Number(BigInt(v.borrowed_icusd_amount ?? 0) + BigInt(v.accrued_interest ?? 0)) / 1e8;
      const p = collateralPrices.get(ct) ?? 0;
      const cfg = collateralConfigs.find((c: any) => {
        const cPrincipal = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id ?? '');
        return cPrincipal === ct;
      });
      const liq = decodeField(cfg?.liquidation_ratio, 1.1);
      const crVal = debt > 0 && p > 0 ? (coll * p) / debt : Infinity;
      return {
        id: Number(v.vault_id),
        cr: crVal,
        liq,
      };
    });
  });

  // ── Timeline reconstruction ──────────────────────────────────────────────
  interface TimelinePoint {
    t: number;
    cr: number;
    collateral: number;
    debt: number;
    price: number;
  }

  const timeline = $derived.by((): TimelinePoint[] => {
    if (!history.length) return [];
    const priceByTs: Array<{ t: number; p: number }> = [];
    for (const snap of priceSeries) {
      const match = snap.prices?.find((pair: any) => {
        const pid = typeof pair[0] === 'object' ? pair[0]?.toText?.() ?? String(pair[0]) : String(pair[0]);
        return pid === collateralPrincipalStr;
      });
      if (match) priceByTs.push({ t: Number(snap.timestamp_ns), p: match[1] });
    }
    priceByTs.sort((a, b) => a.t - b.t);

    function priceAt(t: number): number {
      if (!priceByTs.length) return price;
      let lo = 0;
      let hi = priceByTs.length - 1;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (priceByTs[mid].t < t) lo = mid + 1;
        else hi = mid;
      }
      const candidate = priceByTs[lo];
      const prev = lo > 0 ? priceByTs[lo - 1] : candidate;
      return (Math.abs(candidate.t - t) < Math.abs(prev.t - t) ? candidate : prev).p;
    }

    let collE8s = 0n;
    let debtE8s = 0n;
    const points: TimelinePoint[] = [];

    for (const entry of history) {
      const evt = entry?.event;
      const eventType = evt?.event_type ?? evt;
      const key = Object.keys(eventType)[0];
      if (!key) continue;
      const d = eventType[key];
      const tsRaw = d?.timestamp;
      const ts = Array.isArray(tsRaw) ? tsRaw[0] : tsRaw;
      const t = ts ? Number(ts) : (d?.vault?.last_accrual_time ? Number(d.vault.last_accrual_time) : 0);

      switch (key) {
        case 'open_vault': {
          collE8s = BigInt(d.vault?.collateral_amount ?? 0);
          debtE8s = BigInt(d.vault?.borrowed_icusd_amount ?? 0);
          break;
        }
        case 'borrow_from_vault': {
          debtE8s += BigInt(d.borrowed_amount ?? 0) + BigInt(d.fee_amount ?? 0);
          break;
        }
        case 'repay_to_vault': {
          debtE8s -= BigInt(d.repayed_amount ?? 0);
          if (debtE8s < 0n) debtE8s = 0n;
          break;
        }
        case 'add_margin_to_vault': {
          collE8s += BigInt(d.margin_added ?? 0);
          break;
        }
        case 'partial_collateral_withdrawn': {
          collE8s -= BigInt(d.amount ?? 0);
          if (collE8s < 0n) collE8s = 0n;
          break;
        }
        case 'collateral_withdrawn':
        case 'withdraw_and_close_vault':
        case 'vault_withdrawn_and_closed': {
          collE8s = 0n;
          break;
        }
        case 'close_vault': {
          collE8s = 0n;
          debtE8s = 0n;
          break;
        }
        case 'liquidate_vault': {
          collE8s = 0n;
          debtE8s = 0n;
          break;
        }
        case 'partial_liquidate_vault': {
          collE8s -= BigInt(d.icp_to_liquidator ?? 0);
          const pf = Array.isArray(d.protocol_fee_collateral) ? d.protocol_fee_collateral[0] : d.protocol_fee_collateral;
          if (pf != null) collE8s -= BigInt(pf);
          debtE8s -= BigInt(d.liquidator_payment ?? 0);
          if (collE8s < 0n) collE8s = 0n;
          if (debtE8s < 0n) debtE8s = 0n;
          break;
        }
        case 'redemption_on_vaults': {
          const rawRedemptions = d.vault_redemptions;
          const redemptions: any[] = Array.isArray(rawRedemptions) && rawRedemptions.length > 0
            ? rawRedemptions[0]
            : Array.isArray(rawRedemptions) ? rawRedemptions : [];
          const mine = redemptions.find((vr: any) => Number(vr.vault_id) === vaultId);
          if (mine) {
            collE8s -= BigInt(mine.collateral_amount ?? 0);
            debtE8s -= BigInt(mine.icusd_amount ?? 0);
            if (collE8s < 0n) collE8s = 0n;
            if (debtE8s < 0n) debtE8s = 0n;
          }
          break;
        }
      }

      if (!t) continue;
      const collHuman = Number(collE8s) / 10 ** decimals;
      const debtHuman = Number(debtE8s) / 1e8;
      const p = priceAt(t);
      const crAt = debtHuman > 0 && p > 0 ? (collHuman * p) / debtHuman : Infinity;
      // Multiple backend events frequently share the same nanosecond timestamp
      // (e.g. open_vault + borrow_from_vault recorded in the same transaction).
      // Fold consecutive same-t entries into a single point so charts that
      // key by timestamp don't blow up with duplicate keys, and so the dot
      // overlay doesn't paint two circles on top of each other.
      const last = points.length > 0 ? points[points.length - 1] : null;
      if (last && last.t === t) {
        points[points.length - 1] = { t, cr: crAt, collateral: collHuman, debt: debtHuman, price: p };
      } else {
        points.push({ t, cr: crAt, collateral: collHuman, debt: debtHuman, price: p });
      }
    }

    return points;
  });

  const debtPoints = $derived(timeline.map((pt) => ({ t: pt.t, v: pt.debt })));
  const collateralPoints = $derived(timeline.map((pt) => ({ t: pt.t, v: pt.collateral })));

  /**
   * Rebuild a vault-shaped object from event history so closed vaults can
   * render their full historical state. Uses the first open_vault event for
   * identity (owner, collateral_type) and replays events to derive final
   * collateral + debt (which for a closed vault end at zero).
   */
  function synthesizeVaultFromHistory(id: number, h: VaultHistoryEntry[]): any | null {
    if (!h.length) return null;
    // History can contain owner-keyed events that pre-date this vault (e.g. a
    // redemption_on_vaults that the same principal performed against other
    // vaults), so scan forward for the open_vault entry rather than assuming
    // it sits at index 0.
    let openIdx = -1;
    let openVault: any = null;
    for (let i = 0; i < h.length; i++) {
      const evt = h[i]?.event;
      const et = evt?.event_type ?? evt;
      const key = Object.keys(et)[0];
      if (key !== 'open_vault') continue;
      const v = et[key]?.vault;
      if (v && Number(v.vault_id) === id) {
        openIdx = i;
        openVault = v;
        break;
      }
    }
    if (!openVault) return null;

    let coll = BigInt(openVault.collateral_amount ?? 0);
    let debt = BigInt(openVault.borrowed_icusd_amount ?? 0);
    for (let i = openIdx + 1; i < h.length; i++) {
      const evt = h[i]?.event;
      const et = evt?.event_type ?? evt;
      const key = Object.keys(et)[0];
      const d = et[key];
      switch (key) {
        case 'borrow_from_vault':
          debt += BigInt(d.borrowed_amount ?? 0) + BigInt(d.fee_amount ?? 0); break;
        case 'repay_to_vault':
          debt -= BigInt(d.repayed_amount ?? 0); if (debt < 0n) debt = 0n; break;
        case 'add_margin_to_vault':
          coll += BigInt(d.margin_added ?? 0); break;
        case 'partial_collateral_withdrawn':
          coll -= BigInt(d.amount ?? 0); if (coll < 0n) coll = 0n; break;
        case 'collateral_withdrawn':
        case 'withdraw_and_close_vault':
        case 'vault_withdrawn_and_closed':
          coll = 0n; break;
        case 'close_vault':
          coll = 0n; debt = 0n; break;
        case 'liquidate_vault':
          coll = 0n; debt = 0n; break;
        case 'partial_liquidate_vault': {
          coll -= BigInt(d.icp_to_liquidator ?? 0);
          const pf = Array.isArray(d.protocol_fee_collateral) ? d.protocol_fee_collateral[0] : d.protocol_fee_collateral;
          if (pf != null) coll -= BigInt(pf);
          debt -= BigInt(d.liquidator_payment ?? 0);
          if (coll < 0n) coll = 0n;
          if (debt < 0n) debt = 0n;
          break;
        }
        case 'redemption_on_vaults': {
          const rawRedemptions = d.vault_redemptions;
          const redemptions: any[] = Array.isArray(rawRedemptions) && rawRedemptions.length > 0
            ? rawRedemptions[0]
            : Array.isArray(rawRedemptions) ? rawRedemptions : [];
          const mine = redemptions.find((vr: any) => Number(vr.vault_id) === id);
          if (mine) {
            coll -= BigInt(mine.collateral_amount ?? 0);
            debt -= BigInt(mine.icusd_amount ?? 0);
            if (coll < 0n) coll = 0n;
            if (debt < 0n) debt = 0n;
          }
          break;
        }
      }
    }
    return {
      vault_id: id,
      owner: openVault.owner,
      collateral_type: openVault.collateral_type,
      collateral_amount: coll,
      borrowed_icusd_amount: debt,
      accrued_interest: 0n,
      icp_margin_amount: coll,
      last_accrual_time: openVault.last_accrual_time ?? 0n,
      _synthetic: true,
    };
  }

  async function loadVault() {
    loadingVault = true;
    loadingCore = true;
    loadingHistory = true;
    loadError = null;
    // Reset prior-vault state so navigating between /vault/X and /vault/Y
    // doesn't briefly render Y with X's data while the new fetches resolve.
    vault = null;
    history = [];
    ownerVaults = [];
    try {
      if (!Number.isFinite(vaultId)) {
        loadError = `Invalid vault id: "${$page.params.id}"`;
        return;
      }
      const id = BigInt(vaultId);
      const [v, r, configs, prices, h, pseries] = await Promise.all([
        fetchVault(id).catch(() => null),
        fetchVaultInterestRate(id).catch(() => null),
        fetchCollateralConfigs().catch(() => []),
        fetchCollateralPrices().catch(() => new Map()),
        fetchVaultHistory(id).catch(() => []),
        fetchPriceSeries(500).catch(() => []),
      ]);
      const histArr = Array.isArray(h) ? h : [];
      const resolved = v ?? synthesizeVaultFromHistory(vaultId, histArr);
      if (!resolved) {
        loadError = `Vault #${vaultId} does not exist or has never recorded any events.`;
      } else {
        vault = resolved;
        interestRate = r;
        collateralConfigs = configs;
        collateralPrices = prices;
        history = histArr;
        priceSeries = pseries as any[];
        if (resolved.owner) {
          const ownerPrincipal = typeof resolved.owner === 'object'
            ? resolved.owner as Principal
            : Principal.fromText(String(resolved.owner));
          fetchVaultsByOwner(ownerPrincipal)
            .then((list) => { ownerVaults = list; })
            .catch(() => {});
        }
      }
    } catch (err) {
      console.error('[vault page] load error:', err);
      loadError = 'Failed to load vault data. The backend may be briefly unavailable.';
    } finally {
      loadingVault = false;
      loadingCore = false;
      loadingHistory = false;
    }
  }

  // Re-run on every vaultId change. SvelteKit reuses the component when
  // navigating between sibling /e/vault/[id] routes, so onMount() alone would
  // only fire on the first visit and leave the page stuck on the prior vault.
  $effect(() => {
    void vaultId;
    loadVault();
  });

  const statusLabel = $derived(isClosed ? 'Closed' : 'Active');
</script>

<svelte:head>
  <title>Vault #{vaultId} | Rumi Explorer</title>
</svelte:head>

<EntityShell
  title={`Vault #${vaultId}`}
  loading={loadingVault}
  error={loadError}
  onRetry={loadVault}
>
  {#snippet identity()}
    <div class="flex flex-wrap items-center gap-3">
      <StatusBadge status={isClosed ? 'Closed' : 'Active'} size="md" />
      <span class="inline-flex items-center gap-1.5 px-3 py-1 rounded-full bg-gray-800/60 border border-gray-700/50 text-sm">
        <EntityLink type="token" value={collateralPrincipalStr} label={tokenSymbol} />
      </span>
      <span class="text-xs text-gray-500">Owner</span>
      <EntityLink type="address" value={ownerStr} />
      <CopyButton text={ownerStr} />
    </div>

    <div class="flex flex-wrap items-center gap-x-6 gap-y-2 text-xs text-gray-400">
      {#if creationTimestamp}
        <div>Opened <TimeAgo timestamp={creationTimestamp} showFull={true} /></div>
      {/if}
      {#if interestRate !== null}
        <div>Rate <span class="text-gray-200">{formatPercent(interestRate)}</span></div>
      {/if}
      {#if isClosed && lastActivityTimestamp}
        <div>Closed <TimeAgo timestamp={lastActivityTimestamp} showFull={true} /></div>
      {/if}
    </div>

    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-6 flex flex-wrap items-center gap-8">
      <div class="flex-shrink-0">
        <CRDial {cr} liquidationCR={liquidationRatio} size="lg" />
      </div>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-x-8 gap-y-3 flex-1 min-w-[280px]">
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Collateral</div>
          <div class="text-lg font-mono text-white">{formatE8s(collateralAmount, decimals)} <span class="text-sm text-gray-400">{tokenSymbol}</span></div>
          {#if price > 0}<div class="text-xs text-gray-500">{formatUsdRaw(collateralValueUsd)}</div>{/if}
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Debt</div>
          <div class="text-lg font-mono text-white">{formatE8s(totalDebt, 8)} <span class="text-sm text-gray-400">icUSD</span></div>
          {#if accruedInterest > 0n}<div class="text-xs text-gray-500">incl. {formatE8s(accruedInterest, 8)} interest</div>{/if}
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Liq. Price</div>
          <div class="text-lg font-mono text-white">{liquidationPrice > 0 ? formatUsdRaw(liquidationPrice) : '--'}</div>
          {#if price > 0 && liquidationPrice > 0}<div class="text-xs text-gray-500">now {formatUsdRaw(price)}</div>{/if}
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Headroom</div>
          <div class="text-lg font-mono text-white">{headroomUsd > 0 ? formatUsdRaw(headroomUsd) : '--'}</div>
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Redeem Tier</div>
          <div class="text-lg font-mono text-white">{redemptionTier != null ? String(redemptionTier) : '--'}</div>
        </div>
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Status</div>
          <div class="text-lg font-mono {isClosed ? 'text-gray-400' : 'text-emerald-400'}">{statusLabel}</div>
        </div>
      </div>
    </div>
  {/snippet}

  {#snippet relationships()}
    <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
      <!-- Owner + other vaults -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Owner</div>
        <div class="flex items-center gap-1.5 text-sm">
          <EntityLink type="address" value={ownerStr} />
          <CopyButton text={ownerStr} />
        </div>
        {#if otherOwnerVaultsAnnotated.length > 0}
          <div class="text-[10px] uppercase tracking-wider text-gray-500 mt-3">Other Vaults ({otherOwnerVaultsAnnotated.length})</div>
          <div class="flex flex-wrap gap-2">
            {#each otherOwnerVaultsAnnotated.slice(0, 10) as ov (ov.id)}
              <a href="/explorer/e/vault/{ov.id}" class="inline-flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 font-mono pl-1 pr-2 py-0.5 rounded border border-gray-700/50 hover:border-gray-600">
                <CRDial cr={ov.cr} liquidationCR={ov.liq} size="sm" />
                <span>#{ov.id}</span>
              </a>
            {/each}
          </div>
        {:else}
          <div class="text-xs text-gray-500">No other vaults by this owner.</div>
        {/if}
      </div>

      <!-- Collateral token -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Collateral Token</div>
        <div class="text-sm"><EntityLink type="token" value={collateralPrincipalStr} label={tokenSymbol} /></div>
        {#if price > 0}<div class="text-xs text-gray-500">Price {formatUsdRaw(price)}</div>{/if}
        <div class="text-[10px] uppercase tracking-wider text-gray-500 mt-3">Debt Token</div>
        <div class="text-sm"><EntityLink type="token" value="t6bor-paaaa-aaaap-qrd5q-cai" label="icUSD" /></div>
      </div>

      <!-- Touched by -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Touched By</div>
        {#if touchedBy.length === 0}
          <div class="text-xs text-gray-500">Only the owner has touched this vault.</div>
        {:else}
          <ul class="space-y-1.5">
            {#each touchedBy as t (t.principal)}
              <li class="flex items-center gap-2 text-xs">
                <EntityLink type="address" value={t.principal} />
                <span class="text-gray-500">({t.roles.join(', ')})</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  {/snippet}

  {#snippet activity()}
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
      {#if loadingHistory}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">Loading history...</div>
      {:else if sortedHistory.length === 0}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">No events found for this vault.</div>
      {:else}
        <table class="w-full">
          <thead class="bg-gray-900/40">
            <tr class="text-[10px] uppercase tracking-wider text-gray-500">
              <th class="px-4 py-2 text-left">Event</th>
              <th class="px-4 py-2 text-left">When</th>
              <th class="px-4 py-2 text-left">By</th>
              <th class="px-4 py-2 text-left">Type</th>
              <th class="px-4 py-2 text-left">Summary</th>
              <th class="px-4 py-2 text-right"></th>
            </tr>
          </thead>
          <tbody>
            {#each sortedHistory as entry (entry.index)}
              <EventRow event={entry.event} index={Number(entry.index)} vaultCollateralMap={vaultCollateralMap} />
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/snippet}

  {#snippet analytics()}
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
      <div class="text-xs font-medium text-gray-400 mb-2">Collateral Ratio over time</div>
      <VaultCRChart points={timeline} liquidationCR={liquidationRatio} />
    </div>
    <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
        <MiniAreaChart
          points={debtPoints}
          label="Debt (icUSD)"
          color="#fbbf24"
          fillColor="rgba(251, 191, 36, 0.12)"
          valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 2 })}
        />
      </div>
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
        <MiniAreaChart
          points={collateralPoints}
          label={`Collateral (${tokenSymbol})`}
          color="#34d399"
          fillColor="rgba(52, 211, 153, 0.12)"
          valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 4 })}
        />
      </div>
    </div>
  {/snippet}
</EntityShell>
