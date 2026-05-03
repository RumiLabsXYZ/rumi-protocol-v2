# Rumi Explorer — Design System

## Direction & feel

A geologist's field journal opened on a trading desk after midnight. Materials-forward, dense but unhurried. The user is examining bedrock — not chasing yields. Numbers in monospace tabular form throughout. Light mode is the default presentation; dark mode is a slate stack for night-owl operators.

## Domain palette

Token names belong to the world: parchment, ink, slate, quartz, oxidized copper, sodium-vapor amber, mineral red, peg-blue.

| Token | Light | Dark | Use |
|---|---|---|---|
| `vellum` | warm off-white `40 22% 96%` | deep slate `222 25% 8%` | page background |
| `vellum-raised` | parchment `38 18% 99%` | raised slate `222 22% 11%` | cards |
| `vellum-inset` | warm `42 20% 93%` | inset `222 28% 6%` | inputs |
| `ink-primary` | deep `220 25% 12%` | parchment off-white `40 20% 92%` | primary text |
| `ink-secondary` | `220 15% 30%` | `40 12% 75%` | secondary text |
| `ink-muted` | `220 10% 50%` | `220 10% 55%` | metadata |
| `ink-disabled` | `220 8% 70%` | `220 12% 35%` | disabled |
| `quartz-rule` | rgba black 10% | rgba white 8% | standard borders |
| `quartz-rule-soft` | 6% | 4% | softer separation |
| `quartz-rule-emphasis` | 18% | 16% | emphasis |
| `verdigris` | oxidized green `158 30% 36%` | brighter `158 35% 50%` | primary accent |
| `sodium` | warm amber `36 75% 48%` | brighter `38 80% 60%` | warning |
| `cinnabar` | mineral red `8 55% 42%` | `8 60% 55%` | destructive — breakers, liquidations |
| `peg` | quiet blue `210 38% 42%` | `210 40% 60%` | the meridian color |

**No multiple accent colors.** Verdigris is the single primary. Sodium and cinnabar are reserved for state, not decoration.

## Depth strategy

**Borders-only** in light mode (parchment ground demands it). **Subtle surface tints** in dark mode (each elevation = a few percentage points lighter than the level below). No shadows. No mixed approaches.

Borders are quartz at low opacity — they define edges without demanding attention. The squint test: blur eyes at the interface, you should still perceive structure but nothing jumps out. Solid hex borders look amateur in comparison.

## Typography

- **Inter** for UI text. Headlines use `tracking-tightest` (-0.02em).
- **JetBrains Mono** for ALL numbers. Apply via `font-mono tabular-nums` or the `[data-tabular]` attribute / `tabular-num` class.
- Headline scale: `text-3xl font-semibold tracking-tightest` for page titles.
- Section labels: `text-sm font-medium tracking-[0.08em] uppercase text-ink-muted`.
- Card labels: `text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium`.
- Card values: `text-2xl font-semibold tabular-nums font-mono text-ink-primary`.

## Geometry

- Spacing base: 4px (Tailwind default). Stick to multiples (`gap-3`, `gap-4`, `mb-8`, `py-2.5`, `px-4`).
- Radius scale: `--radius-sm: 4px`, `--radius: 6px`, `--radius-lg: 10px`. Sharper feels technical (the explorer's intent). Cards use `rounded-md`. Buttons + chips use `rounded-sm`. Modals use `rounded-lg`.

## Signature elements

Three signatures that anchor the design. They appear together; removing them makes the explorer indistinguishable from any other dashboard.

### 1. Peg meridian
A 1px reference line at the protocol's peg target ($1.0000), drawn dashed in `--peg-meridian` color. Quiet, never demanding. Anchors every chart where the peg is meaningful (price, virtual price, peg series).

Component: `src/components/design/PegMeridian.tsx`. Render as a sibling to `<Area>` inside any recharts chart: `<PegMeridian y={1} />`. Pass through `MiniAreaChart`'s `peg?: number` prop.

Also rendered as an inline mini-bar on the Overview page's Peg metric card (custom `<PegBar>` SVG showing distance from $1.00).

### 2. Vault glyph
A small SVG rectangle whose fill height = collateral ratio (capped at 200%). Closed vaults render as outline only. Liquidated vaults render with a cinnabar diagonal. Below 110% CR, fill renders cinnabar instead of verdigris.

Component: `src/components/design/VaultGlyph.tsx`. Used inline at 16-24px in tables and metric cards. At 48px in vault detail headers.

Lookup: `VaultGlyph` next to vault count = "1 representative glyph at system CR." A row of N glyphs at varying CR = vault distribution spectrum (used on Collateral lens).

### 3. Ledger entries
Activity rows render like an accountant's journal entry:
```
[date · time]   [glyph]  [kind label] [variable summary]                    [amount]
```
Mono timestamps (left), kind glyph (centered, ⊕/↗/✕/etc.), kind label + summary (variable middle), tabular-num amount (right). No zebra striping. Just bottom border in `border-quartz`.

Component: `src/components/design/LedgerEntry.tsx`. Used in Overview's recent-activity, the Activity feed, every lens with a recent_events list, and entity pages.

The kind glyph mapping: `open_vault: ⊕`, `close_vault: ⊖`, `borrow: ↗`, `repay: ↙`, `liquidation: ✕`, `redemption: ↺`, `stability_pool_deposit: ▼`, `stability_pool_withdraw: ▲`, `admin_mint: +`, etc.

## Component patterns

### Page header
```tsx
<header className="mb-8 pb-4 border-b border-quartz">
  <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">{TITLE}</h1>
  <p className="text-sm text-ink-muted mt-1 tabular-nums">{SUBTITLE_WITH_TIMESTAMP}</p>
</header>
```

Used on Overview and every lens. Quartz rule below the header creates a "page break" that feels like a journal divider.

### Metric card (shaped to its meaning, NOT identical KPI grid)
```tsx
<div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 min-w-[160px]">
  <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">{LABEL}</p>
  <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary">{VALUE}</p>
  {/* shaped indicator unique to this metric — peg bar, vault glyph, sparkline, etc. */}
  <p className="text-[11px] text-ink-muted mt-1">{SUBTITLE}</p>
</div>
```

Layout: `flex flex-wrap gap-3` so cards grow to natural content width. NEVER a 4-column grid of identical KPI boxes — that's the default we explicitly rejected.

### Section header
```tsx
<h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">
  {SECTION_LABEL}
</h2>
```

Spaced uppercase tracking gives section labels the feel of journal margin headers.

### Container card
```tsx
<div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
  {/* LedgerEntry rows or content */}
</div>
```

### Filter chip (toggleable)
```tsx
<button className={cn(
  "text-xs px-2 py-1 rounded-sm border transition-colors",
  selected
    ? "bg-verdigris-soft text-verdigris border-verdigris/30"
    : "bg-vellum-inset border-quartz text-ink-secondary hover:bg-vellum-inset/80"
)}>
```

### Status badge
- Healthy: `bg-verdigris/10 text-verdigris border-verdigris/20`
- Degraded: `bg-sodium/10 text-sodium border-sodium/20`
- Unhealthy: `bg-cinnabar/10 text-cinnabar border-cinnabar/20`

### Synthesized / approximate badge
`bg-sodium-soft text-sodium border-sodium/30` with `ⓘ` glyph and a `title=` tooltip explaining the approximation.

## Lens differentiation rules

Each lens should feel slightly different — not six identical "header + strip + chart + table" pages. Established differentiation:

- **Collateral**: chart + vault distribution spectrum (7 glyphs at varying fills) + events
- **Stability Pool**: chart + inline explainer (`text-sm text-ink-muted max-w-prose`) + events
- **Revenue**: three side-by-side sparklines (borrow / redemption / swap) + events
- **Redemptions**: events-only (no chart) — reads like a logbook
- **DEX**: chart + events
- **Admin**: events-only — audit log

## Animation

Fast micro-interactions only. Color transitions on hover should be `transition-colors` (Tailwind default 150ms). No springs, no bounces. Page-level transitions: avoid; let routing be instant.

## Chart conventions

- Single thin line (1.5px) in `--verdigris`, no fill
- Grid: horizontal-only in `--quartz-rule-soft`, no vertical lines
- Axis ticks: `JetBrains Mono` 10px, `--ink-muted`
- Tooltips: `--vellum-raised` background, `--quartz-rule-emphasis` border, mono content
- Active dot on hover: 3px verdigris with `--vellum` ring
- Peg meridian: when chart shows a peggable series, render `<PegMeridian y={1} />` inside

## What this design is NOT

- Not a dark-mode-by-default crypto dashboard (light is default; dark is opt-in)
- Not vibrant green — the accent is oxidized green (verdigris), not springtime emerald
- Not gradient fills under chart areas — ink on parchment, not painted
- Not zebra-striped data tables — ledger entries with simple bottom borders
- Not a 4-column KPI grid — shaped metrics that grow to natural width
- Not multiple accent colors — verdigris primary, sodium for warning state, cinnabar for critical state, peg-blue ONLY for the meridian

## Files

- `src/index.css` — design tokens (CSS variables for both light/dark)
- `tailwind.config.ts` — exposes tokens as Tailwind classes (`bg-vellum-raised`, `text-ink-muted`, etc.)
- `src/theme/ThemeProvider.tsx` — light is default
- `src/components/design/PegMeridian.tsx`
- `src/components/design/VaultGlyph.tsx`
- `src/components/design/LedgerEntry.tsx`
- `src/components/lenses/MiniAreaChart.tsx`
- `src/components/lenses/LensHealthStrip.tsx`
