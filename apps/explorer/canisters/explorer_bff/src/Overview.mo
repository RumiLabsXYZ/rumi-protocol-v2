import Time "mo:core/Time";
import Array "mo:core/Array";
import Float "mo:core/Float";
import Int64 "mo:core/Int64";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func mapMode(m : ST.Mode) : T.ProtocolMode {
    switch m {
      case (#GeneralAvailability) #GeneralAvailability;
      case (#Recovery) #Recovery;
      case (#ReadOnly) #ReadOnly;
    };
  };

  func mapHealthLevel(lag_seconds : Nat64, breaker_tripped : Bool) : T.HealthLevel {
    if (breaker_tripped) return #Red;
    if (lag_seconds > 7200) return #Red;       // > 2h
    if (lag_seconds > 1800) return #Yellow;    // > 30 min
    #Green;
  };

  func nowSeconds() : Nat64 {
    Nat64.fromIntWrap(Time.now() / 1_000_000_000);
  };

  func cursorLagSeconds(health : ST.CollectorHealth) : Nat64 {
    var max_lag : Nat64 = 0;
    let now_s = nowSeconds();
    for (cursor in health.cursors.vals()) {
      let last_s = cursor.last_success_ns / 1_000_000_000;
      let lag : Nat64 = if (now_s > last_s) now_s - last_s else 0;
      if (lag > max_lag) max_lag := lag;
    };
    max_lag;
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

  func healthMessage(level : T.HealthLevel, _lag_s : Nat64) : Text {
    switch level {
      case (#Green) "All systems nominal.";
      case (#Yellow) "Tailer lag elevated.";
      case (#Red) "Critical: tailer or breaker condition active.";
    };
  };

  public func fetch(sources : SourceConfig.SourceCanisters) : async T.OverviewDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    // Fan out four calls in parallel using async blocks
    let summary_fut = async { await analyticsActor.get_protocol_summary() };
    let status_fut = async { await backendActor.get_protocol_status() };
    let health_fut = async { await analyticsActor.get_collector_health() };
    let events_fut = async {
      await backendActor.get_events_filtered({
        start = 0 : Nat64;
        length = 5 : Nat64;
        types = null;
        principal = null;
        collateral_token = null;
        time_range = null;
        min_size_e8s = null;
        admin_labels = null;
      });
    };

    let summary = await summary_fut;
    let status = await status_fut;
    let health = await health_fut;
    let events_resp = await events_fut;

    let circulating_e8s : Nat64 = switch (summary.circulating_supply_icusd_e8s) {
      case null summary.total_debt_e8s;
      case (?n) Nat64.fromNat(n);
    };

    let tvl_usd : Float = Float.fromInt64(Int64.fromNat64(summary.total_collateral_usd_e8s)) / 100_000_000.0;

    let peg_usd : Float = switch (summary.peg) {
      case null 1.0;
      case (?p) {
        let vp_float = Float.fromInt(p.virtual_price);
        vp_float / 1_000_000_000_000_000_000.0;  // virtual_price has 18 decimals
      };
    };

    let lag_s = cursorLagSeconds(health);
    let mode = mapMode(status.mode);
    let level = mapHealthLevel(lag_s, status.liquidation_breaker_tripped);

    let now_ns : Nat64 = Nat64.fromIntWrap(Time.now());

    {
      tvl_usd = tvl_usd;
      icusd_supply = Format.e8s(circulating_e8s, 8, "icUSD");
      icusd_peg_usd = peg_usd;
      protocol_mode = mode;
      vault_count_open = Nat64.fromNat32(summary.total_vault_count);
      recent_activity = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      health = {
        level = level;
        message = healthMessage(level, lag_s);
        analytics_cursor_lag_seconds = lag_s;
        any_breaker_tripped = status.liquidation_breaker_tripped;
        protocol_mode = mode;
        generated_at_ns = now_ns;
      };
      generated_at_ns = now_ns;
      cache_age_ms = 0;
    };
  };

};
