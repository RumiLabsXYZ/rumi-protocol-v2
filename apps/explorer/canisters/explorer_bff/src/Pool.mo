import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  // Pool detail requires get_pool_state on rumi_analytics which is not
  // present on the mainnet canister. Returns a graceful empty DTO.
  // main.mo wraps this in try/catch as an extra safety net.
  public func fetch(_sources : SourceConfig.SourceCanisters, pool_id : Text) : async T.PoolDetailDTO {
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

};
