<script lang="ts">
  export interface HealthMetric {
    label: string;
    value: string;
    sub?: string;
    tone?: 'normal' | 'good' | 'caution' | 'danger' | 'muted';
  }

  interface Props {
    title: string;
    metrics: HealthMetric[];
    loading?: boolean;
  }
  let { title, metrics, loading = false }: Props = $props();

  const toneClass: Record<NonNullable<HealthMetric['tone']>, string> = {
    normal: '',
    good: 'text-teal-400',
    caution: 'text-amber-400',
    danger: 'text-pink-400',
    muted: 'text-gray-500',
  };
</script>

<div class="explorer-card">
  <div class="text-[11px] uppercase tracking-[0.14em] font-medium mb-3" style="color: var(--rumi-text-secondary);">
    {title}
  </div>
  {#if loading}
    <div class="flex items-center py-1">
      <div class="w-4 h-4 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else}
    <div class="flex flex-wrap items-baseline gap-x-8 gap-y-3">
      {#each metrics as m}
        <div class="flex flex-col min-w-0">
          <span class="text-xs text-gray-500">{m.label}</span>
          <span class="text-base font-semibold tabular-nums mt-0.5 {toneClass[m.tone ?? 'normal']}">
            {m.value}
          </span>
          {#if m.sub}
            <!-- Cap sub width so a long descriptor (e.g. the Admin lens
                 "Collector errors" explanation) wraps within the metric
                 column instead of expanding the column and squeezing
                 siblings out of the row. -->
            <span class="text-[11px] text-gray-500 mt-0.5 max-w-[14rem] leading-snug">{m.sub}</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
