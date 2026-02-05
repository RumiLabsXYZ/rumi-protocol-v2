<script lang="ts">
  import { toastStore } from '../../stores/toast';
  import Toast from './Toast.svelte';
</script>

<div class="toast-container">
  {#each $toastStore as toast (toast.id)}
    <Toast 
      message={toast.message}
      type={toast.type}
      duration={toast.duration}
      onClose={() => toastStore.remove(toast.id)}
    />
  {/each}
</div>

<style>
  .toast-container {
    position: fixed;
    bottom: 0;
    right: 0;
    z-index: 10000;
    pointer-events: none;
  }
  
  .toast-container :global(.toast) {
    pointer-events: auto;
    position: relative;
    margin-bottom: 0.5rem;
  }
</style>
