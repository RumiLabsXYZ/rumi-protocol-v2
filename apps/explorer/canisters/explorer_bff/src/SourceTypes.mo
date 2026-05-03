import Principal "mo:core/Principal";

module {

  // ── rumi_analytics ──

  public type PegStatus = {
    virtual_price : Nat;
    timestamp_ns : Nat64;
    pool_balances : [Nat];
    balance_ratios : [Float];
    max_imbalance_pct : Float;
  };

  // Matches real rumi_analytics.did exactly (anti-pattern A2: shadow type drift cost
  // us a deploy round when this didn't match). Fields are by hash on the wire.
  public type TwapEntry = {
    latest_price : Float;
    collateral : Principal;
    sample_count : Nat32;
    twap_price : Float;
    symbol : Text;
  };

  public type ProtocolSummary = {
    peg : ?PegStatus;
    lp_apy_pct : ?Float;
    timestamp_ns : Nat64;
    sp_apy_pct : ?Float;
    total_debt_e8s : Nat64;
    circulating_supply_icusd_e8s : ?Nat;
    prices : [TwapEntry];
    total_vault_count : Nat32;
    total_collateral_usd_e8s : Nat64;
    system_cr_bps : Nat32;
    swap_count_24h : Nat32;
    volume_24h_e8s : Nat64;
  };

  public type CursorStatus = {
    last_error : ?Text;
    source_count : Nat64;
    name : Text;
    last_success_ns : Nat64;
    cursor_position : Nat64;
  };

  // Matches real rumi_analytics.did
  public type ErrorCounters = {
    amm : Nat64;
    three_pool : Nat64;
    icusd_ledger : Nat64;
    stability_pool : Nat64;
    backend : Nat64;
  };

  // Matches real rumi_analytics.did
  public type BalanceTrackerStats = {
    token : Principal;
    total_tracked_e8s : Nat64;
    holder_count : Nat64;
  };

  public type CollectorHealth = {
    balance_tracker_stats : [BalanceTrackerStats];
    backfill_active : [Principal];
    error_counters : ErrorCounters;
    last_pull_cycle_ns : Nat64;
    cursors : [CursorStatus];
  };

  // ── rumi_protocol_backend ──

  public type Mode = { #ReadOnly; #GeneralAvailability; #Recovery };

  public type ProtocolStatus = {
    mode : Mode;
    total_icusd_borrowed : Nat64;
    total_collateral_ratio : Float;
    liquidation_breaker_tripped : Bool;
    manual_mode_override : Bool;
    snapshot_ts_ns : Nat64;
  };

  public type EventTypeFilter = {
    #OpenVault; #CloseVault; #AdjustVault;
    #Borrow; #Repay;
    #Liquidation; #PartialLiquidation;
    #Redemption; #ReserveRedemption;
    #StabilityPoolDeposit; #StabilityPoolWithdraw;
    #AdminMint; #AdminSweepToTreasury;
    #Admin;
    #PriceUpdate; #AccrueInterest;
  };

  public type TimeRange = { from_ns : Nat64; to_ns : Nat64 };

  public type GetEventsFilteredArg = {
    start : Nat64;
    length : Nat64;
    types : ?[EventTypeFilter];
    principal : ?Principal;
    collateral_token : ?Principal;
    time_range : ?TimeRange;
    min_size_e8s : ?Nat64;
    admin_labels : ?[Text];
  };

  public type EventSummary = {
    global_index : Nat64;
    kind : Text;
    timestamp_ns : Nat64;
    primary_principal : ?Principal;
    amount_e8s : ?Nat64;
    payload_summary : Text;
  };

  public type GetEventsFilteredResponse = {
    total : Nat64;
    events : [EventSummary];
  };

  // ── mock_backend vault types ──

  public type VaultStatus = { #Open; #Closed; #Liquidated };

  public type VaultSummary = {
    vault_id : Nat64;
    status : VaultStatus;
    owner : Principal;
    collateral_type : Principal;
    collateral_amount_e8s : Nat64;
    debt_icusd_e8s : Nat64;
    collateral_ratio : ?Float;
  };

  // ── mock_analytics per-entity types ──

  public type TokenBalance = {
    ledger : Principal;
    symbol : Text;
    balance_e8s : Nat64;
    decimals : Nat8;
  };

  public type SpDeposit = {
    total_deposited_e8s : Nat64;
    current_balance_e8s : Nat64;
    earned_collateral : [(Principal, Nat64)];
  };

  public type AddressHoldings = {
    owner : Principal;
    vaults_owned_ids : [Nat64];
    sp_deposits : [SpDeposit];
    token_balances : [TokenBalance];
    total_value_usd : Float;
  };

  public type PoolState = {
    pool_id : Text;
    pool_label : Text;
    pool_kind : Text;
    reserves : [(Principal, Nat64, Nat8)];
    lp_total_supply_e8s : Nat64;
    virtual_price : ?Float;
  };

  public type TokenMetadata = {
    ledger : Principal;
    symbol : Text;
    decimals : Nat8;
    total_supply_e8s : Nat64;
    fee_e8s : Nat64;
  };

  // ── mock_analytics series types ──

  public type RangeQuery = {
    to_ts : ?Nat64;
    from_ts : ?Nat64;
    offset : ?Nat64;
    limit : ?Nat32;
  };

  public type TvlPoint = {
    timestamp_ns : Nat64;
    total_collateral_usd_e8s : Nat64;
    vault_count : Nat32;
  };

  public type TvlSeriesResponse = { rows : [TvlPoint] };

  public type FeePoint = {
    timestamp_ns : Nat64;
    borrow_fees_e8s : Nat64;
    redemption_fees_e8s : Nat64;
    swap_fees_e8s : Nat64;
  };

  public type FeeSeriesResponse = { rows : [FeePoint] };

  public type RedemptionPoint = {
    timestamp_ns : Nat64;
    count : Nat32;
    volume_e8s : Nat64;
  };

  public type RedemptionSeriesResponse = { rows : [RedemptionPoint] };

  public type SwapPoint = {
    timestamp_ns : Nat64;
    count : Nat32;
    volume_e8s : Nat64;
  };

  public type SwapSeriesResponse = { rows : [SwapPoint] };

  public type StabilityPoint = {
    timestamp_ns : Nat64;
    total_deposits_e8s : Nat64;
    apy_pct : Float;
  };

  public type StabilitySeriesResponse = { rows : [StabilityPoint] };

};
