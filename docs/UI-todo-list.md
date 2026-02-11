# Rumi Protocol — UI Todo List

Priority scale: 1 (lowest) → 5 (highest)

---

## [P4] Vault Card: Compact Buttons + Dynamic CR Preview

**File:** `src/vault_frontend/src/lib/components/vault/VaultDetails.svelte`

**Problem:**
- Action buttons (Add, Borrow, Repay) are `w-full`, stretching across the entire column — too wide for single-word labels
- The CR preview ("→ CR 400.9%") is shown inline next to the Add button, cluttering the action row
- The main CR badge in the vault header is static and doesn't react to user input

**Solution:**
1. **Uniform-width right-aligned buttons below inputs** — Keep buttons on the row below their input field, right-aligned with the field's right edge. All three buttons (Add, Borrow, Repay) should be the same width (`w-24`), matching the widest label ("Borrow"). Use `<div class="flex justify-end">` wrapper around each button.
2. **Dynamic CR preview in header** — When the user types a value into Add Collateral or Borrow, update the CR badge in the vault header to show a live preview: `149.4% → 400.9%` with the old value struck through and faded, an arrow, and the new value pulsing in the appropriate health color (green/yellow/red)
3. **Left border color transition** — The vault card's left accent border should also transition to reflect the previewed health status
4. Remove the inline "→ CR X%" text next to the action buttons entirely

**Mockup:** See `vault-mockup.jsx` artifact for interactive comparison.

---

## [P4] Borrow Input: Show Max Borrowable Amount

**File:** `src/vault_frontend/src/lib/components/vault/VaultDetails.svelte`

**Problem:** The Borrow input has no indication of how much the user can borrow. The Add Collateral field shows "Max: X ICP" but Borrow doesn't follow the same pattern.

**Solution:** Add a `Max: X.XX icUSD` label above the Borrow input field, right-aligned, inline with the "Borrow" label (same pattern as Add Collateral's max label). Formula: `(collateral_value_usd / MINIMUM_COLLATERAL_RATIO) - borrowed_amount`. This should be dynamic based on current ICP price and debt.

**Note:** `MINIMUM_COLLATERAL_RATIO` is 1.33 (133%). It's defined as a constant in `protocol.ts` but hardcoded locally in `VaultDetails.svelte` as `minCollateralRatio = 1.33`. Should import from the shared constant for consistency.

---

## [P3] Collapsed Vault Card: Credit Usage Indicator

**File:** `src/vault_frontend/src/lib/components/vault/VaultCard.svelte`

**Problem:** The collapsed vault card shows Collateral, Borrowed, and CR but gives no sense of how leveraged the vault is relative to its limit. Users have to mentally calculate from CR.

**Solution:** Add a "Credit: X% used" label with a small progress bar (~w-20 to w-24) to the right of the Borrowed column on the collapsed card header. Formula: `(borrowed / (collateral_value / 1.33)) * 100`. Bar colors: green (<65%), yellow (65-85%), red (>85%).

**Placement decision needed:** See mockup for 4 options varying top/bottom alignment and spacing. Current recommendation is Option C (top-aligned with "BORROWED" label, closer gap to Borrowed column).

---
