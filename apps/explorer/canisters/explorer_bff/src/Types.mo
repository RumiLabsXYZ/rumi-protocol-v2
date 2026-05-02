import _Principal "mo:core/Principal";

module {

  public type ProtocolMode = {
    #GeneralAvailability;
    #Recovery;
    #Caution;
    #ReadOnly;
    #Emergency;
  };

  public type VaultStatus = {
    #Open;
    #Closed;
    #Liquidated;
  };

  public type HealthLevel = {
    #Green;
    #Yellow;
    #Red;
  };

  public type FormattedNumber = {
    raw_e8s : Nat64;
    decimals : Nat8;
    formatted : Text;
  };

  public type EventRowDTO = {
    source : Text;
    source_event_id : Nat64;
    global_id : Text;
    kind : Text;
    timestamp_ns : Nat64;
    primary_principal : ?Principal;
    primary_amount : ?FormattedNumber;
    secondary_principal : ?Principal;
    approximate : Bool;
    payload_summary : Text;
  };

  public type HealthSummaryDTO = {
    level : HealthLevel;
    message : Text;
    analytics_cursor_lag_seconds : Nat64;
    any_breaker_tripped : Bool;
    protocol_mode : ProtocolMode;
    generated_at_ns : Nat64;
  };

  public type OverviewDTO = {
    tvl_usd : Float;
    icusd_supply : FormattedNumber;
    icusd_peg_usd : Float;
    protocol_mode : ProtocolMode;
    vault_count_open : Nat64;
    recent_activity : [EventRowDTO];
    health : HealthSummaryDTO;
    generated_at_ns : Nat64;
    cache_age_ms : Nat64;
  };

  public type ActivityFilter = {
    sources : ?[Text];
    types : ?[Text];
    principal : ?Principal;
    from_ns : ?Nat64;
    to_ns : ?Nat64;
  };

  public type ActivityCursor = {
    before_global_id : ?Text;
    page_size : Nat32;
  };

  public type ActivityFeedDTO = {
    events : [EventRowDTO];
    next_cursor : ?Text;
    total_estimated : Nat64;
    filters_applied : ActivityFilter;
  };

  public type VaultSummaryDTO = {
    vault_id : Nat64;
    status : VaultStatus;
    collateral_type : Principal;
    collateral_amount : FormattedNumber;
    debt_icusd : FormattedNumber;
    collateral_ratio : ?Float;
  };

  public type SpDepositDTO = {
    total_deposited : FormattedNumber;
    current_balance : FormattedNumber;
    earned_collateral : [(Principal, FormattedNumber)];
  };

  public type AmmLpPositionDTO = {
    pool_id : Text;
    pool_label : Text;
    shares : FormattedNumber;
    share_value_usd : Float;
    approximate : Bool;
  };

  public type TokenBalanceDTO = {
    ledger : Principal;
    symbol : Text;
    balance : FormattedNumber;
    value_usd : ?Float;
  };

  public type AddressDTO = {
    principal : Principal;
    vaults_owned : [VaultSummaryDTO];
    sp_deposits : [SpDepositDTO];
    amm_lp_positions : [AmmLpPositionDTO];
    token_balances : [TokenBalanceDTO];
    recent_events : [EventRowDTO];
    total_value_usd : Float;
    approximate_sources : [Text];
    generated_at_ns : Nat64;
  };

  public type VaultDetailDTO = {
    vault_id : Nat64;
    status : VaultStatus;
    owner : Principal;
    collateral_type : Principal;
    collateral_amount : FormattedNumber;
    debt_icusd : FormattedNumber;
    collateral_ratio : ?Float;
    history : [EventRowDTO];
    closed_synthesized : Bool;
    generated_at_ns : Nat64;
  };

  public type PoolDetailDTO = {
    pool_id : Text;
    pool_label : Text;
    pool_kind : Text;
    reserves : [(Principal, FormattedNumber)];
    lp_total_supply : FormattedNumber;
    virtual_price : ?Float;
    recent_events : [EventRowDTO];
    generated_at_ns : Nat64;
  };

  public type TokenDetailDTO = {
    ledger : Principal;
    symbol : Text;
    decimals : Nat8;
    total_supply : FormattedNumber;
    fee : FormattedNumber;
    recent_transfers : [EventRowDTO];
    generated_at_ns : Nat64;
  };

  public type EventDetailDTO = {
    global_id : Text;
    source : Text;
    source_event_id : Nat64;
    kind : Text;
    timestamp_ns : Nat64;
    payload_summary : Text;
    payload_json : Text;
    related_event_ids : [Text];
    generated_at_ns : Nat64;
  };

};
