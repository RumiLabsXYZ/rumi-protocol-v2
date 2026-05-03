import Principal "mo:core/Principal";
import T "Types";
import Stub "Stub";
import SourceConfig "SourceConfig";

persistent actor class ExplorerBff(initArgs : SourceConfig.SourceCanistersInit) {

  // Anonymous principal as default admin for local development.
  // Replace with real admin principal in Plan 6's mainnet config.
  let admin : Principal = Principal.fromText("2vxsx-fae");

  var sources : SourceConfig.SourceCanisters = SourceConfig.init(initArgs);

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

  public query func get_source_canisters() : async { analytics : Principal; backend : Principal } {
    { analytics = sources.analytics; backend = sources.backend };
  };

  public shared({ caller }) func set_source_canister(name : Text, id : Principal) : async { #Ok; #Err : Text } {
    if (caller != admin) {
      return #Err("unauthorized: only admin can update source canisters");
    };
    switch (SourceConfig.update(sources, name, id)) {
      case (#ok) #Ok;
      case (#err msg) #Err msg;
    };
  };

};
