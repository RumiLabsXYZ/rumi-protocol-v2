<script lang="ts">
  /**
   * MultiplierBadge — the green airdrop multiplier pill used across every
   * point-earning surface. Reads nothing; the caller supplies the multiplier from
   * pointsRules so the badge can never disagree with the canister.
   */
  interface Props {
    multiplier: number;
    variant?: 'compact' | 'full';
    size?: 'sm' | 'md';
    comingSoon?: boolean;
    /** Override the text after the icon (e.g. "up to 5× points"). */
    label?: string;
  }
  let { multiplier, variant = 'compact', size = 'sm', comingSoon = false, label }: Props = $props();

  const pad = $derived(size === 'md' ? 'px-2.5 py-1 text-sm' : 'px-2 py-0.5 text-xs');
  const tone = $derived(
    comingSoon
      ? 'bg-gray-500/15 text-gray-400 border-gray-500/25'
      : 'bg-emerald-400/15 text-emerald-300 border-emerald-400/35',
  );
  const text = $derived(
    label ??
      (variant === 'full'
        ? comingSoon
          ? `${multiplier}× points soon`
          : `Earn ${multiplier}× points`
        : `${multiplier}×`),
  );
</script>

<span
  class="inline-flex items-center gap-1 rounded-full border font-medium whitespace-nowrap tabular-nums {pad} {tone}"
  title={comingSoon ? 'Coming soon' : `Earns ${multiplier}× airdrop points`}
>
  {#if !comingSoon}
    <svg viewBox="0 0 24 24" width="11" height="11" fill="currentColor" aria-hidden="true" style="flex-shrink:0">
      <path d="M13 2L4.5 13.5H11l-1 8.5L19.5 10H13z" />
    </svg>
  {/if}
  {text}
</span>
