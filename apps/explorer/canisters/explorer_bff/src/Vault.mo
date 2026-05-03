import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  // Vault detail requires get_vault_summary and get_vault_history on the backend.
  // The real backend returns a full Event variant (not EventSummary), so these calls
  // trap on mainnet. Returns a graceful empty synthesized DTO.
  // main.mo wraps this in try/catch as an extra safety net.
  public func fetch(_sources : SourceConfig.SourceCanisters, vault_id : Nat64) : async T.VaultDetailDTO {
    {
      vault_id = vault_id;
      status = #Closed;
      owner = Principal.fromText("aaaaa-aa");
      collateral_type = Principal.fromText("aaaaa-aa");
      collateral_amount = Format.e8s(0, 8, "");
      debt_icusd = Format.e8s(0, 8, "icUSD");
      collateral_ratio = null;
      history = [];
      closed_synthesized = true;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
