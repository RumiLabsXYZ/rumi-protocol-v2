<script lang="ts">
  import { onMount } from 'svelte';

  let copiedColor = '';

  function copyToClipboard(text: string, label: string) {
    navigator.clipboard.writeText(text);
    copiedColor = label;
    setTimeout(() => copiedColor = '', 1500);
  }

  function downloadAsset(filename: string) {
    const link = document.createElement('a');
    link.href = `/${filename}`;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }
</script>

<section class="max-w-5xl mx-auto px-6 py-16 md:py-24">

  <!-- Header -->
  <p class="text-sm font-medium tracking-widest uppercase mb-4"
     style="color: var(--rumi-purple-accent);">
    Brand Guidelines
  </p>
  <h1 class="text-3xl md:text-4xl font-bold mb-3"
      style="font-family: 'Circular Std', 'Inter', sans-serif; color: var(--rumi-text-primary); letter-spacing: -0.02em;">
    Rumi Labs Brand Kit
  </h1>
  <p class="text-sm mb-12" style="color: var(--rumi-text-secondary); max-width: 560px; line-height: 1.7;">
    Official logos, colors, and usage guidelines for Rumi Protocol and Rumi Labs.
    For questions, reach out to <a href="mailto:info@rumiprotocol.com" style="color: var(--rumi-action); text-decoration: none;">info@rumiprotocol.com</a>.
  </p>

  <!-- ═══════════ LOGO ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Logo</h2>
  <p class="section-desc mb-8">
    The Rumi mark is a geometric pattern of isometric triangles in two brand colors.
    Always use the provided assets — do not recreate or modify.
  </p>

  <div class="logo-showcase mb-6">
    <div class="logo-showcase-dark">
      <img src="/rumilogo-vector-v2_NEW_inset2.svg" alt="Rumi Logo" class="logo-large" />
    </div>
    <div class="logo-showcase-light">
      <img src="/rumilogo-vector-v2_NEW_inset2.svg" alt="Rumi Logo on light" class="logo-large" />
    </div>
  </div>
  <p class="text-xs mb-8" style="color: var(--rumi-text-muted);">
    Left: primary usage on dark backgrounds. Right: on light backgrounds (logo works on both).
  </p>

  <!-- Logo variants -->
  <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 mb-8">
    <div class="asset-card">
      <div class="asset-preview">
        <img src="/rumilogo-vector-v2_inset2.png" alt="Inset logo" class="logo-thumb" />
      </div>
      <div class="asset-info">
        <span class="asset-label">Inset / Default</span>
        <span class="asset-note">Navbar, favicon, small contexts. Wider gaps for legibility ≤64px.</span>
        <div class="asset-btn-row">
          <button class="asset-download" on:click={() => downloadAsset('rumilogo-vector-v2_NEW_inset2.svg')}>SVG</button>
          <button class="asset-download" on:click={() => downloadAsset('rumilogo-vector-v2_inset2.png')}>PNG</button>
        </div>
      </div>
    </div>

    <div class="asset-card">
      <div class="asset-preview">
        <img src="/rumilogo-vector-v2_NEW.png" alt="Standard logo" class="logo-thumb" />
      </div>
      <div class="asset-info">
        <span class="asset-label">Standard / Tight</span>
        <span class="asset-note">Tighter triangle gaps. Best at large sizes only (hero, print, 200px+).</span>
        <div class="asset-btn-row">
          <button class="asset-download" on:click={() => downloadAsset('rumilogo-vector-v2_NEW.svg')}>SVG</button>
          <button class="asset-download" on:click={() => downloadAsset('rumilogo-vector-v2_NEW.png')}>PNG</button>
        </div>
      </div>
    </div>

    <div class="asset-card">
      <div class="asset-preview" style="background: #1e2854;">
        <img src="/rumilogo-twitter.png" alt="Twitter logo" class="logo-thumb" />
      </div>
      <div class="asset-info">
        <span class="asset-label">Social / Twitter</span>
        <span class="asset-note">Logo on #1e2854 background. Profile pics, social cards.</span>
        <button class="asset-download" on:click={() => downloadAsset('rumilogo-twitter.png')}>PNG</button>
      </div>
    </div>

    <div class="asset-card">
      <div class="asset-preview" style="background: #1a1a2e;">
        <img src="/rumilogo-twitter2.png" alt="Twitter logo variant 2" class="logo-thumb" />
      </div>
      <div class="asset-info">
        <span class="asset-label">Social / Twitter (Alt)</span>
        <span class="asset-note">Alternate social variant on #1a1a2e.</span>
        <button class="asset-download" on:click={() => downloadAsset('rumilogo-twitter2.png')}>PNG</button>
      </div>
    </div>
  </div>

  <p class="text-xs mb-8" style="color: var(--rumi-text-muted);">
    <strong style="color: var(--rumi-text-secondary);">Which logo to use:</strong> Default to the Inset version — the wider gaps between triangles keep the geometric pattern readable at any size.
    The Standard/Tight version is only for large hero contexts (200px+) where the fine detail is visible.
  </p>

  <!-- Logo + Wordmark -->
  <div class="logo-wordmark-row mb-4">
    <img src="/rumilogo-vector-v2_inset2.png" alt="Rumi" class="w-10 h-10" />
    <span class="brand-wordmark-demo">RUMI</span>
  </div>
  <p class="text-xs mb-8" style="color: var(--rumi-text-muted);">
    Wordmark uses Circular Std Medium, letter-spacing 0.12em, color <code>#e8e4f0</code> (text-primary).
  </p>

  <!-- ═══════════ LOGO COLORS ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Logo Colors</h2>
  <p class="section-desc mb-6">The two brand colors used in the Rumi mark.</p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Rumi Green', hex: '#2DE2A6', var: 'Logo green' },
      { name: 'Rumi Pink', hex: '#F46AAE', var: 'Logo pink' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role">{color.var}</span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ BACKGROUND SYSTEM ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Background System</h2>
  <p class="section-desc mb-6">
    Purple is the base material of the interface.
    No flat black, no gray, no warmth.
  </p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Page Background', hex: '#080b16', var: '--rumi-bg-primary' },
      { name: 'Surface 1', hex: '#0e1222', var: '--rumi-bg-surface1' },
      { name: 'Surface 2', hex: '#141a2e', var: '--rumi-bg-surface2' },
      { name: 'Surface 3', hex: '#1a2139', var: '--rumi-bg-surface3' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex}; border: 1px solid rgba(255,255,255,0.08);"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role"><code>{color.var}</code></span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ PURPLE SYSTEM ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Purple System</h2>
  <p class="section-desc mb-6">
    Structural accents and identity. The purple-accent color is used for titles and section headings
    in the secondary brand (marketing site). It is not used in transactional UI.
  </p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Purple Deep', hex: '#111635', var: '--rumi-purple-deep' },
      { name: 'Purple Muted', hex: '#1e2854', var: '--rumi-purple-muted' },
      { name: 'Purple Base', hex: '#3b3d85', var: '--rumi-purple-base' },
      { name: 'Purple Light', hex: '#7c3aed', var: '--rumi-purple-light' },
      { name: 'Purple Accent', hex: '#d176e8', var: '--rumi-purple-accent' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role"><code>{color.var}</code></span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ ACTION / EMERALD ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Emerald — Action</h2>
  <p class="section-desc mb-6">
    The action color. Buttons, CTAs, active states. Dark text on emerald fills.
  </p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Action', hex: '#34d399', var: '--rumi-action' },
      { name: 'Action Bright', hex: '#6ee7b7', var: '--rumi-action-bright' },
      { name: 'Action Muted', hex: '#10b981', var: '--rumi-action-muted' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role"><code>{color.var}</code></span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ TEAL ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Teal — Quiet Accent</h2>
  <p class="section-desc mb-6">Used sparingly for secondary interactive elements and subtle highlights.</p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Teal', hex: '#2DD4BF', var: '--rumi-teal' },
      { name: 'Teal Bright', hex: '#5eead4', var: '--rumi-teal-bright' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role"><code>{color.var}</code></span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ RISK & STATUS ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Risk & Status</h2>
  <p class="section-desc mb-6">Semantic colors for collateral ratio indicators and system alerts.</p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Safe', hex: '#34d399', var: 'CR ≥ 200%' },
      { name: 'Caution', hex: '#fbbf24', var: 'CR 150–200%' },
      { name: 'Danger', hex: '#f87171', var: 'CR 130–150%' },
      { name: 'Critical', hex: '#ef4444', var: 'CR < 130%' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role">{color.var}</span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <!-- ═══════════ TEXT & BORDERS ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Text & Borders</h2>
  <p class="section-desc mb-6">Three-tier text hierarchy and border system.</p>

  <div class="color-grid mb-10">
    {#each [
      { name: 'Text Primary', hex: '#e8e4f0', var: '--rumi-text-primary' },
      { name: 'Text Secondary', hex: '#a09bb5', var: '--rumi-text-secondary' },
      { name: 'Text Muted', hex: '#605a75', var: '--rumi-text-muted' },
    ] as color}
      <button class="color-card" on:click={() => copyToClipboard(color.hex, color.var)}>
        <div class="color-swatch" style="background: {color.hex};"></div>
        <div class="color-meta">
          <span class="color-name">{color.name}</span>
          <span class="color-hex">{color.hex}</span>
          <span class="color-role"><code>{color.var}</code></span>
          {#if copiedColor === color.var}<span class="copied-badge">Copied!</span>{/if}
        </div>
      </button>
    {/each}
  </div>

  <div class="border-samples mb-10">
    <div class="border-sample" style="border-color: rgba(90, 100, 180, 0.12);">
      <span class="text-xs" style="color: var(--rumi-text-muted);">Border Default — <code>rgba(90, 100, 180, 0.12)</code></span>
    </div>
    <div class="border-sample" style="border-color: rgba(90, 100, 180, 0.25);">
      <span class="text-xs" style="color: var(--rumi-text-muted);">Border Hover — <code>rgba(90, 100, 180, 0.25)</code></span>
    </div>
  </div>

  <!-- ═══════════ TYPOGRAPHY ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Typography</h2>
  <p class="section-desc mb-6">Two typefaces. No exceptions.</p>

  <div class="type-samples mb-10">
    <div class="type-sample">
      <span class="type-name">Circular Std Medium</span>
      <span class="type-use">Headlines, page titles, wordmark, CTAs</span>
      <p class="type-demo" style="font-family: 'Circular Std', 'Inter', sans-serif; font-size: 1.5rem; font-weight: 500; color: var(--rumi-text-primary);">
        Borrow stable. Stay long ICP.
      </p>
    </div>
    <div class="type-sample">
      <span class="type-name">Inter</span>
      <span class="type-use">Body text, labels, numbers, UI elements</span>
      <p class="type-demo" style="font-family: 'Inter', sans-serif; font-size: 0.9375rem; color: var(--rumi-text-secondary); line-height: 1.7;">
        Users deposit ICP into personal vaults and mint icUSD stablecoins against that collateral.
        Vaults must maintain a minimum 133% collateral ratio.
      </p>
    </div>
  </div>

  <!-- ═══════════ GRADIENTS ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Gradients</h2>
  <p class="section-desc mb-6">
    Gradients are identity elements, not heading styles. Use only in designated contexts.
  </p>

  <div class="gradient-samples mb-10">
    <div class="gradient-sample">
      <div class="gradient-preview" style="background: linear-gradient(135deg, #d176e8, #34d399);"></div>
      <div class="gradient-info">
        <span class="gradient-name">Identity Gradient</span>
        <span class="gradient-code"><code>linear-gradient(135deg, #d176e8, #34d399)</code></span>
        <span class="gradient-use">RUMI wordmark only. Not for headings.</span>
      </div>
    </div>
    <div class="gradient-sample">
      <div class="gradient-preview" style="background: linear-gradient(135deg, var(--rumi-text-primary) 30%, var(--rumi-purple-accent) 70%, var(--rumi-action) 100%);"></div>
      <div class="gradient-info">
        <span class="gradient-name">Hero Headline</span>
        <span class="gradient-code"><code>linear-gradient(135deg, #e8e4f0 30%, #d176e8 70%, #34d399 100%)</code></span>
        <span class="gradient-use">Marketing hero headlines only (secondary brand).</span>
      </div>
    </div>
  </div>

  <!-- ═══════════ USAGE RULES ═══════════ -->
  <div class="section-divider"></div>
  <h2 class="section-heading">Usage Rules</h2>
  <p class="section-desc mb-6">Quick reference for consistent brand application.</p>

  <div class="grid grid-cols-1 md:grid-cols-2 gap-5 mb-10">
    <div class="rule-card rule-do">
      <h3 class="rule-title">DO</h3>
      <ul class="rule-list">
        <li>Use Inset logo at small sizes (≤64px)</li>
        <li>Use dark backgrounds from the background system</li>
        <li>Use emerald for interactive/action elements</li>
        <li>Use Circular Std for headings, Inter for body</li>
        <li>Use purple-accent for marketing section headings</li>
        <li>Keep plenty of whitespace around the logo</li>
      </ul>
    </div>
    <div class="rule-card rule-dont">
      <h3 class="rule-title">DON'T</h3>
      <ul class="rule-list">
        <li>Don't use gradients on transactional UI headings</li>
        <li>Don't use flat black or gray backgrounds</li>
        <li>Don't recolor or distort the logo mark</li>
        <li>Don't use emerald/teal for decorative elements</li>
        <li>Don't use Standard/Tight logo at small sizes</li>
        <li>Don't add drop shadows or effects to the logo</li>
      </ul>
    </div>
  </div>

</section>

<style>
  .section-divider {
    height: 1px;
    background: var(--rumi-border);
    margin: 2.5rem 0;
  }
  .section-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.5rem;
    font-weight: 700;
    color: var(--rumi-purple-accent);
    letter-spacing: -0.02em;
    margin-bottom: 0.5rem;
  }
  .section-desc {
    font-size: 0.875rem;
    color: var(--rumi-text-secondary);
    line-height: 1.65;
    max-width: 560px;
  }

  /* Logo showcase */
  .logo-showcase {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
    border-radius: 0.75rem;
    overflow: hidden;
  }
  .logo-showcase-dark {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 3rem;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .logo-showcase-light {
    background: #f0f0f0;
    border-radius: 0.75rem;
    padding: 3rem;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .logo-large { width: 120px; height: 120px; }

  /* Asset cards */
  .asset-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    overflow: hidden;
    transition: border-color 0.15s;
  }
  .asset-card:hover { border-color: var(--rumi-border-hover); }
  .asset-preview {
    background: var(--rumi-bg-surface2);
    padding: 1.5rem;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100px;
  }
  .logo-thumb { width: 64px; height: 64px; }
  .asset-info {
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .asset-label {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }
  .asset-note {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    line-height: 1.4;
  }
  .asset-download {
    margin-top: 0.5rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-action);
    background: none;
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    padding: 0.375rem 0.75rem;
    cursor: pointer;
    transition: all 0.15s;
    text-align: center;
  }
  .asset-download:hover {
    border-color: var(--rumi-action);
    background: rgba(52, 211, 153, 0.05);
  }
  .asset-btn-row {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
  .asset-btn-row .asset-download { flex: 1; margin-top: 0; }

  /* Logo + Wordmark */
  .logo-wordmark-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem 1.5rem;
    width: fit-content;
  }
  .brand-wordmark-demo {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.5rem;
    font-weight: 500;
    letter-spacing: 0.12em;
    color: #e8e4f0;
  }

  /* Color cards */
  .color-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 0.75rem;
  }
  .color-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.625rem;
    overflow: hidden;
    cursor: pointer;
    transition: border-color 0.15s;
    text-align: left;
    width: 100%;
  }
  .color-card:hover { border-color: var(--rumi-border-hover); }
  .color-swatch { height: 64px; width: 100%; }
  .color-meta {
    padding: 0.625rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }
  .color-name {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }
  .color-hex {
    font-size: 0.75rem;
    font-family: 'Inter', monospace;
    color: var(--rumi-text-secondary);
  }
  .color-role {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
  }
  .color-role code {
    font-size: 0.625rem;
  }
  .copied-badge {
    font-size: 0.625rem;
    font-weight: 600;
    color: var(--rumi-action);
    margin-top: 0.125rem;
  }

  /* Border samples */
  .border-samples { display: flex; flex-direction: column; gap: 0.75rem; }
  .border-sample {
    padding: 0.75rem 1rem;
    border: 1px solid;
    border-radius: 0.5rem;
    background: var(--rumi-bg-surface1);
  }

  /* Typography samples */
  .type-samples { display: flex; flex-direction: column; gap: 1.5rem; }
  .type-sample {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .type-name {
    display: block;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.25rem;
  }
  .type-use {
    display: block;
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    margin-bottom: 1rem;
  }

  /* Gradient samples */
  .gradient-samples { display: flex; flex-direction: column; gap: 1rem; }
  .gradient-sample {
    display: flex;
    gap: 1rem;
    align-items: center;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1rem;
  }
  .gradient-preview {
    width: 80px;
    height: 48px;
    border-radius: 0.5rem;
    flex-shrink: 0;
  }
  .gradient-info { display: flex; flex-direction: column; gap: 0.125rem; }
  .gradient-name {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }
  .gradient-code {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
  }
  .gradient-code code { font-size: 0.625rem; }
  .gradient-use {
    font-size: 0.6875rem;
    color: var(--rumi-text-secondary);
  }

  /* Usage rules */
  .rule-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .rule-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 700;
    letter-spacing: 0.05em;
    margin-bottom: 1rem;
  }
  .rule-do .rule-title { color: var(--rumi-action); }
  .rule-dont .rule-title { color: #f87171; }
  .rule-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .rule-list li {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    line-height: 1.5;
    padding-left: 1rem;
    position: relative;
  }
  .rule-do .rule-list li::before {
    content: '✓';
    position: absolute;
    left: 0;
    color: var(--rumi-action);
    font-size: 0.75rem;
  }
  .rule-dont .rule-list li::before {
    content: '✗';
    position: absolute;
    left: 0;
    color: #f87171;
    font-size: 0.75rem;
  }
</style>
