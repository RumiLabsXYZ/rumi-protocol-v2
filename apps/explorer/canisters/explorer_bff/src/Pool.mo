import Principal "mo:core/Principal";
import Array "mo:core/Array";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import Nat8 "mo:core/Nat8";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func defaultPool(pool_id : Text) : T.PoolDetailDTO {
    {
      pool_id = pool_id;
      pool_label = "Unknown pool";
      pool_kind = "unknown";
      reserves = [];
      lp_total_supply = Format.e8s(0, 8, "");
      virtual_price = null;
      recent_events = [];
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetch(sources : SourceConfig.SourceCanisters, pool_id : Text) : async T.PoolDetailDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let state_opt = await analyticsActor.get_pool_state(pool_id);

    switch (state_opt) {
      case null defaultPool(pool_id);
      case (?state) {
        let reserves = Array.map<(Principal, Nat64, Nat8), (Principal, T.FormattedNumber)>(
          state.reserves,
          func((p, amt, decs)) { (p, Format.e8s(amt, decs, "")) }
        );
        {
          pool_id = state.pool_id;
          pool_label = state.pool_label;
          pool_kind = state.pool_kind;
          reserves = reserves;
          lp_total_supply = Format.e8s(state.lp_total_supply_e8s, 8, "LP");
          virtual_price = state.virtual_price;
          recent_events = [];
          generated_at_ns = Nat64.fromIntWrap(Time.now());
        };
      };
    };
  };

};
