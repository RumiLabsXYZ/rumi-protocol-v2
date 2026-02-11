<script lang="ts">
  import { page } from '$app/stores';
  $: isSubPage = $page.url.pathname !== '/docs';
</script>

{#if isSubPage}
  <div class="docs-back">
    <a href="/docs" class="docs-back-link">← All Docs</a>
  </div>
{/if}

<slot />

<style>
  .docs-back { max-width: 700px; margin: 0 auto 0.75rem; }
  .docs-back-link {
    font-size: 0.8125rem; color: var(--rumi-text-muted);
    text-decoration: none; transition: color 0.15s ease;
  }
  .docs-back-link:hover { color: var(--rumi-text-primary); }
</style>
