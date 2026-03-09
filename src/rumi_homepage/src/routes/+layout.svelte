<script>
  import { page } from '$app/stores';
  import '../app.css';

  let appUrl = 'https://app.rumiprotocol.xyz';
</script>

<div class="min-h-screen flex flex-col relative z-10">
  <header class="top-bar">
    <a href="/" class="top-brand">
      <img src="/rumilogo-vector-v2_inset2.png" alt="Rumi" class="top-logo" />
      <span class="top-wordmark">RUMI</span>
    </a>

    <nav class="top-nav">
      <a href="/" class="nav-link" class:active={$page.url.pathname === '/'}>Home</a>
      <a href="/about" class="nav-link" class:active={$page.url.pathname === '/about'}>About</a>
      <a href="/Rumi-Protocol-v2-Whitepaper.pdf" target="_blank" class="nav-link">Whitepaper</a>
      <a href="/AVAI_Security_Audit_Rumi_Protocol_Professional.pdf" target="_blank" class="nav-link">Audit</a>
    </nav>

    <div class="top-actions">
      <div class="top-social">
        <a href="mailto:info@rumiprotocol.com" class="header-icon-link" aria-label="Email">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z"/>
            <polyline points="22,6 12,13 2,6"/>
          </svg>
        </a>
        <a href="https://x.com/rumilabsxyz" target="_blank" rel="noopener" class="header-icon-link" aria-label="Twitter">
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z"/>
          </svg>
        </a>
        <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener" class="header-icon-link" aria-label="GitHub">
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"/>
          </svg>
        </a>
        <a href="https://t.me/+nVydvqMbUWY3MGE5" target="_blank" rel="noopener" class="header-icon-link" aria-label="Telegram">
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.479.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z"/>
          </svg>
        </a>
        <a href="https://discord.gg/Ywtu9wUP" target="_blank" rel="noopener" class="header-icon-link" aria-label="Discord">
          <svg viewBox="0 0 24 24" fill="currentColor">
            <path d="M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189z"/>
          </svg>
        </a>
      </div>
      <a href={appUrl} target="_blank" rel="noopener" class="launch-cta">Launch App</a>
    </div>
  </header>

  <main class="flex-grow">
    <slot />
  </main>

  <footer class="w-full px-6 py-8 border-t mt-auto"
          style="background: var(--rumi-bg-surface1); border-color: var(--rumi-border);">
    <div class="max-w-6xl mx-auto flex flex-col md:flex-row justify-between items-center gap-4">
      <div class="flex items-center gap-6 text-sm" style="color: var(--rumi-text-muted);">
        <span>&copy; 2025-2026 Rumi Labs LLC</span>
        <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener" class="footer-link">GitHub</a>
        <a href="/Rumi-Protocol-v2-Whitepaper.pdf" target="_blank" class="footer-link">Whitepaper</a>
        <a href="/AVAI_Security_Audit_Rumi_Protocol_Professional.pdf" target="_blank" class="footer-link">Audit</a>
        <a href="/branding" class="footer-link">Branding</a>
        <a href="mailto:info@rumiprotocol.com" class="footer-link">Contact</a>
      </div>
      <a href={appUrl} target="_blank" rel="noopener"
         class="text-sm font-medium transition-colors" style="color: var(--rumi-action);">
        Launch App →
      </a>
    </div>
  </footer>
</div>

<style>
  /* ── Top bar: CSS Grid matching the app layout ── */
  .top-bar {
    position: sticky;
    top: 0;
    left: 0;
    right: 0;
    width: 100%;
    height: 3.5rem;
    background: rgba(8,11,22,0.85);
    backdrop-filter: blur(12px);
    border-bottom: 1px solid var(--rumi-border);
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    padding: 0 1.5rem;
    z-index: 50;
  }
  .top-brand { display: flex; align-items: center; gap: 0.5rem; text-decoration: none; }
  .top-logo { width: 2rem; height: 2rem; }
  .top-wordmark {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.0625rem;
    font-weight: 500;
    letter-spacing: 0.12em;
    color: var(--rumi-text-primary);
  }

  /* ── Nav: centered in middle grid column ── */
  .top-nav { display: flex; align-items: center; gap: 0.25rem; justify-self: center; }
  .nav-link {
    position: relative;
    display: flex;
    align-items: center;
    padding: 1rem;
    color: var(--rumi-text-muted);
    text-decoration: none;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 500;
    letter-spacing: 0.01em;
    transition: color 0.15s ease;
    white-space: nowrap;
  }
  .nav-link:hover { color: var(--rumi-text-primary); }
  .nav-link.active { color: var(--rumi-text-primary); }
  .nav-link.active::after {
    content: '';
    position: absolute;
    bottom: 0;
    left: 1rem;
    right: 1rem;
    height: 2px;
    background: var(--rumi-action);
    border-radius: 1px 1px 0 0;
  }

  /* ── Right side: social + launch CTA ── */
  .top-actions { display: flex; align-items: center; gap: 0.75rem; justify-self: end; }
  .top-social { display: flex; gap: 0.25rem; align-items: center; }
  .header-icon-link {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.375rem;
    color: var(--rumi-text-muted);
    text-decoration: none;
    transition: color 0.15s ease;
  }
  .header-icon-link:hover { color: var(--rumi-text-primary); }
  .header-icon-link svg { width: 0.875rem; height: 0.875rem; }

  .launch-cta {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 500;
    padding: 0.625rem 1.25rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    border-radius: 0.5rem;
    text-decoration: none;
    transition: all 0.2s ease;
    white-space: nowrap;
  }
  .launch-cta:hover {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 20px rgba(52,211,153,0.15);
  }

  /* ── Mobile: hide nav ── */
  @media (max-width: 768px) {
    .top-nav { display: none; }
    .top-social { display: none; }
  }

  .footer-link {
    color: var(--rumi-text-muted);
    text-decoration: none;
    transition: color 0.15s ease;
  }

  .footer-link:hover {
    color: var(--rumi-text-secondary);
  }
</style>
