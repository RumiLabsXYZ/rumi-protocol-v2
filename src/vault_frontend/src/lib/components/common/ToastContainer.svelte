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
    top: 5rem;
    right: 1.5rem;
    z-index: 10000;
    pointer-events: none;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  
  .toast-container :global(.toast) {
    pointer-events: auto;
    position: relative;
    margin-bottom: 0.5rem;
  }
</style>
