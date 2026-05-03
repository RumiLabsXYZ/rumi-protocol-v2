import Principal "mo:core/Principal";
import Array "mo:core/Array";
import Nat64 "mo:core/Nat64";
import Text "mo:core/Text";

persistent actor MockBackend {

  type Mode = { #ReadOnly; #GeneralAvailability; #Recovery };

  type VaultStatus = { #Open; #Closed; #Liquidated };

  type VaultSummary = {
    vault_id : Nat64;
    status : VaultStatus;
    owner : Principal;
    collateral_type : Principal;
    collateral_amount_e8s : Nat64;
    debt_icusd_e8s : Nat64;
    collateral_ratio : ?Float;
  };

  type ProtocolStatus = {
    mode : Mode;
    total_icusd_borrowed : Nat64;
    total_collateral_ratio : Float;
    liquidation_breaker_tripped : Bool;
    manual_mode_override : Bool;
    snapshot_ts_ns : Nat64;
  };

  type EventTypeFilter = {
    #OpenVault; #CloseVault; #AdjustVault;
    #Borrow; #Repay;
    #Liquidation; #PartialLiquidation;
    #Redemption; #ReserveRedemption;
    #StabilityPoolDeposit; #StabilityPoolWithdraw;
    #AdminMint; #AdminSweepToTreasury;
    #Admin;
    #PriceUpdate; #AccrueInterest;
  };

  type TimeRange = { from_ns : Nat64; to_ns : Nat64 };

  type GetEventsFilteredArg = {
    start : Nat64;
    length : Nat64;
    types : ?[EventTypeFilter];
    principal : ?Principal;
    collateral_token : ?Principal;
    time_range : ?TimeRange;
    min_size_e8s : ?Nat64;
    admin_labels : ?[Text];
  };

  type EventSummary = {
    global_index : Nat64;
    kind : Text;
    timestamp_ns : Nat64;
    primary_principal : ?Principal;
    amount_e8s : ?Nat64;
    payload_summary : Text;
  };

  type GetEventsFilteredResponse = {
    total : Nat64;
    events : [EventSummary];
  };

  // Sample event pool, newest-first (high index = most recent timestamp)
  func sampleEvents() : [EventSummary] {
    let base_ts : Nat64 = 1_730_000_000_000_000_000;
    [
      { global_index = 29; kind = "open_vault"; timestamp_ns = base_ts; primary_principal = null; amount_e8s = ?1_50000000; payload_summary = "Vault #142 opened with 1.5 ICP collateral" },
      { global_index = 28; kind = "borrow"; timestamp_ns = base_ts - 60_000_000_000; primary_principal = null; amount_e8s = ?50_00000000; payload_summary = "Borrowed 50 icUSD from vault #142" },
      { global_index = 27; kind = "stability_pool_deposit"; timestamp_ns = base_ts - 120_000_000_000; primary_principal = null; amount_e8s = ?1000_00000000; payload_summary = "Deposited 1000 icUSD to SP" },
      { global_index = 26; kind = "redemption"; timestamp_ns = base_ts - 300_000_000_000; primary_principal = null; amount_e8s = ?200_00000000; payload_summary = "Redeemed 200 icUSD against vault #97" },
      { global_index = 25; kind = "partial_liquidation"; timestamp_ns = base_ts - 600_000_000_000; primary_principal = null; amount_e8s = ?30000000; payload_summary = "Liquidated 0.3 ICP from vault #88" },
      { global_index = 24; kind = "borrow"; timestamp_ns = base_ts - 900_000_000_000; primary_principal = null; amount_e8s = ?75_00000000; payload_summary = "Borrowed 75 icUSD from vault #138" },
      { global_index = 23; kind = "open_vault"; timestamp_ns = base_ts - 1_200_000_000_000; primary_principal = null; amount_e8s = ?2_00000000; payload_summary = "Vault #141 opened with 2.0 ICP collateral" },
      { global_index = 22; kind = "stability_pool_withdraw"; timestamp_ns = base_ts - 1_800_000_000_000; primary_principal = null; amount_e8s = ?500_00000000; payload_summary = "Withdrew 500 icUSD from SP" },
      { global_index = 21; kind = "repay"; timestamp_ns = base_ts - 2_400_000_000_000; primary_principal = null; amount_e8s = ?25_00000000; payload_summary = "Repaid 25 icUSD to vault #135" },
      { global_index = 20; kind = "redemption"; timestamp_ns = base_ts - 3_000_000_000_000; primary_principal = null; amount_e8s = ?500_00000000; payload_summary = "Redeemed 500 icUSD against vault #102" },
      { global_index = 19; kind = "borrow"; timestamp_ns = base_ts - 3_600_000_000_000; primary_principal = null; amount_e8s = ?100_00000000; payload_summary = "Borrowed 100 icUSD from vault #140" },
      { global_index = 18; kind = "liquidation"; timestamp_ns = base_ts - 4_500_000_000_000; primary_principal = null; amount_e8s = ?1_20000000; payload_summary = "Fully liquidated vault #76" },
      { global_index = 17; kind = "stability_pool_deposit"; timestamp_ns = base_ts - 5_400_000_000_000; primary_principal = null; amount_e8s = ?5000_00000000; payload_summary = "Deposited 5,000 icUSD to SP" },
      { global_index = 16; kind = "close_vault"; timestamp_ns = base_ts - 6_300_000_000_000; primary_principal = null; amount_e8s = null; payload_summary = "Closed vault #134" },
      { global_index = 15; kind = "open_vault"; timestamp_ns = base_ts - 7_200_000_000_000; primary_principal = null; amount_e8s = ?3_50000000; payload_summary = "Vault #140 opened with 3.5 ICP collateral" },
      { global_index = 14; kind = "borrow"; timestamp_ns = base_ts - 9_000_000_000_000; primary_principal = null; amount_e8s = ?40_00000000; payload_summary = "Borrowed 40 icUSD from vault #139" },
      { global_index = 13; kind = "redemption"; timestamp_ns = base_ts - 10_800_000_000_000; primary_principal = null; amount_e8s = ?150_00000000; payload_summary = "Redeemed 150 icUSD against vault #105" },
      { global_index = 12; kind = "partial_liquidation"; timestamp_ns = base_ts - 12_600_000_000_000; primary_principal = null; amount_e8s = ?20000000; payload_summary = "Liquidated 0.2 ICP from vault #82" },
      { global_index = 11; kind = "stability_pool_deposit"; timestamp_ns = base_ts - 14_400_000_000_000; primary_principal = null; amount_e8s = ?2500_00000000; payload_summary = "Deposited 2,500 icUSD to SP" },
      { global_index = 10; kind = "borrow"; timestamp_ns = base_ts - 16_200_000_000_000; primary_principal = null; amount_e8s = ?60_00000000; payload_summary = "Borrowed 60 icUSD from vault #137" },
      { global_index = 9; kind = "repay"; timestamp_ns = base_ts - 18_000_000_000_000; primary_principal = null; amount_e8s = ?15_00000000; payload_summary = "Repaid 15 icUSD to vault #131" },
      { global_index = 8; kind = "open_vault"; timestamp_ns = base_ts - 19_800_000_000_000; primary_principal = null; amount_e8s = ?1_00000000; payload_summary = "Vault #139 opened with 1.0 ICP collateral" },
      { global_index = 7; kind = "redemption"; timestamp_ns = base_ts - 21_600_000_000_000; primary_principal = null; amount_e8s = ?300_00000000; payload_summary = "Redeemed 300 icUSD against vault #108" },
      { global_index = 6; kind = "stability_pool_withdraw"; timestamp_ns = base_ts - 23_400_000_000_000; primary_principal = null; amount_e8s = ?1000_00000000; payload_summary = "Withdrew 1,000 icUSD from SP" },
      { global_index = 5; kind = "borrow"; timestamp_ns = base_ts - 25_200_000_000_000; primary_principal = null; amount_e8s = ?80_00000000; payload_summary = "Borrowed 80 icUSD from vault #136" },
      { global_index = 4; kind = "liquidation"; timestamp_ns = base_ts - 27_000_000_000_000; primary_principal = null; amount_e8s = ?2_00000000; payload_summary = "Fully liquidated vault #71" },
      { global_index = 3; kind = "open_vault"; timestamp_ns = base_ts - 28_800_000_000_000; primary_principal = null; amount_e8s = ?5_00000000; payload_summary = "Vault #138 opened with 5.0 ICP collateral" },
      { global_index = 2; kind = "borrow"; timestamp_ns = base_ts - 30_600_000_000_000; primary_principal = null; amount_e8s = ?125_00000000; payload_summary = "Borrowed 125 icUSD from vault #135" },
      { global_index = 1; kind = "stability_pool_deposit"; timestamp_ns = base_ts - 32_400_000_000_000; primary_principal = null; amount_e8s = ?10000_00000000; payload_summary = "Deposited 10,000 icUSD to SP" },
      { global_index = 0; kind = "open_vault"; timestamp_ns = base_ts - 34_200_000_000_000; primary_principal = null; amount_e8s = ?1_75000000; payload_summary = "Vault #134 opened with 1.75 ICP collateral" },
    ];
  };

  func kindToFilter(kind : Text) : ?EventTypeFilter {
    switch kind {
      case "open_vault" ?#OpenVault;
      case "close_vault" ?#CloseVault;
      case "adjust_vault" ?#AdjustVault;
      case "borrow" ?#Borrow;
      case "repay" ?#Repay;
      case "liquidation" ?#Liquidation;
      case "partial_liquidation" ?#PartialLiquidation;
      case "redemption" ?#Redemption;
      case "reserve_redemption" ?#ReserveRedemption;
      case "stability_pool_deposit" ?#StabilityPoolDeposit;
      case "stability_pool_withdraw" ?#StabilityPoolWithdraw;
      case "admin_mint" ?#AdminMint;
      case "admin_sweep_to_treasury" ?#AdminSweepToTreasury;
      case "price_update" ?#PriceUpdate;
      case "accrue_interest" ?#AccrueInterest;
      case _ null;
    };
  };

  func filterEvent(ev : EventSummary, arg : GetEventsFilteredArg) : Bool {
    // types filter
    switch (arg.types) {
      case null {};
      case (?types) {
        switch (kindToFilter(ev.kind)) {
          case null return false;
          case (?evFilter) {
            var matched = false;
            for (t in types.vals()) {
              if (t == evFilter) matched := true;
            };
            if (not matched) return false;
          };
        };
      };
    };
    // time_range filter
    switch (arg.time_range) {
      case null {};
      case (?tr) {
        if (ev.timestamp_ns < tr.from_ns or ev.timestamp_ns > tr.to_ns) return false;
      };
    };
    // min_size_e8s filter
    switch (arg.min_size_e8s) {
      case null {};
      case (?min) {
        switch (ev.amount_e8s) {
          case null return false;
          case (?amt) { if (amt < min) return false };
        };
      };
    };
    // principal / collateral_token / admin_labels: stubbed (no-op for now)
    true;
  };

  public query func get_protocol_status() : async ProtocolStatus {
    {
      mode = #GeneralAvailability;
      total_icusd_borrowed = 500_000_00000000;
      total_collateral_ratio = 2.47;
      liquidation_breaker_tripped = false;
      manual_mode_override = false;
      snapshot_ts_ns = 1_730_000_000_000_000_000;
    };
  };

  public query func get_vault_count() : async Nat64 {
    142;
  };

  public query func get_events_filtered(arg : GetEventsFilteredArg) : async GetEventsFilteredResponse {
    let pool = sampleEvents();
    let matches = Array.filter<EventSummary>(pool, func(ev) { filterEvent(ev, arg) });
    let totalNat = matches.size();
    let total = Nat64.fromNat(totalNat);

    let length : Nat64 = if (arg.length > 100) 100 else arg.length;
    let startNat = Nat64.toNat(arg.start);
    let lengthNat = Nat64.toNat(length);
    let start = if (startNat >= totalNat) totalNat else startNat;
    let end = if (start + lengthNat >= totalNat) totalNat else start + lengthNat;
    let slice = Array.tabulate<EventSummary>(end - start, func(i) {
      matches[start + i];
    });

    {
      total = total;
      events = slice;
    };
  };

  // Vault 134 history: hardcoded events for the closed-vault synthesis path.
  func vault134History() : [EventSummary] {
    let base_ts : Nat64 = 1_730_000_000_000_000_000;
    [
      { global_index = 0; kind = "open_vault"; timestamp_ns = base_ts - 34_200_000_000_000; primary_principal = null; amount_e8s = ?1_75000000; payload_summary = "Vault #134 opened with 1.75 ICP collateral" },
      { global_index = 2; kind = "borrow"; timestamp_ns = base_ts - 30_600_000_000_000; primary_principal = null; amount_e8s = ?125_00000000; payload_summary = "Borrowed 125 icUSD from Vault #134" },
      { global_index = 9; kind = "repay"; timestamp_ns = base_ts - 18_000_000_000_000; primary_principal = null; amount_e8s = ?125_00000000; payload_summary = "Repaid 125 icUSD to Vault #134" },
      { global_index = 16; kind = "close_vault"; timestamp_ns = base_ts - 6_300_000_000_000; primary_principal = null; amount_e8s = null; payload_summary = "Closed Vault #134" },
    ];
  };

  // Vault 142 history: 3 recent events from the sample pool referencing vault #142.
  func vault142History() : [EventSummary] {
    let base_ts : Nat64 = 1_730_000_000_000_000_000;
    [
      { global_index = 29; kind = "open_vault"; timestamp_ns = base_ts; primary_principal = null; amount_e8s = ?1_50000000; payload_summary = "Vault #142 opened with 1.5 ICP collateral" },
      { global_index = 28; kind = "borrow"; timestamp_ns = base_ts - 60_000_000_000; primary_principal = null; amount_e8s = ?50_00000000; payload_summary = "Borrowed 50 icUSD from vault #142" },
    ];
  };

  public query func get_vault_summary(vault_id : Nat64) : async ?VaultSummary {
    let owner = Principal.fromText("tfesu-vyaaa-aaaap-qrd7a-cai");
    let collateral_type = Principal.fromText("t6bor-paaaa-aaaap-qrd5q-cai");
    if (vault_id == 142) {
      ?{
        vault_id = 142;
        status = #Open;
        owner = owner;
        collateral_type = collateral_type;
        collateral_amount_e8s = 1_50000000;
        debt_icusd_e8s = 50_00000000;
        collateral_ratio = ?2.85;
      };
    } else if (vault_id == 138) {
      ?{
        vault_id = 138;
        status = #Open;
        owner = owner;
        collateral_type = collateral_type;
        collateral_amount_e8s = 5_00000000;
        debt_icusd_e8s = 125_00000000;
        collateral_ratio = ?4.0;
      };
    } else {
      // vault 134 is closed — returns null to trigger synthesis path
      null;
    };
  };

  public query func get_vault_history(vault_id : Nat64) : async [EventSummary] {
    if (vault_id == 134) {
      vault134History();
    } else if (vault_id == 142) {
      vault142History();
    } else {
      [];
    };
  };

};
