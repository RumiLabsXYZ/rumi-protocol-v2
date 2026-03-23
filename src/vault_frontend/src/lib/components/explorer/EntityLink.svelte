<script lang="ts">
  import { truncatePrincipal } from '$lib/utils/principalHelpers';

  type EntityType = 'vault' | 'address' | 'token' | 'canister' | 'event';

  interface Props {
    type: EntityType;
    id: string | number;
    label?: string;
  }

  let { type, id, label }: Props = $props();

  const routes: Record<EntityType, string> = {
    vault: '/explorer/vault/',
    address: '/explorer/address/',
    token: '/explorer/token/',
    canister: '/explorer/canister/',
    event: '/explorer/event/',
  };

  const href = `${routes[type]}${id}`;

  const displayLabel = $derived(
    label ??
    (type === 'address' || type === 'canister' || type === 'token'
      ? truncatePrincipal(String(id))
      : String(id))
  );
</script>

<a {href} class="text-blue-400 hover:text-blue-300 hover:underline font-mono text-sm transition-colors">
  {displayLabel}
</a>
