<script lang="ts">
  interface Props {
    mode: string;
    frozen?: boolean;
  }
  let { mode, frozen = false }: Props = $props();

  const config = $derived.by(() => {
    if (frozen) return { label: 'Frozen', color: 'bg-pink-500/20 text-pink-300 border-pink-500/30' };
    switch (mode) {
      case 'ReadOnly':
        return { label: 'Read-Only', color: 'bg-amber-500/20 text-amber-300 border-amber-500/30' };
      case 'Recovery':
        return { label: 'Recovery', color: 'bg-violet-500/20 text-violet-300 border-violet-500/30' };
      default:
        return { label: 'Normal', color: 'bg-teal-500/15 text-teal-300 border-teal-500/25' };
    }
  });
</script>

<span class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded-full border {config.color}">
  <span class="w-1.5 h-1.5 rounded-full {frozen ? 'bg-pink-400' : mode === 'Recovery' ? 'bg-violet-400' : mode === 'ReadOnly' ? 'bg-amber-400' : 'bg-teal-400'}"></span>
  {config.label}
</span>
