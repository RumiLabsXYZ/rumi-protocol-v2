import Array "mo:core/Array";
import Time "mo:core/Time";
import Float "mo:core/Float";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  // ── helpers ──

  func e8sToFloat(e8s : Nat64) : Float {
    Float.fromInt(Nat64.toNat(e8s)) / 100_000_000.0;
  };

  func natToFloat(n : Nat) : Float {
    // Nat is a subtype of Int in Motoko; Float.fromInt accepts it directly
    Float.fromInt(n) / 100_000_000.0;
  };

  func emptyRange() : ST.RangeQuery {
    { to_ts = null; from_ts = null; offset = null; limit = null };
  };

  // mapEvent is only used when get_events_filtered succeeds (i.e. on mock or future real
  // EventSummary support). The real backend returns a full Event variant that doesn't
  // decode into EventSummary, so all events calls are wrapped in try/catch.
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

  // ── safe wrappers (return null on any trap/error) ──

  func trySummary(analyticsActor : SA.AnalyticsActor) : async ?ST.ProtocolSummary {
    try { ?(await analyticsActor.get_protocol_summary()) } catch (_e) { null };
  };

  func tryTvlSeries(analyticsActor : SA.AnalyticsActor) : async ?ST.TvlSeriesResponse {
    try { ?(await analyticsActor.get_tvl_series(emptyRange())) } catch (_e) { null };
  };

  func tryFeeSeries(analyticsActor : SA.AnalyticsActor) : async ?ST.FeeSeriesResponse {
    try { ?(await analyticsActor.get_fee_series(emptyRange())) } catch (_e) { null };
  };

  func trySwapSeries(analyticsActor : SA.AnalyticsActor) : async ?ST.SwapSeriesResponse {
    try { ?(await analyticsActor.get_swap_series(emptyRange())) } catch (_e) { null };
  };

  func tryStabilitySeries(analyticsActor : SA.AnalyticsActor) : async ?ST.StabilitySeriesResponse {
    try { ?(await analyticsActor.get_stability_series(emptyRange())) } catch (_e) { null };
  };

  func tryProtocolStatus(backendActor : SA.BackendActor) : async ?ST.ProtocolStatus {
    try { ?(await backendActor.get_protocol_status()) } catch (_e) { null };
  };

  // Events call: real backend returns full Event variant (not EventSummary), so this
  // will trap on mainnet until the Event variant is ported. Wrapped in try/catch so
  // it degrades to an empty list rather than crashing the lens.
  func tryEvents(
    backendActor : SA.BackendActor,
    arg : ST.GetEventsFilteredArg,
  ) : async [T.EventRowDTO] {
    try {
      let resp = await backendActor.get_events_filtered(arg);
      Array.map<ST.EventSummary, T.EventRowDTO>(resp.events, mapEvent);
    } catch (_e) {
      // Real backend Event variant not yet ported into BFF shadow types.
      // Returns empty list until a follow-up ports the full variant.
      [];
    };
  };

  // ── DailyTvlRow -> T.TvlPoint ──
  // total_icp_collateral_e8s is ICP-denominated (not USD). Without an oracle price we
  // treat it as a proxy for "vault collateral over time" — the chart series is meaningful
  // even if the unit is ICP rather than USD. vault_count is not available per-row;
  // we fill it from summary (constant across the series — best available data).
  func mapTvlRow(row : ST.DailyTvlRow, vault_count : Nat32) : T.TvlPoint {
    {
      timestamp_ns = row.timestamp_ns;
      // Approximate: ICP collateral treated as collateral proxy (not USD-denominated)
      total_collateral_usd = natToFloat(row.total_icp_collateral_e8s);
      vault_count = vault_count;
    };
  };

  // ── DailyFeeRollup -> T.FeePoint ──
  func mapFeeRow(row : ST.DailyFeeRollup) : T.FeePoint {
    let borrow_fees : Float = switch (row.borrowing_fees_e8s) {
      case null 0.0;
      case (?v) e8sToFloat(v);
    };
    let redemption_fees : Float = switch (row.redemption_fees_e8s) {
      case null 0.0;
      case (?v) e8sToFloat(v);
    };
    {
      timestamp_ns = row.timestamp_ns;
      borrow_fees_usd = borrow_fees;
      redemption_fees_usd = redemption_fees;
      swap_fees_usd = e8sToFloat(row.swap_fees_e8s);
    };
  };

  // ── DailySwapRollup -> T.SwapPoint ──
  func mapSwapRow(row : ST.DailySwapRollup) : T.SwapPoint {
    let count_nat32 : Nat32 = row.three_pool_swap_count + row.amm_swap_count;
    let volume_e8s : Nat64 = row.three_pool_volume_e8s + row.amm_volume_e8s;
    {
      timestamp_ns = row.timestamp_ns;
      count = count_nat32;
      volume_usd = e8sToFloat(volume_e8s);
    };
  };

  // ── DailyStabilityRow -> T.StabilityPoint ──
  // apy_pct is not available per-row; filled from summary.sp_apy_pct (constant).
  func mapStabilityRow(row : ST.DailyStabilityRow, apy_pct : Float) : T.StabilityPoint {
    {
      timestamp_ns = row.timestamp_ns;
      total_deposits_usd = e8sToFloat(row.total_deposits_e8s);
      apy_pct = apy_pct;
    };
  };

  // ── lens fetchers ──

  public func fetchCollateral(sources : SourceConfig.SourceCanisters) : async T.CollateralLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let summary_opt = await trySummary(analyticsActor);
    let series_opt = await tryTvlSeries(analyticsActor);
    let recent_events = await tryEvents(backendActor, {
      start = 0; length = 10;
      types = ?[#OpenVault, #CloseVault, #Borrow, #Repay];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let vault_count : Nat32 = switch (summary_opt) {
      case null 0;
      case (?s) s.total_vault_count;
    };
    let total_collateral_usd : Float = switch (summary_opt) {
      case null 0.0;
      case (?s) Float.fromInt(Nat64.toNat(s.total_collateral_usd_e8s)) / 100_000_000.0;
    };
    let system_cr_bps : Nat32 = switch (summary_opt) {
      case null 0;
      case (?s) s.system_cr_bps;
    };
    let tvl_series : [T.TvlPoint] = switch (series_opt) {
      case null [];
      case (?resp) Array.map<ST.DailyTvlRow, T.TvlPoint>(
        resp.rows,
        func(row) { mapTvlRow(row, vault_count) },
      );
    };

    {
      total_collateral_usd = total_collateral_usd;
      vault_count = vault_count;
      system_cr_bps = system_cr_bps;
      tvl_series = tvl_series;
      recent_events = recent_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchStabilityPool(sources : SourceConfig.SourceCanisters) : async T.StabilityPoolLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let summary_opt = await trySummary(analyticsActor);
    let series_opt = await tryStabilitySeries(analyticsActor);
    let recent_events = await tryEvents(backendActor, {
      start = 0; length = 10;
      types = ?[#StabilityPoolDeposit, #StabilityPoolWithdraw];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let apy : Float = switch (summary_opt) {
      case null 0.0;
      case (?s) switch (s.sp_apy_pct) { case null 0.0; case (?v) v };
    };

    let mapped : [T.StabilityPoint] = switch (series_opt) {
      case null [];
      case (?resp) Array.map<ST.DailyStabilityRow, T.StabilityPoint>(
        resp.rows,
        func(row) { mapStabilityRow(row, apy) },
      );
    };

    let total_deposits : Float =
      if (mapped.size() > 0) mapped[mapped.size() - 1].total_deposits_usd
      else 0.0;

    {
      total_deposits_usd = total_deposits;
      current_apy_pct = apy;
      series = mapped;
      recent_events = recent_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchRevenue(sources : SourceConfig.SourceCanisters) : async T.RevenueLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let series_opt = await tryFeeSeries(analyticsActor);
    let recent_events = await tryEvents(backendActor, {
      start = 0; length = 10;
      types = ?[#Borrow, #Redemption];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let mapped : [T.FeePoint] = switch (series_opt) {
      case null [];
      case (?resp) Array.map<ST.DailyFeeRollup, T.FeePoint>(resp.rows, mapFeeRow);
    };

    var total : Float = 0.0;
    for (p in mapped.vals()) {
      total += p.borrow_fees_usd + p.redemption_fees_usd + p.swap_fees_usd;
    };

    {
      total_fees_30d_usd = total;
      series = mapped;
      recent_events = recent_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchRedemptions(sources : SourceConfig.SourceCanisters) : async T.RedemptionsLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    // No get_redemption_series on real analytics. Synthesize from DailyFeeRollup:
    // redemption_count gives daily counts, redemption_fees_e8s gives fee totals.
    // We surface count-only series (volume_usd = 0 since we have fees not volume).
    let series_opt = await tryFeeSeries(analyticsActor);
    let recent_events = await tryEvents(backendActor, {
      start = 0; length = 10;
      types = ?[#Redemption, #ReserveRedemption];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let mapped : [T.RedemptionPoint] = switch (series_opt) {
      case null [];
      case (?resp) Array.map<ST.DailyFeeRollup, T.RedemptionPoint>(
        resp.rows,
        func(row) {
          {
            timestamp_ns = row.timestamp_ns;
            count = row.redemption_count;
            // volume_usd is not available without a redemption fee rate;
            // use 0 and display count-only in the chart label
            volume_usd = 0.0;
          };
        },
      );
    };

    var totalCount : Nat32 = 0;
    for (p in mapped.vals()) {
      totalCount += p.count;
    };

    {
      total_count_30d = totalCount;
      total_volume_30d_usd = 0.0; // not available without full Event variant
      series = mapped;
      recent_events = recent_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchDex(sources : SourceConfig.SourceCanisters) : async T.DexLensDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let summary_opt = await trySummary(analyticsActor);
    let series_opt = await trySwapSeries(analyticsActor);
    let recent_events = await tryEvents(backendActor, {
      start = 0; length = 10;
      types = null;
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let swap_count_24h : Nat32 = switch (summary_opt) {
      case null 0;
      case (?s) s.swap_count_24h;
    };
    let volume_24h_usd : Float = switch (summary_opt) {
      case null 0.0;
      case (?s) e8sToFloat(s.volume_24h_e8s);
    };
    let virtual_price : ?Float = switch (summary_opt) {
      case null null;
      case (?s) switch (s.peg) {
        case null null;
        case (?p) ?(Float.fromInt(p.virtual_price) / 1_000_000_000_000_000_000.0);
      };
    };
    let series : [T.SwapPoint] = switch (series_opt) {
      case null [];
      case (?resp) Array.map<ST.DailySwapRollup, T.SwapPoint>(resp.rows, mapSwapRow);
    };

    {
      swap_count_24h = swap_count_24h;
      volume_24h_usd = volume_24h_usd;
      virtual_price = virtual_price;
      series = series;
      recent_events = recent_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetchAdmin(sources : SourceConfig.SourceCanisters) : async T.AdminLensDTO {
    let backendActor = SA.backend(sources.backend);

    let status_opt = await tryProtocolStatus(backendActor);
    let recent_admin_events = await tryEvents(backendActor, {
      start = 0; length = 20;
      types = ?[#Admin, #AdminMint, #AdminSweepToTreasury];
      principal = null; collateral_token = null; time_range = null; min_size_e8s = null; admin_labels = null;
    });

    let protocol_mode : T.ProtocolMode = switch (status_opt) {
      case null #ReadOnly;
      case (?s) mapMode(s.mode);
    };
    let any_breaker_tripped : Bool = switch (status_opt) {
      case null false;
      case (?s) s.liquidation_breaker_tripped;
    };

    {
      protocol_mode = protocol_mode;
      any_breaker_tripped = any_breaker_tripped;
      recent_admin_events = recent_admin_events;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
