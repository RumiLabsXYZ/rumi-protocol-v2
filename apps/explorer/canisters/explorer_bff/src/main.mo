import Principal "mo:core/Principal";
import T "Types";
import Stub "Stub";

persistent actor ExplorerBff {

  public query func ping() : async Text {
    "explorer_bff is alive"
  };

  public query func get_health() : async T.HealthSummaryDTO {
    Stub.health()
  };

  public query func get_overview() : async T.OverviewDTO {
    Stub.overview()
  };

  public query func get_activity(filter : T.ActivityFilter, cursor : T.ActivityCursor) : async T.ActivityFeedDTO {
    Stub.activity(filter, cursor)
  };

  public query func get_address(p : Principal) : async T.AddressDTO {
    Stub.address(p)
  };

  public query func get_vault(vault_id : Nat64) : async T.VaultDetailDTO {
    Stub.vault(vault_id)
  };

  public query func get_pool(pool_id : Text) : async T.PoolDetailDTO {
    Stub.pool(pool_id)
  };

  public query func get_token(ledger : Principal) : async T.TokenDetailDTO {
    Stub.token(ledger)
  };

  public query func get_event(global_id : Text) : async T.EventDetailDTO {
    Stub.event(global_id)
  };

};
