<script lang="ts">
  import type { Writable } from 'svelte/store';
  import VaultCard from '../../lib/components/vault/VaultCard.svelte';
  export let vaultInput: Writable<any>;
</script>
<VaultCard vault={$vaultInput} icpPrice={10} expandedVaultId={33} />
