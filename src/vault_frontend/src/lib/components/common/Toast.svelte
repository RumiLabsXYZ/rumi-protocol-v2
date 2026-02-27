<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  
  export let message: string;
  export let duration: number = 3500;
  export let type: 'success' | 'error' | 'info' = 'success';
  export let onClose: () => void = () => {};
  
  let visible = true;
  let timeoutId: ReturnType<typeof setTimeout>;
  
  const typeStyles = {
    success: {
      bg: 'rgba(45, 212, 191, 0.95)',
      border: '#2DD4BF',
      icon: '✓'
    },
    error: {
      bg: 'rgba(224, 107, 159, 0.95)',
      border: '#e06b9f',
      icon: '✕'
    },
    info: {
      bg: 'rgba(59, 130, 246, 0.95)',
      border: '#3b82f6',
      icon: 'ℹ'
    }
  };
  
  $: style = typeStyles[type];
  
  onMount(() => {
    timeoutId = setTimeout(() => {
      visible = false;
      setTimeout(onClose, 300); // Wait for exit animation
    }, duration);
  });
  
  onDestroy(() => {
    if (timeoutId) clearTimeout(timeoutId);
  });
  
  function handleClose() {
    visible = false;
    setTimeout(onClose, 300);
  }
</script>

{#if visible}
  <div 
    class="toast"
    style="--bg: {style.bg}; --border: {style.border}"
    transition:fly={{ y: -20, duration: 250 }}
    role="alert"
    aria-live="polite"
  >
    <span class="toast-icon">{style.icon}</span>
    <span class="toast-message">{message}</span>
    <button class="toast-close" on:click={handleClose} aria-label="Close">
      ✕
    </button>
  </div>
{/if}

<style>
  .toast {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.875rem 1rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    max-width: 340px;
    width: 100%;
    backdrop-filter: blur(8px);
  }
  
  .toast-icon {
    font-size: 1rem;
    font-weight: bold;
    flex-shrink: 0;
  }
  
  .toast-message {
    color: white;
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.4;
  }
  
  .toast-close {
    background: transparent;
    border: none;
    color: rgba(255, 255, 255, 0.7);
    cursor: pointer;
    padding: 0.25rem;
    font-size: 0.75rem;
    line-height: 1;
    margin-left: auto;
    transition: color 0.15s ease;
  }
  
  .toast-close:hover {
    color: white;
  }
</style>
