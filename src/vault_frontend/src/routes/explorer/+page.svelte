<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import LensTabs from '$components/explorer/LensTabs.svelte';
  import type { LensId } from '$components/explorer/lenses/lensTypes';
  import OverviewLens from '$components/explorer/lenses/OverviewLens.svelte';
  import CollateralLens from '$components/explorer/lenses/CollateralLens.svelte';
  import StabilityPoolLens from '$components/explorer/lenses/StabilityPoolLens.svelte';
  import RedemptionsLens from '$components/explorer/lenses/RedemptionsLens.svelte';
  import RevenueLens from '$components/explorer/lenses/RevenueLens.svelte';
  import DexsLens from '$components/explorer/lenses/DexsLens.svelte';
  import AdminLens from '$components/explorer/lenses/AdminLens.svelte';

  const VALID_LENSES: LensId[] = ['overview', 'collateral', 'stability', 'redemptions', 'revenue', 'dexs', 'admin'];

  const lens = $derived.by<LensId>(() => {
    const raw = $page.url.searchParams.get('lens') ?? 'overview';
    return (VALID_LENSES as string[]).includes(raw) ? (raw as LensId) : 'overview';
  });

  function setLens(l: LensId) {
    const url = new URL($page.url);
    if (l === 'overview') url.searchParams.delete('lens');
    else url.searchParams.set('lens', l);
    goto(url.pathname + (url.search || ''), { keepFocus: true, noScroll: true, replaceState: false });
  }

  const LENS_TITLES: Record<LensId, string> = {
    overview: 'Overview',
    collateral: 'Collateral',
    stability: 'Stability Pool',
    redemptions: 'Redemptions',
    revenue: 'Revenue',
    dexs: 'DEXs',
    admin: 'Admin',
  };
</script>

<svelte:head>
  <title>Protocol / {LENS_TITLES[lens]} | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <LensTabs active={lens} {setLens} />

  {#if lens === 'overview'}
    <OverviewLens />
  {:else if lens === 'collateral'}
    <CollateralLens />
  {:else if lens === 'stability'}
    <StabilityPoolLens />
  {:else if lens === 'redemptions'}
    <RedemptionsLens />
  {:else if lens === 'revenue'}
    <RevenueLens />
  {:else if lens === 'dexs'}
    <DexsLens />
  {:else if lens === 'admin'}
    <AdminLens />
  {/if}
</div>
