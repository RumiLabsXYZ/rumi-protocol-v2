<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { walletStore as wallet } from "../lib/stores/wallet";
  import { permissionStore } from "../lib/stores/permissionStore";
  import WalletConnector from "../lib/components/wallet/WalletConnector.svelte";
  import PriceDebug from "../lib/components/debug/PriceDebug.svelte";
  import WalletDebug from "../lib/components/debug/WalletDebug.svelte";
  import "../app.css";
  import { protocolService } from "../lib/services/protocol";
  import { isDevelopment } from "../lib/config";
  import { developerAccess } from "../lib/stores/developer";
  let permissionInitialized = false;
  let showDebug = false;
  $: currentPath = $page.url.pathname;
  $: ({ isConnected } = $wallet);
  $: isDeveloperMode = isDevelopment || ($permissionStore.initialized && $permissionStore.isDeveloper);
  $: canViewVaults = isDevelopment || $developerAccess || isConnected || ($permissionStore.initialized && $permissionStore.canViewVaults);
  $: if (isConnected && !permissionInitialized) {
    permissionStore.init().then(s => { if (s) permissionInitialized = true; }).catch(() => { permissionInitialized = true; });
  } else if (!isConnected && permissionInitialized) { permissionStore.clear(); permissionInitialized = false; }
  onMount(async () => {
    try { await wallet.initialize(); } catch (e) { console.error('Wallet init failed:', e); }
    if (isConnected && !permissionInitialized) { try { if (await permissionStore.init()) permissionInitialized = true; } catch (e) { permissionInitialized = true; } }
    protocolService.getICPPrice().catch(() => {});
    // Ctrl+D toggles debug panels in dev mode
    const handleKey = (e: KeyboardEvent) => { if (e.ctrlKey && e.key === 'd') { e.preventDefault(); showDebug = !showDebug; } };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });
</script>
<header class="top-bar">
  <a href="/" class="top-brand"><img src="/rumi-header-logo.png" alt="Rumi" class="top-logo" /><span class="top-wordmark">RUMI</span></a>
  <nav class="top-nav">
    <a href="/" class="nav-link" class:active={currentPath === '/'}><span>Borrow</span></a>
    {#if isConnected && canViewVaults}<a href="/vaults" class="nav-link" class:active={currentPath.startsWith('/vaults')}><span>Vaults</span></a>{/if}
    <a href="/liquidations" class="nav-link" class:active={currentPath === '/liquidations'}><span>Liquidate</span></a>
    <a href="/stability-pool" class="nav-link" class:active={currentPath === '/stability-pool'}><span>Stability</span></a>
    {#if isConnected && $permissionStore.isDeveloper}<a href="/treasury" class="nav-link" class:active={currentPath === '/treasury'}><span>Treasury</span></a>{/if}
    <a href="/docs" class="nav-link" class:active={currentPath.startsWith('/docs')}><span>Docs</span></a>
  </nav>
  <div class="top-actions">
    <span class="beta-chip" title="This protocol is in beta. Use at your own risk.">Beta</span>
    <div class="top-social">
      <a href="mailto:team@rumiprotocol.io" class="header-icon-link" aria-label="Email"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z"/><polyline points="22,6 12,13 2,6"/></svg></a>
      <a href="https://x.com/rumilabsxyz" target="_blank" rel="noopener noreferrer" class="header-icon-link" aria-label="Twitter"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z"/></svg></a>
      <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener noreferrer" class="header-icon-link" aria-label="GitHub"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"/></svg></a>
    </div>
    <WalletConnector />
  </div>
</header>
<main class="main-content"><slot /></main>
<footer class="app-footer"><span>&copy; 2025 Rumi Labs LLC</span>{#if isConnected}<span class="footer-status"><span class="status-dot"></span>Connected to IC</span>{/if}</footer>
<nav class="mobile-nav">
  <a href="/" class="mob-item" class:active={currentPath === '/'}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9,22 9,12 15,12 15,22"/></svg><span>Borrow</span></a>
  {#if isConnected && canViewVaults}<a href="/vaults" class="mob-item" class:active={currentPath.startsWith('/vaults')}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg><span>Vaults</span></a>{/if}
  <a href="/liquidations" class="mob-item" class:active={currentPath === '/liquidations'}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><line x1="12" y1="1" x2="12" y2="23"/><path d="M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6"/></svg><span>Liquidate</span></a>
  <a href="/stability-pool" class="mob-item" class:active={currentPath === '/stability-pool'}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg><span>Stability</span></a>
</nav>
{#if isDevelopment && showDebug}<div class="fixed bottom-4 right-4 z-50"><div class="flex flex-col gap-2"><PriceDebug /><WalletDebug /></div></div>{/if}
<style>
  /* ── Top bar: CSS Grid for true center nav ──
     3-column grid: [brand] [nav] [actions]
     Nav is viewport-centered regardless of left/right content width.
     Brand and actions size to content. */
  .top-bar { position:fixed;top:0;left:0;right:0;height:3.5rem;background:var(--rumi-bg-surface-1);border-bottom:1px solid var(--rumi-border);display:grid;grid-template-columns:auto 1fr auto;align-items:center;padding:0 1.5rem;z-index:100; }
  .top-brand { display:flex;align-items:center;gap:0.5rem;text-decoration:none; }
  .top-logo { width:2rem;height:2rem; }
  .top-wordmark { font-family:'Circular Std','Inter',sans-serif;font-size:1.0625rem;font-weight:500;letter-spacing:0.08em;background:var(--rumi-identity-gradient);-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent; }

  /* ── Nav: centered in middle grid column ── */
  .top-nav { display:flex;align-items:center;gap:0.25rem;justify-self:center; }
  .nav-link { position:relative;display:flex;align-items:center;padding:1rem 1rem;color:var(--rumi-text-muted);text-decoration:none;font-family:'Circular Std','Inter',sans-serif;font-size:0.9375rem;font-weight:500;letter-spacing:0.01em;transition:color 0.15s ease;white-space:nowrap; }
  .nav-link:hover { color:var(--rumi-text-primary); }
  .nav-link.active { color:var(--rumi-text-primary); }
  .nav-link.active::after { content:'';position:absolute;bottom:0;left:1rem;right:1rem;height:2px;background:var(--rumi-action);border-radius:1px 1px 0 0; }

  /* ── Right side: beta + social + wallet ── */
  .top-actions { display:flex;align-items:center;gap:0.75rem;justify-self:end; }
  .beta-chip {
    font-size:0.625rem;font-weight:500;padding:0.125rem 0.4375rem;border-radius:999px;
    background:rgba(217,165,60,0.10);color:#c9952a;letter-spacing:0.02em;cursor:default;
    position:relative;line-height:1.4;
  }
  .beta-chip:hover::after {
    content:'This protocol is in beta. Use at your own risk.';
    position:absolute;top:calc(100% + 6px);right:0;
    padding:0.375rem 0.625rem;background:var(--rumi-bg-surface3);
    border:1px solid var(--rumi-border);border-radius:0.375rem;
    font-size:0.6875rem;color:var(--rumi-text-secondary);
    white-space:nowrap;z-index:110;pointer-events:none;
  }
  .top-social { display:flex;gap:0.25rem;align-items:center; }
  .header-icon-link { display:flex;align-items:center;justify-content:center;width:1.75rem;height:1.75rem;border-radius:0.375rem;color:var(--rumi-text-muted);text-decoration:none;transition:color 0.15s ease; }
  .header-icon-link:hover { color:var(--rumi-text-primary); }
  .header-icon-link svg { width:0.875rem;height:0.875rem; }

  /* ── Main content ── */
  .main-content { padding:4.75rem 2rem 2rem;min-height:100vh;position:relative;z-index:1;max-width:1200px;margin:0 auto; }
  .app-footer { padding:1.25rem 2rem;border-top:1px solid var(--rumi-border);display:flex;justify-content:center;align-items:center;gap:2rem;font-size:0.75rem;color:var(--rumi-text-muted); }
  .footer-status { display:flex;align-items:center;gap:0.375rem; }
  .status-dot { width:0.375rem;height:0.375rem;background:var(--rumi-safe);border-radius:50%;box-shadow:0 0 6px rgba(16,185,129,0.4); }

  /* ── Mobile bottom nav ── */
  .mobile-nav { display:none;position:fixed;bottom:0;left:0;right:0;height:3.5rem;background:var(--rumi-bg-surface-1);border-top:1px solid var(--rumi-border);z-index:100;justify-content:space-around;align-items:center; }
  .mob-item { display:flex;flex-direction:column;align-items:center;gap:0.125rem;padding:0.375rem 0.75rem;border-radius:0.375rem;color:var(--rumi-text-muted);text-decoration:none;font-size:0.625rem; }
  .mob-item svg { width:1.125rem;height:1.125rem; }
  .mob-item.active { color:var(--rumi-action); }
  @media (max-width:768px) { .top-nav{display:none} .main-content{padding:4.25rem 1rem 5rem} .mobile-nav{display:flex} }
</style>
