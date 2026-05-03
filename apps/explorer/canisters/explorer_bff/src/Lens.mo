import Array "mo:core/Array";
import Time "mo:core/Time";
import Float "mo:core/Float";
import Int64 "mo:core/Int64";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func e8sToFloat(e8s : Nat64) : Float {
    Float.fromInt64(Int64.fromNat64(e8s)) / 100_000_000.0;
  };

  func mapTvlPoint(p : ST.TvlPoint) : T.TvlPoint {
    {
      timestamp_ns = p.timestamp_ns;
      total_collateral_usd = e8sToFloat(p.total_collateral_usd_e8s);
      vault_count = p.vault_count;
    };
  };

  func mapFeePoint(p : ST.FeePoint) : T.FeePoint {
    {
      timestamp_ns = p.timestamp_ns;
      borrow_fees_usd = e8sToFloat(p.borrow_fees_e8s);
      redemption_fees_usd = e8sToFloat(p.redemption_fees_e8s);
      swap_fees_usd = e8sToFloat(p.swap_fees_e8s);
    };
  };

  func mapRedemptionPoint(p : ST.RedemptionPoint) : T.RedemptionPoint {
    {
      timestamp_ns = p.timestamp_ns;
      count = p.count;
      volume_usd = e8sToFloat(p.volume_e8s);
    };
  };

  func mapSwapPoint(p : ST.SwapPoint) : T.SwapPoint {
    {
      timestamp_ns = p.timestamp_ns;
      count = p.count;
      volume_usd = e8sToFloat(p.volume_e8s);
    };
  };

  func mapStabilityPoint(p : ST.StabilityPoint) : T.StabilityPoint {
    {
      timestamp_ns = p.timestamp_ns;
      total_deposits_usd = e8sToFloat(p.total_deposits_e8s);
      apy_pct = p.apy_pct;
    };
  };

  func mapEvent(ev : ST.EventSummary) : T.EventRowDTO {
    {
      source = "backend";
      source_event_id = ev.global_index;
      global_id = "backend:" # debug_show(ev.global_index);
      kind = ev.kind;
      timestamp_ns = ev.timestamp_ns;
      primary_principal = ev.primary_principal;
      primary_amount = switch (ev.amount_e8s) {
        case null null;
        case (?amt) ?Format.e8s(amt, 8, "");
      };
      secondary_principal = null;
      approximate = false;
      payload_summary = ev.payload_summary;
    };
  };

  func mapMode(m : ST.Mode) : T.ProtocolMode {
    switch m {
      case (#GeneralAvailability) #GeneralAvailability;
      case (#Recovery) #Recovery;
      case (#ReadOnly) #ReadOnly;
    };
  };

  func emptyRange() : ST.RangeQuery {
    { to_ts = null; from_ts = null; offset = null; limit = null };
  };

  public func fetchCollateral(sources : SourceConfig.SourceCanisters) : async T.CollateralLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);
    let summary = await analyticsActor.get_protocol_summary();
    let series_resp = await analyticsActor.get_tvl_series(emptyRange());
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 10;
      types = ?[#OpenVault, #CloseVault, #Borrow, #Repay];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    {
      total_collateral_usd = e8sToFloat(summary.total_collateral_usd_e8s);
      vault_count = summary.total_vault_count;
      system_cr_bps = summary.system_cr_bps;
      tvl_series = Array.map<ST.TvlPoint, T.TvlPoint>(series_resp.rows, mapTvlPoint);
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchStabilityPool(sources : SourceConfig.SourceCanisters) : async T.StabilityPoolLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);
    let summary = await analyticsActor.get_protocol_summary();
    let series_resp = await analyticsActor.get_stability_series(emptyRange());
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 10;
      types = ?[#StabilityPoolDeposit, #StabilityPoolWithdraw];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    let mapped = Array.map<ST.StabilityPoint, T.StabilityPoint>(series_resp.rows, mapStabilityPoint);
    let total_deposits = if (mapped.size() > 0) mapped[mapped.size() - 1].total_deposits_usd else 0.0;
    let apy = switch (summary.sp_apy_pct) {
      case null 0.0;
      case (?v) v;
    };
    {
      total_deposits_usd = total_deposits;
      current_apy_pct = apy;
      series = mapped;
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchRevenue(sources : SourceConfig.SourceCanisters) : async T.RevenueLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);
    let series_resp = await analyticsActor.get_fee_series(emptyRange());
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 10;
      types = ?[#Borrow, #Redemption];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    let mapped = Array.map<ST.FeePoint, T.FeePoint>(series_resp.rows, mapFeePoint);
    var total : Float = 0.0;
    for (p in mapped.vals()) {
      total += p.borrow_fees_usd + p.redemption_fees_usd + p.swap_fees_usd;
    };
    {
      total_fees_30d_usd = total;
      series = mapped;
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchRedemptions(sources : SourceConfig.SourceCanisters) : async T.RedemptionsLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);
    let series_resp = await analyticsActor.get_redemption_series(emptyRange());
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 10;
      types = ?[#Redemption, #ReserveRedemption];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    let mapped = Array.map<ST.RedemptionPoint, T.RedemptionPoint>(series_resp.rows, mapRedemptionPoint);
    var totalCount : Nat32 = 0;
    var totalVol : Float = 0.0;
    for (p in mapped.vals()) {
      totalCount += p.count;
      totalVol += p.volume_usd;
    };
    {
      total_count_30d = totalCount;
      total_volume_30d_usd = totalVol;
      series = mapped;
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchDex(sources : SourceConfig.SourceCanisters) : async T.DexLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);
    let summary = await analyticsActor.get_protocol_summary();
    let series_resp = await analyticsActor.get_swap_series(emptyRange());
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 10;
      types = null;
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    let virtual_price : ?Float = switch (summary.peg) {
      case null null;
      case (?p) ?(Float.fromInt(p.virtual_price) / 1_000_000_000_000_000_000.0);
    };
    {
      swap_count_24h = summary.swap_count_24h;
      volume_24h_usd = e8sToFloat(summary.volume_24h_e8s);
      virtual_price = virtual_price;
      series = Array.map<ST.SwapPoint, T.SwapPoint>(series_resp.rows, mapSwapPoint);
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchAdmin(sources : SourceConfig.SourceCanisters) : async T.AdminLensDTO {
    let backendActor = SA.backend(sources.backend);
    let status = await backendActor.get_protocol_status();
    let events_resp = await backendActor.get_events_filtered({
      start = 0; length = 20;
      types = ?[#Admin, #AdminMint, #AdminSweepToTreasury];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });
    {
      protocol_mode = mapMode(status.mode);
      any_breaker_tripped = status.liquidation_breaker_tripped;
      recent_admin_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
