import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  // Token detail requires get_token_metadata on rumi_analytics which is not
  // present on the mainnet canister. Returns a graceful empty DTO (symbol = "?").
  // main.mo wraps this in try/catch as an extra safety net.
  public func fetch(_sources : SourceConfig.SourceCanisters, ledger : Principal) : async T.TokenDetailDTO {
    {
      ledger = ledger;
      symbol = "?";
      decimals = 8;
      total_supply = Format.e8s(0, 8, "");
      fee = Format.e8s(0, 8, "");
      recent_transfers = [];
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
