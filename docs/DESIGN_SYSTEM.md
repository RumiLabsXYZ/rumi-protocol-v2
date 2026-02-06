# Rumi Protocol — Design Constitution

> This is a standing design constitution. Treat as non-negotiable unless explicitly revised by the team.

---

## Context & Audience

Rumi is a lending, borrowing, and stability protocol built specifically for ICP holders.

The core audience is ICP maxis and long-term ICP holders:
- Users deeply familiar with NNS, OpenChat, ICPSwap, canisters, Motoko, and ICP culture
- Users who do NOT primarily come to ICP for DeFi
- Users who value sovereignty, technical credibility, and infrastructure over hype

**ICP identity is not optional.**
If the interface does not feel ICP-native, the product has no pitch.

Rumi is NOT trying to:
- Attract general Ethereum/Solana DeFi tourists
- Look chain-agnostic
- Hide its L1 affiliation

Association with ICP is a feature, not a liability.

---

## Primary vs Secondary Brand

Rumi has TWO intentional brand modes. They must never be mixed accidentally.

### Primary Brand: "Protocol Rumi"
- Lives anywhere money moves or risk exists
- Emotional contract: calm, confident, durable, boring when safe
- Visual contract: restrained, cool, structural, information-forward
- Goal: **earn trust**

### Secondary Brand: "Community Rumi"
- Lives in social, narrative, celebratory, or expressive spaces
- Emotional contract: expressive, optimistic, weird (in an ICP-native way)
- Visual contract: higher saturation, gradients, logo inversions, play
- Goal: **earn affection**

**Rule:** Primary brand earns trust. Secondary brand earns affection. Never use secondary brand expression inside transactional or risk surfaces.

---

## Core Design Principles (Non-Negotiable)

### 1. ICP Is the Material, Not the Decoration

ICP identity should be felt through:
- Deep cool purples
- Structural consistency
- Technical confidence

NOT through: ICP logos, badges, "Powered by ICP" callouts, or gimmicks.

If the UI sits next to NNS or ICPSwap, it should feel like family.

### 2. Calm by Default, Loud Only When It Matters

- Safe vaults should feel quiet
- Risk should be unmistakable
- Color intensity is earned, not constant

No excitement unless something important is happening.

### 3. Numbers Are the Interface

The most important UI elements are always numbers: prices, balances, collateral ratios, health factors.

If you squint at the screen, numbers should dominate. Everything else supports comprehension.

### 4. Density Over Decoration

This is a protocol interface, not a marketing site. Avoid: decorative gradients, hero-style sections inside the app, unnecessary cards, ornamental dividers.

Every pixel must justify itself in clarity or trust.

### 5. The Protocol Is the Brand

Rumi does not "apply branding" to the UI. The brand emerges from: consistency, restraint, precision, confidence.

If it feels solid, it *is* solid.

---

## Color System

### Background System (Structural, Not Decorative)

Backgrounds must read as: dark, cool, unmistakably purple, never brown/gray/warm.

Purple is the base material of the interface.

| Role | Description |
|------|-------------|
| Page background | Very dark cool purple |
| Surface 1 | Slightly elevated purple (sidebar, header, cards) |
| Surface 2 | Clearly visible purple (elevated cards, modals, inputs) |
| Surface 3 | Hover / focus purple |

If compared side-by-side with a neutral dark UI, Rumi must clearly look purple.

**NO** flat black. **NO** gray masquerading as purple. **NO** warm desaturation.

### Accent Color: Teal — Earned, Not Sprayed

Teal (`#2DD4BF`) is Rumi's voice inside the ICP ecosystem.

**Teal means:** interactivity, key protocol values, success.

**Teal must NOT be used for:** headlines by default, decoration, card borders, step numbers, ambient accents.

Fewer teal moments = stronger teal signal.

### Risk Color System (The Most Expressive Element)

This is where color gets loud.

| State | Color | Meaning |
|-------|-------|---------|
| Safe (>200% CR) | Teal/green | Calm, expected |
| Caution (150-200% CR) | Amber | Warm, noticeable |
| Danger (<150% CR) | Red | Urgent, unmissable |
| Liquidatable (<130% CR) | Bright red + subtle pulse/glow | The loudest moment in the UI |

The collateral ratio visualization should be the most expressive color element in the entire app.

---

## Typography

### Typeface Pairing
- **Circular Std:** structure, headings, identity
- **Inter:** numbers, data, readability

### Headlines
- Solid off-white with slight purple tint (`~#e8e4f0`)
- NO gradient by default in primary brand surfaces
- Weight and spacing provide confidence, not color

### Numbers
- Visually dominant — heavier weight, larger size
- Tabular numerals where possible
- Color only when meaningfully emphasized (e.g., teal for key values, risk colors for CR)

### Labels & Support Text
- Quiet, muted purple-gray
- Never competing with numbers

---

## Gradient Rule (Very Important)

Gradient text inside the core application is a 2021 crypto aesthetic and reads as novelty over durability.

**Rule:** No gradient headlines in core transactional UI.

**Gradients are allowed only when:**
- The user is not making a financial decision
- The context is narrative, celebratory, or educational
- The gradient functions as identity, not decoration

Gradients belong to **Secondary Brand** by default.

---

## Placement Rules (Hard Constraints)

### Secondary Brand MUST NOT appear in:
- Borrow / Repay / Liquidate flows
- Vault dashboards
- Stability Pool deposit screens
- Risk indicators / health factor displays

### Secondary Brand IS appropriate in:
- Community channels & announcements
- Learn section
- Blog headers
- Zero states & illustrations
- Occasional marketing surfaces

---

## Working Expectations

When proposing design changes:
- Default to restraint
- Justify any expressive choice
- Prefer subtraction over addition
- Ask: *"Does this increase trust or clarity?"*

Avoid generic DeFi SaaS aesthetics.
Avoid chain-agnostic design instincts.
Design like this protocol must still feel correct in 5 years.

**Be opinionated. Optimize for ICP-native credibility, not trend alignment.**
