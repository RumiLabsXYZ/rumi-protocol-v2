<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';

  interface Column {
    key: string;
    label: string;
    sortable?: boolean;
    align?: 'left' | 'center' | 'right';
    width?: string;
  }

  interface Props {
    columns: Column[];
    data: T[];
    loading?: boolean;
    emptyMessage?: string;
    rowKey?: (item: T) => string;
    row: Snippet<[T, number]>;
    compact?: boolean;
  }

  let {
    columns,
    data,
    loading = false,
    emptyMessage = 'No data found',
    rowKey,
    row,
    compact = false,
  }: Props = $props();

  let sortKey = $state<string | null>(null);
  let sortDirection = $state<'asc' | 'desc'>('asc');

  function handleSort(key: string) {
    const col = columns.find(c => c.key === key);
    if (!col?.sortable) return;
    if (sortKey === key) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      sortKey = key;
      sortDirection = 'asc';
    }
  }

  const sortedData = $derived.by(() => {
    if (!sortKey) return data;
    const col = columns.find(c => c.key === sortKey);
    if (!col?.sortable) return data;
    const dir = sortDirection === 'asc' ? 1 : -1;
    return [...data].sort((a: any, b: any) => {
      const aVal = a[sortKey!];
      const bVal = b[sortKey!];
      if (aVal == null && bVal == null) return 0;
      if (aVal == null) return 1;
      if (bVal == null) return -1;
      if (typeof aVal === 'number' && typeof bVal === 'number') return (aVal - bVal) * dir;
      if (typeof aVal === 'bigint' && typeof bVal === 'bigint') return aVal < bVal ? -dir : aVal > bVal ? dir : 0;
      return String(aVal).localeCompare(String(bVal)) * dir;
    });
  });

  const cellPadding = $derived(compact ? 'px-3 py-1.5' : 'px-4 py-3');
  const headerPadding = $derived(compact ? 'px-3 py-2' : 'px-4 py-3');
</script>

<div class="overflow-x-auto rounded-xl bg-gray-800/30 border border-gray-700/50">
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b border-gray-700/50">
        {#each columns as col}
          <th
            class="{headerPadding} text-xs font-medium text-gray-400 uppercase tracking-wider
                   {col.align === 'right' ? 'text-right' : col.align === 'center' ? 'text-center' : 'text-left'}
                   {col.sortable ? 'cursor-pointer hover:text-gray-200 select-none transition-colors' : ''}"
            style={col.width ? `width: ${col.width}` : ''}
            onclick={() => handleSort(col.key)}
          >
            <span class="inline-flex items-center gap-1">
              {col.label}
              {#if col.sortable && sortKey === col.key}
                <span class="text-blue-400">{sortDirection === 'asc' ? '↑' : '↓'}</span>
              {/if}
            </span>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr>
          <td colspan={columns.length} class="{cellPadding} py-16 text-center">
            <div class="flex flex-col items-center gap-3">
              <div class="w-7 h-7 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
              <span class="text-sm text-gray-500">Loading...</span>
            </div>
          </td>
        </tr>
      {:else if sortedData.length === 0}
        <tr>
          <td colspan={columns.length} class="{cellPadding} py-16 text-center text-gray-500">
            {emptyMessage}
          </td>
        </tr>
      {:else}
        {#each sortedData as item, i (rowKey ? rowKey(item) : i)}
          {@render row(item, i)}
        {/each}
      {/if}
    </tbody>
  </table>
</div>
