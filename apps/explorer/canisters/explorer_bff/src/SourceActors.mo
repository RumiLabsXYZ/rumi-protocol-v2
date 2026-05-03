import Principal "mo:core/Principal";
import T "SourceTypes";

module {

  public type AnalyticsActor = actor {
    get_protocol_summary : () -> async T.ProtocolSummary;
    get_collector_health : () -> async T.CollectorHealth;
    get_address_holdings : (Principal) -> async T.AddressHoldings;
    get_pool_state : (Text) -> async ?T.PoolState;
    get_token_metadata : (Principal) -> async ?T.TokenMetadata;
  };

  public type BackendActor = actor {
    get_protocol_status : () -> async T.ProtocolStatus;
    get_vault_count : () -> async Nat64;
    get_events_filtered : (T.GetEventsFilteredArg) -> async T.GetEventsFilteredResponse;
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
