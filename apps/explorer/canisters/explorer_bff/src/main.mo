import Principal "mo:core/Principal";
import Timer "mo:core/Timer";
import T "Types";
import Stub "Stub";
import SourceConfig "SourceConfig";
import Overview "Overview";
import Activity "Activity";
import Address "Address";
import Vault "Vault";
import Pool "Pool";
import Token "Token";
import Event "Event";
import Cache "Cache";
import Health "Health";

persistent actor class ExplorerBff(initArgs : SourceConfig.SourceCanistersInit) {

  // Anonymous principal as default admin for local development.
  // Replace with real admin principal in Plan 6's mainnet config.
  let admin : Principal = Principal.fromText("2vxsx-fae");

  var sources : SourceConfig.SourceCanisters = SourceConfig.init(initArgs);

  // Cache: 30-second TTL on the overview snapshot.
  // transient so it is not included in upgrade migration (heap-only; rebuilds on next tick).
  transient let overview_cache = Cache.TtlCache<T.OverviewDTO>(30_000_000_000);

  // Schedule a recurring refresh every 30 seconds.
  ignore Timer.recurringTimer<system>(#seconds 30, func() : async () {
    try {
      let fresh = await Overview.fetch(sources);
      overview_cache.set(fresh);
    } catch (_e) {
      // Swallow; next tick retries. Stale cache continues serving.
    };
  });

  // Seed once on startup so the first user request doesn't wait 30 seconds.
  ignore Timer.setTimer<system>(#seconds 0, func() : async () {
    try {
      let fresh = await Overview.fetch(sources);
      overview_cache.set(fresh);
    } catch (_e) {};
  });

  public query func ping() : async Text {
    "explorer_bff is alive"
  };

  public query func get_health() : async T.HealthSummaryDTO {
    switch (overview_cache.getStale()) {
      case null Health.defaultHealth();
      case (?cached) cached.health;
    };
  };

  public query func get_overview() : async T.OverviewDTO {
    switch (overview_cache.getStale()) {
      case null Stub.overview();
      case (?cached) {
        let age = overview_cache.ageMs();
        {
          tvl_usd = cached.tvl_usd;
          icusd_supply = cached.icusd_supply;
          icusd_peg_usd = cached.icusd_peg_usd;
          protocol_mode = cached.protocol_mode;
          vault_count_open = cached.vault_count_open;
          recent_activity = cached.recent_activity;
          health = cached.health;
          generated_at_ns = cached.generated_at_ns;
          cache_age_ms = age;
        };
      };
    };
  };

  public func get_activity(filter : T.ActivityFilter, cursor : T.ActivityCursor) : async T.ActivityFeedDTO {
    await Activity.fetch(sources, filter, cursor)
  };

  public func get_address(p : Principal) : async T.AddressDTO {
    await Address.fetch(sources, p)
  };

  public func get_vault(vault_id : Nat64) : async T.VaultDetailDTO {
    await Vault.fetch(sources, vault_id)
  };

  public func get_pool(pool_id : Text) : async T.PoolDetailDTO {
    await Pool.fetch(sources, pool_id)
  };

  public func get_token(ledger : Principal) : async T.TokenDetailDTO {
    await Token.fetch(sources, ledger)
  };

  public func get_event(global_id : Text) : async T.EventDetailDTO {
    await Event.fetch(sources, global_id)
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
