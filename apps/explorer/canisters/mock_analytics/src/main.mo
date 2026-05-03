import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";

persistent actor MockAnalytics {

  type PegStatus = {
    virtual_price : Nat;
    timestamp_ns : Nat64;
    pool_balances : [Nat];
    balance_ratios : [Float];
    max_imbalance_pct : Float;
  };

  type TwapEntry = {
    asset : Principal;
    price_usd : Float;
    window_seconds : Nat32;
  };

  type ProtocolSummary = {
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

  type CursorStatus = {
    last_error : ?Text;
    source_count : Nat64;
    name : Text;
    last_success_ns : Nat64;
    cursor_position : Nat64;
  };

  type ErrorCounters = {
    backend : Nat64;
    three_pool : Nat64;
    amm : Nat64;
    stability_pool : Nat64;
    liquidation_bot : Nat64;
  };

  type BalanceTrackerStats = {
    ledger : Principal;
    tracked_principals : Nat64;
    last_refresh_ns : Nat64;
  };

  type CollectorHealth = {
    balance_tracker_stats : [BalanceTrackerStats];
    backfill_active : [Principal];
    error_counters : ErrorCounters;
    last_pull_cycle_ns : Nat64;
    cursors : [CursorStatus];
  };

  func samplePeg() : PegStatus {
    {
      virtual_price = 1_063_100_000_000_000_000;
      timestamp_ns = 1_730_000_000_000_000_000;
      pool_balances = [333_333_00000000, 333_333_00000000, 333_334_00000000];
      balance_ratios = [0.3333, 0.3333, 0.3334];
      max_imbalance_pct = 0.0001;
    };
  };

  public query func get_protocol_summary() : async ProtocolSummary {
    {
      peg = ?samplePeg();
      lp_apy_pct = ?12.4;
      timestamp_ns = 1_730_000_000_000_000_000;
      sp_apy_pct = ?8.7;
      total_debt_e8s = 500_000_00000000;
      circulating_supply_icusd_e8s = ?500_000_00000000;
      prices = [];
      total_vault_count = 142;
      total_collateral_usd_e8s = 1_234_567_89000000;
      system_cr_bps = 24700;
      swap_count_24h = 47;
      volume_24h_e8s = 850_000_00000000;
    };
  };

  public query func get_collector_health() : async CollectorHealth {
    let now_ns : Nat64 = Nat64.fromIntWrap(Time.now());
    {
      balance_tracker_stats = [];
      backfill_active = [];
      error_counters = {
        backend = 0;
        three_pool = 0;
        amm = 0;
        stability_pool = 0;
        liquidation_bot = 0;
      };
      last_pull_cycle_ns = now_ns;
      cursors = [
        {
          last_error = null;
          source_count = 1234;
          name = "backend";
          last_success_ns = now_ns;
          cursor_position = 1234;
        },
      ];
    };
  };

};
