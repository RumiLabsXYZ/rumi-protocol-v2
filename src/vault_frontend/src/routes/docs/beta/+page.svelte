<svelte:head><title>Beta Disclaimer - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Beta Disclaimer</h1>

  <section class="doc-section">
    <h2 class="doc-heading">Status</h2>
    <p>Rumi Protocol is in beta. The software is functional and deployed on the Internet Computer mainnet, but it has not been formally audited and should be considered experimental.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">No Warranty</h2>
    <p>The protocol is provided "as is" without warranty of any kind, express or implied. Rumi Labs LLC makes no guarantees about the correctness, reliability, or security of the software. Use of the protocol is entirely at your own risk.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Risk of Loss</h2>
    <p>You may lose some or all of the assets you deposit into the protocol. This can happen due to smart contract bugs, oracle failures, liquidation events, or other causes described in the <a href="/docs/risks" class="doc-link">risk documentation</a>. Do not deposit more than you can afford to lose.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">No Formal Audit</h2>
    <p>The Rumi Protocol codebase has not undergone a formal security audit by a traditional human-led auditing firm. An AI-powered review was conducted (see below), but undiscovered vulnerabilities may exist.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Security Review</h2>
    <p>The Rumi Protocol codebase was reviewed by <a href="https://www.avai.life/" class="doc-link" target="_blank" rel="noopener">AVAI</a>, an autonomous AI-powered security auditing agent, in December 2025. The review covered 68 Rust source files across all four canisters and found no critical or high-severity issues (3 medium, 5 low). The overall security rating was B+ (83.5/100).</p>
    <p>This is not a traditional third-party audit conducted by a human security firm. It is an AI-assisted review. You can read the <a href="/AVAI_Security_Audit_Rumi_Protocol_Professional.pdf" class="doc-link" target="_blank">full report (PDF)</a>.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Canister Control</h2>
    <p>Protocol canisters are currently controlled by the development team's principals. This means the team can upgrade canister code at any time without a governance vote or time-lock. While this enables rapid bug fixes, it also means you are trusting the team not to deploy malicious or broken code.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Not Financial Advice</h2>
    <p>Nothing in this application constitutes financial, investment, or legal advice. icUSD is not a regulated financial instrument. Consult your own advisors before using this protocol.</p>
  </section>

  <section class="doc-section">
    <p class="doc-legal">© 2025–2026 Rumi Labs LLC. All rights reserved.</p>
  </section>
</article>

<style>
  .doc-legal { font-size: 0.75rem; color: var(--rumi-text-muted); margin-top: 1rem; }
</style>
