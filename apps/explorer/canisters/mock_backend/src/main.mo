import Principal "mo:core/Principal";
import Array "mo:core/Array";
import Nat64 "mo:core/Nat64";

persistent actor MockBackend {

  type Mode = { #ReadOnly; #GeneralAvailability; #Recovery };

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

  func sampleEvent(idx : Nat64, kind : Text, summary : Text, amount : Nat64) : EventSummary {
    {
      global_index = idx;
      kind = kind;
      timestamp_ns = 1_730_000_000_000_000_000 - idx * 60_000_000_000;
      primary_principal = null;
      amount_e8s = ?amount;
      payload_summary = summary;
    };
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
    let all : [EventSummary] = [
      sampleEvent(0, "open_vault", "Vault #142 opened with 1.5 ICP collateral", 1_50000000),
      sampleEvent(1, "borrow", "Borrowed 50 icUSD from vault #142", 50_00000000),
      sampleEvent(2, "stability_pool_deposit", "Deposited 1000 icUSD to SP", 1000_00000000),
      sampleEvent(3, "redemption", "Redeemed 200 icUSD against vault #97", 200_00000000),
      sampleEvent(4, "partial_liquidation", "Liquidated 0.3 ICP from vault #88", 30000000),
    ];
    let totalNat = all.size();
    let total = Nat64.fromNat(totalNat);
    let length : Nat64 = if (arg.length > 100) 100 else arg.length;
    let startNat = Nat64.toNat(arg.start);
    let lengthNat = Nat64.toNat(length);
    let start = if (startNat >= totalNat) totalNat else startNat;
    let end = if (start + lengthNat >= totalNat) totalNat else start + lengthNat;
    let slice = Array.tabulate<EventSummary>(end - start, func(i) {
      all[start + i];
    });
    {
      total = total;
      events = slice;
    };
  };

};
