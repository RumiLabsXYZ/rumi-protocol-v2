# Earn pool tabs

Supersedes the overview-page layout in 2026-09-13-earn-navigation.md.

Earn opens the pool with the highest available APY at entry. Equal rates or two unavailable rates default to 3USD. Each request has a five-second decision timeout; unavailable data is never displayed as zero. Auto-selection happens once and yields immediately to manual navigation. Direct pool URLs preserve their destination. Existing APY calculations and their service caching are unchanged.

Both pool pages share a prominent selector with 3USD branding and Stability Pool labels. Each shows an APY badge beside its label on desktop, stacking below the label at narrow widths. The selected pool description precedes its existing deposit/withdraw interface. Manual liquidation links remain available.

Validation: 598 frontend tests passed across 55 files; production frontend build passed. Svelte check retains the same 28 baseline error messages, with warnings reduced from 46 to 45. Browser checks against the local production build confirmed higher-rate selection, switching in both directions, keyboard activation, browser Back, purple 3USD branding, and fitting selectors at 1440, 768, 390, and 320 pixels. No wallet was connected and no transaction was attempted. These checks establish local frontend behavior, not deployment.
