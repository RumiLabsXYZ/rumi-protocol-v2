<svelte:head><title>Dev Tooling | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Developer & Admin Tooling</h1>

  <p class="doc-intro">
    All admin functions are restricted to the developer principal or canister controller.
    They are called via <code>dfx canister call</code> on the IC mainnet.
    This page documents every dev/admin endpoint across all Rumi canisters.
  </p>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Dev Testing</h2>

  <div class="doc-card">
    <h3>dev_force_bot_liquidate</h3>
    <p class="doc-desc">Force-liquidate a vault via the bot path, bypassing the CR health check. Full liquidation — seizes all collateral, writes down all debt.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend dev_force_bot_liquidate '(42 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>dev_force_partial_bot_liquidate</h3>
    <p class="doc-desc">Force-liquidate a vault via the bot path using the partial liquidation cap (restores to per-asset min CR), bypassing the CR health check.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend dev_force_partial_bot_liquidate '(42 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>dev_test_pool_only_liquidation</h3>
    <p class="doc-desc">Send a vault directly to the stability pool for liquidation, skipping the bot entirely. Does not bypass the backend's CR check — the vault must actually be underwater.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend dev_test_pool_only_liquidation '(42 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>dev_set_collateral_price</h3>
    <p class="doc-desc">Manually set a collateral asset's cached price (USD). Used for testing liquidation flows with assets that don't have XRC price feeds. The price persists until the next XRC fetch overwrites it (won't happen for assets not on XRC).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend dev_set_collateral_price '(principal "np5km-uyaaa-aaaaq-aadrq-cai", 0.10 : float64)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Emergency Controls</h2>

  <div class="doc-card">
    <h3>freeze_protocol</h3>
    <p class="doc-desc">Emergency kill switch. Halts ALL state-changing operations (borrows, repays, liquidations, redemptions). Only the canister controller can call this. Overrides all modes.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend freeze_protocol --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>unfreeze_protocol</h3>
    <p class="doc-desc">Lifts the emergency freeze and resumes normal operations.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend unfreeze_protocol --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>enter_recovery_mode / exit_recovery_mode</h3>
    <p class="doc-desc">Manually enter or exit Recovery mode. Overrides the automatic CR-based mode transitions. Use sparingly — disables automatic mode switching until exit is called.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend enter_recovery_mode --network ic dfx canister call rumi_protocol_backend exit_recovery_mode --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>admin_mint_icusd</h3>
    <p class="doc-desc">Emergency mint icUSD to a recipient. Capped at 1,500 icUSD per call with a 72-hour cooldown. Logged on-chain and visible on the Transparency page. Use for correcting accounting errors only.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend admin_mint_icusd '(record &#123;&#125; amount_e8s = 100_000_000 : nat64; to = principal "zegjz-..."; reason = "Correct SP burn accounting error" &#125;&#125;)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Fee & Rate Configuration</h2>

  <div class="doc-card">
    <h3>set_borrowing_fee</h3>
    <p class="doc-desc">Set the base one-time borrowing fee (0.0 – 0.10). Applied when minting new icUSD. Per-collateral overrides take precedence if set.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_borrowing_fee '(0.005 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_collateral_borrowing_fee</h3>
    <p class="doc-desc">Set a per-collateral borrowing fee override.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_collateral_borrowing_fee '(principal "ryjl3-tyaaa-aaaaa-aaaba-cai", 0.005 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_interest_rate</h3>
    <p class="doc-desc">Set the annualized interest rate (APR) for a specific collateral type.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_interest_rate '(principal "ryjl3-tyaaa-aaaaa-aaaba-cai", 0.03 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_rate_curve_markers</h3>
    <p class="doc-desc">Set borrowing fee rate curve control points (utilization → fee). Per-collateral if a principal is provided, system-wide if null.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_rate_curve_markers '( opt principal "ryjl3-tyaaa-aaaaa-aaaba-cai", vec &#123; record &#123; 0.5 : float64; 0.005 : float64 &#125;; record &#123; 1.0 : float64; 0.05 : float64 &#125; &#125; )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_ckstable_repay_fee</h3>
    <p class="doc-desc">Set the fee surcharge for repaying debt with ckUSDT or ckUSDC instead of icUSD (0.0 – 0.05).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_ckstable_repay_fee '(0.01 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_redemption_fee_floor / set_redemption_fee_ceiling</h3>
    <p class="doc-desc">Set the min/max redemption fee bounds.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_redemption_fee_floor '(0.005 : float64)' --network ic dfx canister call rumi_protocol_backend set_redemption_fee_ceiling '(0.05 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_reserve_redemption_fee</h3>
    <p class="doc-desc">Set the flat fee for reserve-backed redemptions (ckStable path).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_reserve_redemption_fee '(0.005 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_reserve_redemptions_enabled</h3>
    <p class="doc-desc">Enable or disable the reserve-backed redemption pathway.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_reserve_redemptions_enabled '(true)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Liquidation Configuration</h2>

  <div class="doc-card">
    <h3>set_liquidation_bonus</h3>
    <p class="doc-desc">Set the liquidation bonus multiplier (1.0 – 1.5). Liquidators receive this multiple of the debt value in collateral.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_liquidation_bonus '(1.10 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_liquidation_protocol_share</h3>
    <p class="doc-desc">Set the protocol's share of liquidation bonus profits (0.0 – 1.0). Remainder goes to liquidators.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_liquidation_protocol_share '(0.03 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_liquidation_bot_config</h3>
    <p class="doc-desc">Configure the liquidation bot: set its principal and monthly budget (e8s).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_liquidation_bot_config '( principal "nygob-3qaaa-aaaap-qttcq-cai", 500_000_000 : nat64 )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_bot_allowed_collateral_types</h3>
    <p class="doc-desc">Set which collateral types the liquidation bot is allowed to claim. Vaults with unlisted collateral bypass the bot entirely.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_bot_allowed_collateral_types '(vec &#123;&#125; principal "ryjl3-tyaaa-aaaaa-aaaba-cai" &#125;&#125;)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Collateral Management</h2>

  <div class="doc-card">
    <h3>set_collateral_status</h3>
    <p class="doc-desc">Set a collateral type's status: Active (all operations), BorrowPaused (no new borrows, existing vaults ok), or Frozen (liquidation only).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_collateral_status '( principal "ryjl3-tyaaa-aaaaa-aaaba-cai", variant &#123;&#125; Active &#125;&#125; )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_collateral_debt_ceiling</h3>
    <p class="doc-desc">Set the maximum icUSD that can be minted against a specific collateral type.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_collateral_debt_ceiling '( principal "ryjl3-tyaaa-aaaaa-aaaba-cai", 50_000_000_000 : nat64 )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_healthy_cr</h3>
    <p class="doc-desc">Set the per-collateral "healthy" CR threshold. Vaults above this are fully healthy and don't appear in the manual queue.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_healthy_cr '( principal "ryjl3-tyaaa-aaaaa-aaaba-cai", opt (1.6 : float64) )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_lst_haircut</h3>
    <p class="doc-desc">Set a "haircut" discount for liquid staking tokens used as collateral. Reduces their effective value to account for depeg risk.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_lst_haircut '( principal "buwm7-7yaaa-aaaar-qagva-cai", 0.05 : float64 )' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Recovery Mode</h2>

  <div class="doc-card">
    <h3>set_recovery_cr_multiplier</h3>
    <p class="doc-desc">Set the multiplier applied to minimum CR during recovery mode (1.001 – 1.5). Higher = more conservative borrowing limits in recovery.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_recovery_cr_multiplier '(1.10 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_recovery_target_cr</h3>
    <p class="doc-desc">Set the target CR for partial liquidations during recovery mode.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_recovery_target_cr '(1.50 : float64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_recovery_parameters</h3>
    <p class="doc-desc">Set per-collateral recovery mode parameters: borrowing fee override and liquidation bonus override during recovery.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_recovery_parameters '( principal "ryjl3-tyaaa-aaaaa-aaaba-cai", opt (0.01 : float64), opt (1.15 : float64) )' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_rmr_floor / set_rmr_ceiling / set_rmr_floor_cr / set_rmr_ceiling_cr</h3>
    <p class="doc-desc">Configure the Redemption Margin Ratio (RMR) parameters. Controls how redemption fees scale with system collateralization.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_rmr_floor '(0.96 : float64)' --network ic dfx canister call rumi_protocol_backend set_rmr_ceiling '(1.0 : float64)' --network ic dfx canister call rumi_protocol_backend set_rmr_floor_cr '(2.25 : float64)' --network ic dfx canister call rumi_protocol_backend set_rmr_ceiling_cr '(1.5 : float64)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Interest Distribution</h2>

  <div class="doc-card">
    <h3>set_interest_split</h3>
    <p class="doc-desc">Set the N-way interest revenue distribution. Destinations: stability_pool, treasury, three_pool. Basis points must sum to 10,000.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_interest_split '(vec &#123;&#125; record &#123;&#125; destination = "stability_pool"; bps = 5000 : nat64 &#125;&#125;; record &#123;&#125; destination = "treasury"; bps = 3000 : nat64 &#125;&#125;; record &#123;&#125; destination = "three_pool"; bps = 2000 : nat64 &#125;&#125; &#125;&#125;)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_interest_flush_threshold</h3>
    <p class="doc-desc">Set the minimum accumulated interest (e8s) before it gets flushed to recipients.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_interest_flush_threshold '(10_000_000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_interest_pool_share</h3>
    <p class="doc-desc">Legacy: sets the fraction of interest going to the stability pool (0.0 – 1.0). Superseded by set_interest_split.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_interest_pool_share '(0.50 : float64)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — System Configuration</h2>

  <div class="doc-card">
    <h3>set_min_icusd_amount</h3>
    <p class="doc-desc">Set the minimum icUSD amount for borrow/repay/redemption operations (e8s).</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_min_icusd_amount '(10_000_000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_global_icusd_mint_cap</h3>
    <p class="doc-desc">Set the global cap on total icUSD that can be minted across all collateral types (e8s). Default: unlimited.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_global_icusd_mint_cap '(100_000_000_000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_treasury_principal</h3>
    <p class="doc-desc">Set the treasury canister principal for receiving protocol revenue.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_treasury_principal '(principal "tlg74-oiaaa-aaaap-qrd6a-cai")' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_stability_pool_principal</h3>
    <p class="doc-desc">Set the stability pool canister principal. Required for liquidation cascade.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_stability_pool_principal '(principal "tmhzi-dqaaa-aaaap-qrd6q-cai")' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_three_pool_canister</h3>
    <p class="doc-desc">Set the 3pool canister principal. Required for reserve redemptions and LP token liquidations.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_three_pool_canister '(principal "fohh4-yyaaa-aaaap-qtkpa-cai")' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_stable_token_enabled / set_stable_ledger_principal</h3>
    <p class="doc-desc">Enable/disable ckUSDT or ckUSDC for repayments, and set their ledger principals.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend set_stable_token_enabled '(variant &#123;&#125; CKUSDT &#125;&#125;, true)' --network ic dfx canister call rumi_protocol_backend set_stable_ledger_principal '(variant &#123;&#125; CKUSDT &#125;&#125;, principal "cngnf-vqaaa-aaaar-qag4q-cai")' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Backend — Accounting Corrections</h2>

  <div class="doc-card">
    <h3>admin_correct_vault_collateral</h3>
    <p class="doc-desc">Correct a vault's collateral amount for accounting errors. Use when the on-chain collateral balance doesn't match the tracked amount.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend admin_correct_vault_collateral '(42 : nat64, 100_000_000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>admin_sweep_to_treasury</h3>
    <p class="doc-desc">Sweep untracked collateral (dust from liquidations, rounding errors) to the treasury.</p>
    <pre class="doc-code">dfx canister call rumi_protocol_backend admin_sweep_to_treasury '("Sweep post-liquidation dust")' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Stability Pool</h2>

  <div class="doc-card">
    <h3>emergency_pause / resume_operations</h3>
    <p class="doc-desc">Pause or resume all stability pool operations (deposits, withdrawals, liquidations).</p>
    <pre class="doc-code">dfx canister call rumi_stability_pool emergency_pause --network ic dfx canister call rumi_stability_pool resume_operations --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>register_stablecoin</h3>
    <p class="doc-desc">Register a new stablecoin that can be deposited into the pool. Includes decimals, transfer fee, priority, and LP token config.</p>
    <pre class="doc-code">dfx canister call rumi_stability_pool register_stablecoin '(record &#123;&#125; ledger_id = principal "t6bor-paaaa-aaaap-qrd5q-cai"; symbol = "icUSD"; decimals = 8 : nat8; priority = 1 : nat8; is_active = true; transfer_fee = opt (100_000 : nat64); is_lp_token = null; underlying_pool = null &#125;&#125;)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>register_collateral</h3>
    <p class="doc-desc">Register a collateral type that the pool can receive from liquidations. Auto-pushed by the backend when adding new collateral.</p>
    <pre class="doc-code">dfx canister call rumi_stability_pool register_collateral '(record &#123;&#125; ledger_id = principal "ryjl3-tyaaa-aaaaa-aaaba-cai"; symbol = "ICP"; decimals = 8 : nat8; status = variant &#123;&#125; Active &#125;&#125; &#125;&#125;)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>update_pool_configuration</h3>
    <p class="doc-desc">Update pool parameters: minimum deposit, max liquidations per batch, emergency pause flag.</p>
    <pre class="doc-code">dfx canister call rumi_stability_pool update_pool_configuration '(record &#123;&#125; min_deposit_e8s = 100_000_000 : nat64; max_liquidations_per_batch = 5 : nat64; emergency_pause = false &#125;&#125;)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>admin_correct_balance</h3>
    <p class="doc-desc">Correct a depositor's tracked balance for a specific token to match the actual ledger balance. Used after accounting errors (e.g., burned tokens not deducted).</p>
    <pre class="doc-code">dfx canister call rumi_stability_pool admin_correct_balance '( principal "zegjz-...", principal "fohh4-yyaaa-aaaap-qtkpa-cai", 218_184_186_394 : nat64 )' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Liquidation Bot</h2>

  <div class="doc-card">
    <h3>test_force_liquidate</h3>
    <p class="doc-desc">Force the bot to liquidate a specific vault, bypassing health checks. Full liquidation.</p>
    <pre class="doc-code">dfx canister call liquidation_bot test_force_liquidate '(42 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>test_force_partial_liquidate</h3>
    <p class="doc-desc">Force the bot to partially liquidate a vault using the partial cap formula.</p>
    <pre class="doc-code">dfx canister call liquidation_bot test_force_partial_liquidate '(42 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>test_swap_pipeline</h3>
    <p class="doc-desc">Test the bot's ICP → ckStable → icUSD swap pipeline with a specified amount.</p>
    <pre class="doc-code">dfx canister call liquidation_bot test_swap_pipeline '(10_000_000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_config</h3>
    <p class="doc-desc">Update the bot's runtime configuration (admin principal, backend canister, swap parameters).</p>
    <pre class="doc-code">dfx canister call liquidation_bot set_config '(record &#123;&#125;...&#125;&#125;)' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">3pool (Stablecoin AMM)</h2>

  <div class="doc-card">
    <h3>set_swap_fee / set_admin_fee</h3>
    <p class="doc-desc">Set swap fee (0–100 bps) and admin fee (share of swap fees taken by protocol, 0–10,000 bps).</p>
    <pre class="doc-code">dfx canister call rumi_3pool set_swap_fee '(4 : nat64)' --network ic dfx canister call rumi_3pool set_admin_fee '(5000 : nat64)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>ramp_a / stop_ramp_a</h3>
    <p class="doc-desc">Ramp the amplification parameter (A) over time. Minimum ramp duration: 24 hours. Maximum change: 10x. Can be stopped mid-ramp.</p>
    <pre class="doc-code">dfx canister call rumi_3pool ramp_a '(200 : nat64, 1711065600 : nat64)' --network ic dfx canister call rumi_3pool stop_ramp_a --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>withdraw_admin_fees</h3>
    <p class="doc-desc">Withdraw accumulated admin fees from the 3pool to the admin account.</p>
    <pre class="doc-code">dfx canister call rumi_3pool withdraw_admin_fees --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>set_paused</h3>
    <p class="doc-desc">Pause or unpause 3pool operations (swaps, add/remove liquidity).</p>
    <pre class="doc-code">dfx canister call rumi_3pool set_paused '(true)' --network ic</pre>
  </div>

  <div class="doc-card">
    <h3>add_authorized_burn_caller / remove_authorized_burn_caller</h3>
    <p class="doc-desc">Authorize or revoke a canister's permission to call <code>authorized_redeem_and_burn</code>. The stability pool must be authorized to liquidate using 3USD.</p>
    <pre class="doc-code">dfx canister call rumi_3pool add_authorized_burn_caller '(principal "tmhzi-dqaaa-aaaap-qrd6q-cai")' --network ic</pre>
  </div>

  <!-- ═══════════════════════════════════════════════ -->
  <h2 class="doc-section-title">Liquidation Cascade (No-Retry)</h2>

  <p class="doc-intro">
    When a vault becomes undercollateralized, the system processes it through a one-shot cascade:
  </p>

  <ol class="doc-list">
    <li><strong>Bot</strong> — If the collateral type is in <code>bot_allowed_collateral_types</code>, the bot gets one attempt. It claims the collateral, swaps on KongSwap, deposits icUSD. If the swap fails, it returns the collateral and cancels.</li>
    <li><strong>Stability Pool</strong> — If the bot didn't handle it (not allowed, rejected, or cancelled), the SP gets one attempt on the next <code>check_vaults</code> cycle. It draws stablecoins from depositors and calls the backend to seize collateral. The vault is marked in <code>sp_attempted_vaults</code> to prevent retries.</li>
    <li><strong>Manual Queue</strong> — If both fail, the vault sits in the manual liquidation queue. Anyone can liquidate it via the Liquidate page.</li>
  </ol>

  <p class="doc-intro">
    The no-retry design ensures each tier of the liquidation cascade gets exactly one attempt per vault. If it fails, the vault moves to the next tier rather than retrying and burning tokens on repeated failures.
  </p>

</article>

<style>
  .doc-page {
    max-width: 700px;
    margin: 0 auto;
    font-size: 0.9375rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
  }
  .doc-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.75rem;
    font-weight: 700;
    color: var(--rumi-action);
    margin-bottom: 0.5rem;
  }
  .doc-intro {
    margin-bottom: 1.5rem;
    line-height: 1.6;
  }
  .doc-section-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-top: 2.5rem;
    margin-bottom: 1rem;
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--rumi-border);
  }
  .doc-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.625rem;
    padding: 1rem 1.25rem;
    margin-bottom: 0.75rem;
  }
  .doc-card h3 {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--rumi-accent-green);
    margin-bottom: 0.375rem;
  }
  .doc-desc {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    line-height: 1.5;
    margin-bottom: 0.5rem;
  }
  .doc-code {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.75rem;
    background: var(--rumi-bg-surface2, rgba(0,0,0,0.3));
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    padding: 0.5rem 0.75rem;
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--rumi-text-muted);
  }
  .doc-list {
    padding-left: 1.25rem;
    margin-bottom: 1.5rem;
  }
  .doc-list li {
    margin-bottom: 0.75rem;
    line-height: 1.5;
  }
  code {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.8125rem;
    background: rgba(209, 118, 232, 0.08);
    padding: 0.1em 0.3em;
    border-radius: 0.25rem;
  }
</style>
