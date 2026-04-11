# Explorer Phase 3: Pools Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Pools section showing 3pool health (peg status, virtual price, pool balances, swap volume), stability pool metrics (deposits, liquidations, APY), and historical charts for both pools.

**Architecture:** Single Pools page (`/explorer/pools`) with two main sections: 3Pool and Stability Pool. Each section has vitals cards and a historical chart. Data comes from analyticsService (fetchPegStatus, fetchApys, fetchSwapSeries, fetchStabilitySeries, fetchThreePoolSeries) and explorerService (fetchThreePoolStatus, fetchStabilityPoolStatus).

**Tech Stack:** SvelteKit (Svelte 5 runes), Tailwind CSS, lightweight-charts for time series, TypeScript

---

### Task 1: Pool Balance Bar Component
- Create: `src/vault_frontend/src/lib/components/explorer/PoolBalanceBar.svelte`
- Visual bar showing 3pool token balances as proportional segments (icUSD, USDT, USDC)

### Task 2: Pools Page
- Rewrite: `src/vault_frontend/src/routes/explorer/pools/+page.svelte`
- Sections: 3Pool Overview (peg, virtual price, balances, LP supply, swap volume/fees), Stability Pool Overview (total deposits, SP APY, liquidation count), Historical charts

### Task 3: Build, Deploy, Verify
