<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { fade, scale } from 'svelte/transition';
  
  export let title: string = '';
  export let onClose: () => void;
  export let showClose: boolean = true;
  export let closeOnBackdrop: boolean = true;
  export let closeOnEscape: boolean = true;
  export let maxWidth: string = '28rem';
  
  let portalTarget: HTMLDivElement;
  let backdropEl: HTMLDivElement;
  
  function handleKeydown(event: KeyboardEvent) {
    if (closeOnEscape && event.key === 'Escape') {
      onClose();
    }
  }
  
  function handleBackdropClick(event: MouseEvent) {
    if (closeOnBackdrop && event.target === backdropEl) {
      onClose();
    }
  }
  
  onMount(async () => {
    // Portal: move modal to document.body to escape stacking contexts
    document.body.appendChild(portalTarget);
    document.addEventListener('keydown', handleKeydown);
    document.body.style.overflow = 'hidden';
    await tick();
  });
  
  onDestroy(() => {
    document.removeEventListener('keydown', handleKeydown);
    document.body.style.overflow = '';
    // Clean up portal element
    if (portalTarget && portalTarget.parentNode) {
      portalTarget.parentNode.removeChild(portalTarget);
    }
  });
</script>

<div bind:this={portalTarget} class="modal-portal-root">
  <div 
    class="modal-backdrop" 
    bind:this={backdropEl}
    on:click={handleBackdropClick}
    role="dialog"
    aria-modal="true"
    aria-labelledby={title ? 'modal-title' : undefined}
  >
    <div 
      class="modal-content"
      style="--max-width: {maxWidth}"
    >
      {#if title || showClose}
        <div class="modal-header">
          {#if title}
            <h2 id="modal-title" class="modal-title">{title}</h2>
          {/if}
          {#if showClose}
            <button 
              class="modal-close" 
              on:click={onClose}
              aria-label="Close modal"
            >
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M18 6L6 18M6 6l12 12" />
              </svg>
            </button>
          {/if}
        </div>
      {/if}
      
      <div class="modal-body">
        <slot />
      </div>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9000;
    padding: 1rem;
  }
  
  .modal-content {
    background: linear-gradient(145deg, #1a1a2e 0%, #16213e 100%);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 0.75rem;
    width: 100%;
    max-width: var(--max-width);
    max-height: 90vh;
    overflow-y: auto;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  }
  
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  }
  
  .modal-title {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 600;
    color: white;
  }
  
  .modal-close {
    background: transparent;
    border: none;
    color: rgba(255, 255, 255, 0.6);
    cursor: pointer;
    padding: 0.25rem;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 0.25rem;
    transition: all 0.15s ease;
  }
  
  .modal-close:hover {
    color: white;
    background: rgba(255, 255, 255, 0.1);
  }
  
  .modal-body {
    padding: 1.25rem;
  }
</style>
