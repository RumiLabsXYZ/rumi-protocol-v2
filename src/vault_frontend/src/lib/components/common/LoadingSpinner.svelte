<script lang="ts">
  export let size: 'small' | 'medium' | 'large' = 'medium';
  export let color: string = 'var(--primary-accent, #29abe2)';
  
  const sizes = {
    small: '1rem',
    medium: '2rem',
    large: '3rem'
  };
</script>

<div 
  class="spinner" 
  style="--size: {sizes[size]}; --color: {color}"
  role="status"
  aria-label="Loading"
>
  <div class="spinner-ring"></div>
</div>

<style>
  .spinner {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--size);
    height: var(--size);
  }

  .spinner-ring {
    width: 100%;
    height: 100%;
    border: 3px solid rgba(255, 255, 255, 0.1);
    border-top-color: var(--color);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
