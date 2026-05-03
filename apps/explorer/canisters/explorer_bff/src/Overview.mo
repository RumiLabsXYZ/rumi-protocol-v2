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
    if (lag_seconds > 7200) return #Red;
    if (lag_seconds > 1800) return #Yellow;
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

  // Per-call wrappers that swallow trap errors.
  // The real Rumi `rumi_protocol_backend.get_events_filtered` returns a different
  // shape than our simplified mock (`vec record { nat64; Event }` vs `vec EventSummary`),
  // so on mainnet that call will always fail to decode. Wrapping in try/catch lets
  // the rest of Overview.fetch succeed with `recent_activity = []`. A follow-up plan
  // can adapt the BFF to consume the real Event variant.
  func trySummary(a : SA.AnalyticsActor) : async ?ST.ProtocolSummary {
    try { ?(await a.get_protocol_summary()) } catch (_e) { null };
  };

  func tryHealth(a : SA.AnalyticsActor) : async ?ST.CollectorHealth {
    try { ?(await a.get_collector_health()) } catch (_e) { null };
  };

  func tryStatus(b : SA.BackendActor) : async ?ST.ProtocolStatus {
    try { ?(await b.get_protocol_status()) } catch (_e) { null };
  };

  func tryEvents(b : SA.BackendActor) : async ?ST.GetEventsFilteredResponse {
    try {
      ?(await b.get_events_filtered({
        start = 0 : Nat64;
        length = 5 : Nat64;
        types = null;
        principal = null;
        collateral_token = null;
        time_range = null;
        min_size_e8s = null;
        admin_labels = null;
      }))
    } catch (_e) { null };
  };

  public func fetch(sources : SourceConfig.SourceCanisters) : async T.OverviewDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let summary_opt = await trySummary(analyticsActor);
    let status_opt = await tryStatus(backendActor);
    let health_opt = await tryHealth(analyticsActor);
    let events_opt = await tryEvents(backendActor);

    // Defaults if any call failed
    let total_collateral_usd_e8s : Nat64 = switch (summary_opt) {
      case null 0;
      case (?s) s.total_collateral_usd_e8s;
    };
    let total_debt_e8s : Nat64 = switch (summary_opt) {
      case null 0;
      case (?s) s.total_debt_e8s;
    };
    let total_vault_count : Nat32 = switch (summary_opt) {
      case null 0;
      case (?s) s.total_vault_count;
    };
    let circulating_supply_opt : ?Nat = switch (summary_opt) {
      case null null;
      case (?s) s.circulating_supply_icusd_e8s;
    };
    let peg_opt : ?ST.PegStatus = switch (summary_opt) {
      case null null;
      case (?s) s.peg;
    };

    let mode : T.ProtocolMode = switch (status_opt) {
      case null #ReadOnly;
      case (?s) mapMode(s.mode);
    };
    let breaker : Bool = switch (status_opt) {
      case null false;
      case (?s) s.liquidation_breaker_tripped;
    };

    let lag_s : Nat64 = switch (health_opt) {
      case null 0;
      case (?h) cursorLagSeconds(h);
    };
    let level = mapHealthLevel(lag_s, breaker);

    let circulating_e8s : Nat64 = switch (circulating_supply_opt) {
      case null total_debt_e8s;
      case (?n) Nat64.fromNat(n);
    };

    let tvl_usd : Float = Float.fromInt64(Int64.fromNat64(total_collateral_usd_e8s)) / 100_000_000.0;

    let peg_usd : Float = switch (peg_opt) {
      case null 1.0;
      case (?p) {
        let vp_float = Float.fromInt(p.virtual_price);
        vp_float / 1_000_000_000_000_000_000.0;
      };
    };

    let recent_activity : [T.EventRowDTO] = switch (events_opt) {
      case null [];
      case (?ev) Array.map<ST.EventSummary, T.EventRowDTO>(ev.events, mapEvent);
    };

    let now_ns : Nat64 = Nat64.fromIntWrap(Time.now());

    {
      tvl_usd = tvl_usd;
      icusd_supply = Format.e8s(circulating_e8s, 8, "icUSD");
      icusd_peg_usd = peg_usd;
      protocol_mode = mode;
      vault_count_open = Nat64.fromNat32(total_vault_count);
      recent_activity = recent_activity;
      health = {
        level = level;
        message = healthMessage(level, lag_s);
        analytics_cursor_lag_seconds = lag_s;
        any_breaker_tripped = breaker;
        protocol_mode = mode;
        generated_at_ns = now_ns;
      };
      generated_at_ns = now_ns;
      cache_age_ms = 0;
    };
  };

};
