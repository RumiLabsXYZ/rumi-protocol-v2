import Principal "mo:core/Principal";
import T "SourceTypes";

module {

  public type AnalyticsActor = actor {
    get_protocol_summary : () -> async T.ProtocolSummary;
    get_collector_health : () -> async T.CollectorHealth;
    // get_address_holdings, get_pool_state, get_token_metadata not present on real rumi_analytics
    get_tvl_series : (T.RangeQuery) -> async T.TvlSeriesResponse;
    get_fee_series : (T.RangeQuery) -> async T.FeeSeriesResponse;
    // get_redemption_series not present on real rumi_analytics
    get_swap_series : (T.RangeQuery) -> async T.SwapSeriesResponse;
    get_stability_series : (T.RangeQuery) -> async T.StabilitySeriesResponse;
  };

  public type BackendActor = actor {
    get_protocol_status : () -> async T.ProtocolStatus;
    get_vault_count : () -> async Nat64;
    get_events_filtered : (T.GetEventsFilteredArg) -> async T.GetEventsFilteredResponse;
    // get_vault_summary and get_vault_history have different shapes on real backend
    // (returns full Event variant, not EventSummary) — calls are wrapped in try/catch
    get_vault_summary : (Nat64) -> async ?T.VaultSummary;
    get_vault_history : (Nat64) -> async [T.EventSummary];
  };

  public func analytics(id : Principal) : AnalyticsActor {
    actor (Principal.toText(id)) : AnalyticsActor;
  };

  public func backend(id : Principal) : BackendActor {
    actor (Principal.toText(id)) : BackendActor;
  };

};
