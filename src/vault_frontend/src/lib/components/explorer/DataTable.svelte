<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';

  interface Column {
    key: string;
    label: string;
    sortable?: boolean;
    align?: 'left' | 'right' | 'center';
    width?: string;
  }

  interface Props {
    columns: Column[];
    rows: T[];
    sortKey?: string;
    sortDirection?: 'asc' | 'desc';
    onSort?: (key: string) => void;
    row: Snippet<[T, number]>;
    emptyMessage?: string;
    loading?: boolean;
  }

  let {
    columns,
    rows,
    sortKey,
    sortDirection = 'desc',
    onSort,
    row,
    emptyMessage = 'No data',
    loading = false,
  }: Props = $props();

  function handleSort(key: string) {
    const col = columns.find(c => c.key === key);
    if (col?.sortable && onSort) onSort(key);
  }
</script>

<div class="overflow-x-auto">
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b border-gray-700/50">
        {#each columns as col}
          <th
            class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider
                   {col.align === 'right' ? 'text-right' : col.align === 'center' ? 'text-center' : 'text-left'}
                   {col.sortable ? 'cursor-pointer hover:text-gray-200 select-none' : ''}"
            style={col.width ? `width: ${col.width}` : ''}
            onclick={() => handleSort(col.key)}
          >
            {col.label}
            {#if col.sortable && sortKey === col.key}
              <span class="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
            {/if}
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr>
          <td colspan={columns.length} class="px-4 py-12 text-center text-gray-500">
            Loading...
          </td>
        </tr>
      {:else if rows.length === 0}
        <tr>
          <td colspan={columns.length} class="px-4 py-12 text-center text-gray-500">
            {emptyMessage}
          </td>
        </tr>
      {:else}
        {#each rows as item, i}
          {@render row(item, i)}
        {/each}
      {/if}
    </tbody>
  </table>
</div>
