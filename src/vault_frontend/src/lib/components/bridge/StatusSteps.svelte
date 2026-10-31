<script lang="ts">
  interface Props {
    steps: string[];
    /** Index of the step in progress. A value equal to steps.length means every step is complete. */
    active: number;
    /** Mark the active step as needing attention (held, failed). */
    attention?: boolean;
    label: string;
  }

  let { steps, active, attention = false, label }: Props = $props();
</script>

<ol class="steps" aria-label={label}>
  {#each steps as step, index (step)}
    <li class:done={index < active} class:current={index === active} class:attention={attention && index === active} aria-current={index === active ? 'step' : undefined}>
      <span class="dot" aria-hidden="true">{index < active ? '✓' : index + 1}</span>
      <span class="name">{step}</span>
    </li>
  {/each}
</ol>

<style>
  .steps { display:flex; flex-wrap:wrap; gap:0.5rem 0.9rem; margin:0.6rem 0; padding:0; list-style:none; font-size:0.75rem; color:var(--rumi-text-muted); }
  li { display:flex; align-items:center; gap:0.4rem; }
  .dot { display:grid; place-items:center; width:1.35rem; height:1.35rem; border-radius:50%; border:1px solid var(--rumi-border-hover); font-size:0.65rem; font-variant-numeric:tabular-nums; }
  .done { color:var(--rumi-text-secondary); }
  .done .dot { background:var(--rumi-action-dim); border-color:var(--rumi-border-accent); color:var(--rumi-action-bright); }
  .current { color:var(--rumi-text-primary); font-weight:600; }
  .current .dot { border-color:var(--rumi-action); color:var(--rumi-action-bright); }
  .attention { color:#ffd9a8; }
  .attention .dot { border-color:#e5a85a; color:#e5a85a; }
</style>
