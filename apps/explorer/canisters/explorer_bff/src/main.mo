import Principal "mo:core/Principal";
import Timer "mo:core/Timer";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
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
import Lens "Lens";

persistent actor class ExplorerBff(initArgs : SourceConfig.SourceCanistersInit) {

  // Admin principal — read from init args, defaulting to anonymous (2vxsx-fae) for
  // local development. On mainnet, callers MUST pass `admin = opt principal "<real>"`.
  let admin : Principal = switch (initArgs.admin) {
    case (?p) p;
    case null Principal.fromText("2vxsx-fae");
  };

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
    // Activity calls get_events_filtered which traps on mainnet because the real backend
    // returns a full Event variant (not EventSummary). Wrapped in try/catch so the
    // /activity page degrades gracefully instead of showing a raw error.
    try {
      await Activity.fetch(sources, filter, cursor)
    } catch (_e) {
      {
        events = [];
        next_cursor = null;
        total_estimated = 0;
        filters_applied = filter;
      };
    };
  };

  public func get_address(p : Principal) : async T.AddressDTO {
    // get_address_holdings does not exist on real rumi_analytics. Returns empty DTO.
    try { await Address.fetch(sources, p) } catch (_e) {
      {
        owner = p;
        vaults_owned = [];
        sp_deposits = [];
        amm_lp_positions = [];
        token_balances = [];
        recent_events = [];
        total_value_usd = 0.0;
        approximate_sources = ["entity_pages_pending_v2"];
        generated_at_ns = Nat64.fromIntWrap(Time.now());
      };
    };
  };

  public func get_vault(vault_id : Nat64) : async T.VaultDetailDTO {
    // get_vault_summary/get_vault_history return full Event variant on real backend.
    // Returns empty synthesized DTO when calls trap.
    try { await Vault.fetch(sources, vault_id) } catch (_e) {
      {
        vault_id = vault_id;
        status = #Closed;
        owner = Principal.fromText("aaaaa-aa");
        collateral_type = Principal.fromText("aaaaa-aa");
        collateral_amount = { raw_e8s = 0; decimals = 8; formatted = "0" };
        debt_icusd = { raw_e8s = 0; decimals = 8; formatted = "0 icUSD" };
        collateral_ratio = null;
        history = [];
        closed_synthesized = true;
        generated_at_ns = Nat64.fromIntWrap(Time.now());
      };
    };
  };

  public func get_pool(pool_id : Text) : async T.PoolDetailDTO {
    // get_pool_state does not exist on real rumi_analytics. Returns empty DTO.
    try { await Pool.fetch(sources, pool_id) } catch (_e) {
      {
        pool_id = pool_id;
        pool_label = "Unknown pool";
        pool_kind = "unknown";
        reserves = [];
        lp_total_supply = { raw_e8s = 0; decimals = 8; formatted = "0" };
        virtual_price = null;
        recent_events = [];
        generated_at_ns = Nat64.fromIntWrap(Time.now());
      };
    };
  };

  public func get_token(ledger : Principal) : async T.TokenDetailDTO {
    // get_token_metadata does not exist on real rumi_analytics. Returns empty DTO.
    try { await Token.fetch(sources, ledger) } catch (_e) {
      {
        ledger = ledger;
        symbol = "?";
        decimals = 8;
        total_supply = { raw_e8s = 0; decimals = 8; formatted = "0" };
        fee = { raw_e8s = 0; decimals = 8; formatted = "0" };
        recent_transfers = [];
        generated_at_ns = Nat64.fromIntWrap(Time.now());
      };
    };
  };

  public func get_event(global_id : Text) : async T.EventDetailDTO {
    // get_events_filtered traps on mainnet (full Event variant). Returns "not found" DTO.
    try { await Event.fetch(sources, global_id) } catch (_e) {
      {
        global_id = global_id;
        source = "backend";
        source_event_id = 0;
        kind = "unknown";
        timestamp_ns = 0;
        payload_summary = "Event detail not yet available (Event variant porting in progress)";
        payload_json = "{}";
        related_event_ids = [];
        generated_at_ns = Nat64.fromIntWrap(Time.now());
      };
    };
  };

  public query func get_source_canisters() : async { analytics : Principal; backend : Principal } {
    { analytics = sources.analytics; backend = sources.backend };
  };

  public func get_lens_collateral() : async T.CollateralLensDTO { await Lens.fetchCollateral(sources) };
  public func get_lens_stability_pool() : async T.StabilityPoolLensDTO { await Lens.fetchStabilityPool(sources) };
  public func get_lens_revenue() : async T.RevenueLensDTO { await Lens.fetchRevenue(sources) };
  public func get_lens_redemptions() : async T.RedemptionsLensDTO { await Lens.fetchRedemptions(sources) };
  public func get_lens_dex() : async T.DexLensDTO { await Lens.fetchDex(sources) };
  public func get_lens_admin() : async T.AdminLensDTO { await Lens.fetchAdmin(sources) };

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
