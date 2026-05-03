import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import SourceConfig "SourceConfig";

module {

  // Address detail requires get_address_holdings on rumi_analytics which is not
  // present on the mainnet canister. Returns a graceful empty DTO with
  // approximate_sources flagged so the frontend can surface the warning.
  //
  // This module is called from main.mo which wraps it in a try/catch as well,
  // so even if the actor resolution fails, it degrades safely.
  public func fetch(sources : SourceConfig.SourceCanisters, p : Principal) : async T.AddressDTO {
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
