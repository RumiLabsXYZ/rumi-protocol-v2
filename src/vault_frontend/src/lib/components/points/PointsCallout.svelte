<script lang="ts">
  /**
   * PointsCallout — the inline green callout shown on deposit forms to tell the
   * user what multiplier their current inputs earn, plus an optional hint (e.g.
   * the 3USD-holding rule or a nudge toward a higher tier).
   */
  interface Props {
    headline: string;
    hint?: string;
    tone?: 'earning' | 'info' | 'warning';
  }
  let { headline, hint, tone = 'earning' }: Props = $props();

  const styles: Record<string, string> = {
    earning: 'bg-emerald-400/10 border-emerald-400/30 text-emerald-300',
    info: 'bg-gray-500/10 border-gray-500/25 text-gray-200',
    warning: 'bg-amber-400/10 border-amber-400/30 text-amber-200',
  };
</script>

<div class="flex items-start gap-2.5 rounded-lg border px-3 py-2.5 {styles[tone]}">
  <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true" style="flex-shrink:0;margin-top:1px">
    <path d="M13 2L4.5 13.5H11l-1 8.5L19.5 10H13z" />
  </svg>
  <div class="min-w-0">
    <p class="text-sm font-medium leading-tight">{headline}</p>
    {#if hint}<p class="text-xs text-gray-400 mt-0.5 leading-snug">{hint}</p>{/if}
  </div>
</div>
