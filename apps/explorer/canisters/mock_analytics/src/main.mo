import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat32 "mo:core/Nat32";
import Nat64 "mo:core/Nat64";
import Array "mo:core/Array";
import Float "mo:core/Float";

persistent actor MockAnalytics {

  type TokenBalance = {
    ledger : Principal;
    symbol : Text;
    balance_e8s : Nat64;
    decimals : Nat8;
  };

  type SpDeposit = {
    total_deposited_e8s : Nat64;
    current_balance_e8s : Nat64;
    earned_collateral : [(Principal, Nat64)];
  };

  type AddressHoldings = {
    owner : Principal;
    vaults_owned_ids : [Nat64];
    sp_deposits : [SpDeposit];
    token_balances : [TokenBalance];
    total_value_usd : Float;
  };

  type PoolState = {
    pool_id : Text;
    pool_label : Text;
    pool_kind : Text;
    reserves : [(Principal, Nat64, Nat8)];
    lp_total_supply_e8s : Nat64;
    virtual_price : ?Float;
  };

  type TokenMetadata = {
    ledger : Principal;
    symbol : Text;
    decimals : Nat8;
    total_supply_e8s : Nat64;
    fee_e8s : Nat64;
  };

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

  public query func get_address_holdings(p : Principal) : async AddressHoldings {
    let known = Principal.fromText("tfesu-vyaaa-aaaap-qrd7a-cai");
    let icusd_ledger = Principal.fromText("t6bor-paaaa-aaaap-qrd5q-cai");
    let icp_ledger = Principal.fromText("ryjl3-tyaaa-aaaaa-aaaba-cai");
    if (p == known) {
      {
        owner = p;
        vaults_owned_ids = [142, 138];
        sp_deposits = [{
          total_deposited_e8s = 1000_00000000;
          current_balance_e8s = 987_50000000;
          earned_collateral = [(icp_ledger, 5_000_000)];
        }];
        token_balances = [{
          ledger = icusd_ledger;
          symbol = "icUSD";
          balance_e8s = 250_00000000;
          decimals = 8;
        }];
        total_value_usd = 1234.56;
      };
    } else {
      {
        owner = p;
        vaults_owned_ids = [];
        sp_deposits = [];
        token_balances = [];
        total_value_usd = 0.0;
      };
    };
  };

  public query func get_pool_state(pool_id : Text) : async ?PoolState {
    let icusd = Principal.fromText("t6bor-paaaa-aaaap-qrd5q-cai");
    let usdc  = Principal.fromText("xevnm-gaaaa-aaaar-qafnq-cai");
    let usdt  = Principal.fromText("cngnf-vqaaa-aaaar-qag4q-cai");
    let threeusd = Principal.fromText("fohh4-yyaaa-aaaap-qtkpa-cai");
    let icp   = Principal.fromText("ryjl3-tyaaa-aaaaa-aaaba-cai");
    if (pool_id == "3pool") {
      ?{
        pool_id = "3pool";
        pool_label = "Rumi 3Pool";
        pool_kind = "stable";
        reserves = [
          (icusd, 333_333_00000000, 8),
          (usdc,  333_333_00000000, 8),
          (usdt,  333_334_00000000, 8),
        ];
        lp_total_supply_e8s = 2_500_000_00000000;
        virtual_price = ?1.0631;
      };
    } else if (pool_id == "amm-3usd-icp") {
      ?{
        pool_id = "amm-3usd-icp";
        pool_label = "AMM 3USD/ICP";
        pool_kind = "constant-product";
        reserves = [
          (threeusd, 50_000_00000000, 8),
          (icp,      5_000_00000000, 8),
        ];
        lp_total_supply_e8s = 100_000_00000000;
        virtual_price = null;
      };
    } else {
      null;
    };
  };

  // ── Series types ──

  type RangeQuery = {
    to_ts : ?Nat64;
    from_ts : ?Nat64;
    offset : ?Nat64;
    limit : ?Nat32;
  };

  type TvlPoint = {
    timestamp_ns : Nat64;
    total_collateral_usd_e8s : Nat64;
    vault_count : Nat32;
  };

  type TvlSeriesResponse = { rows : [TvlPoint] };

  type FeePoint = {
    timestamp_ns : Nat64;
    borrow_fees_e8s : Nat64;
    redemption_fees_e8s : Nat64;
    swap_fees_e8s : Nat64;
  };

  type FeeSeriesResponse = { rows : [FeePoint] };

  type RedemptionPoint = {
    timestamp_ns : Nat64;
    count : Nat32;
    volume_e8s : Nat64;
  };

  type RedemptionSeriesResponse = { rows : [RedemptionPoint] };

  type SwapPoint = {
    timestamp_ns : Nat64;
    count : Nat32;
    volume_e8s : Nat64;
  };

  type SwapSeriesResponse = { rows : [SwapPoint] };

  type StabilityPoint = {
    timestamp_ns : Nat64;
    total_deposits_e8s : Nat64;
    apy_pct : Float;
  };

  type StabilitySeriesResponse = { rows : [StabilityPoint] };

  // ── Series helpers ──

  let DAY_NS : Nat64 = 86_400_000_000_000;

  func dayTs(daysAgo : Nat64) : Nat64 {
    let now = Nat64.fromIntWrap(Time.now());
    if (daysAgo * DAY_NS > now) 0 else now - daysAgo * DAY_NS;
  };

  // ── Series endpoints ──

  public query func get_tvl_series(_q : RangeQuery) : async TvlSeriesResponse {
    let rows = Array.tabulate<TvlPoint>(30, func(i) {
      let daysAgo : Nat64 = Nat64.fromNat(29 - i);
      {
        timestamp_ns = dayTs(daysAgo);
        total_collateral_usd_e8s = 800_000_00000000 + Nat64.fromNat(i) * 14_400_000_000_000;
        vault_count = Nat32.fromNat(120 + i);
      };
    });
    { rows = rows };
  };

  public query func get_fee_series(_q : RangeQuery) : async FeeSeriesResponse {
    // borrow: ~80-200 icUSD e8s per day, redemption: ~20-100, swap: ~50-150
    let rows = Array.tabulate<FeePoint>(30, func(i) {
      let daysAgo : Nat64 = Nat64.fromNat(29 - i);
      {
        timestamp_ns = dayTs(daysAgo);
        borrow_fees_e8s = 80_00000000 + Nat64.fromNat(i) * 4_00000000;
        redemption_fees_e8s = 20_00000000 + Nat64.fromNat(i) * 2_66666666;
        swap_fees_e8s = 50_00000000 + Nat64.fromNat(i) * 3_33333333;
      };
    });
    { rows = rows };
  };

  public query func get_redemption_series(_q : RangeQuery) : async RedemptionSeriesResponse {
    // count: 0-3 per day, volume: 0-500 icUSD
    let rows = Array.tabulate<RedemptionPoint>(30, func(i) {
      let daysAgo : Nat64 = Nat64.fromNat(29 - i);
      {
        timestamp_ns = dayTs(daysAgo);
        count = Nat32.fromNat(i % 4);
        volume_e8s = Nat64.fromNat(i % 4) * 166_66666666;
      };
    });
    { rows = rows };
  };

  public query func get_swap_series(_q : RangeQuery) : async SwapSeriesResponse {
    // count: 5-15 per day, volume: 200-1000 icUSD
    let rows = Array.tabulate<SwapPoint>(30, func(i) {
      let daysAgo : Nat64 = Nat64.fromNat(29 - i);
      {
        timestamp_ns = dayTs(daysAgo);
        count = Nat32.fromNat(5 + (i * 10 / 29));
        volume_e8s = 200_00000000 + Nat64.fromNat(i) * 26_66666666;
      };
    });
    { rows = rows };
  };

  public query func get_stability_series(_q : RangeQuery) : async StabilitySeriesResponse {
    // total_deposits: 4500-6500 icUSD, apy: 6-10%
    let rows = Array.tabulate<StabilityPoint>(30, func(i) {
      let daysAgo : Nat64 = Nat64.fromNat(29 - i);
      {
        timestamp_ns = dayTs(daysAgo);
        total_deposits_e8s = 4500_00000000 + Nat64.fromNat(i) * 68_96551724;
        apy_pct = 6.0 + Float.fromInt(i) * (4.0 / 29.0);
      };
    });
    { rows = rows };
  };

  public query func get_token_metadata(ledger : Principal) : async ?TokenMetadata {
    let icusd = Principal.fromText("t6bor-paaaa-aaaap-qrd5q-cai");
    if (ledger == icusd) {
      ?{
        ledger = icusd;
        symbol = "icUSD";
        decimals = 8;
        total_supply_e8s = 500_000_00000000;
        fee_e8s = 10_000;
      };
    } else {
      null;
    };
  };

};
