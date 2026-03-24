<script lang="ts">
  interface Props {
    status: string;
    size?: 'sm' | 'md';
  }

  let { status, size = 'sm' }: Props = $props();

  const paddings: Record<string, string> = { sm: 'px-2 py-0.5 text-xs', md: 'px-3 py-1 text-sm' };
  let padding = $derived(paddings[size]);

  let colorClasses = $derived.by(() => {
    const s = status.toLowerCase();
    if (['normal', 'active', 'healthy'].includes(s)) {
      return 'bg-emerald-500/20 text-emerald-300 border-emerald-500/30';
    }
    if (['recovery', 'caution', 'paused'].includes(s)) {
      return 'bg-yellow-500/20 text-yellow-300 border-yellow-500/30';
    }
    if (['frozen', 'danger', 'liquidatable'].includes(s)) {
      return 'bg-red-500/20 text-red-300 border-red-500/30';
    }
    return 'bg-gray-500/20 text-gray-300 border-gray-500/30';
  });
</script>

<span class="inline-flex items-center rounded-full border font-medium {padding} {colorClasses}">
  {status}
</span>
