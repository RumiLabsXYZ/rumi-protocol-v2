import Principal "mo:core/Principal";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func defaultToken(ledger : Principal) : T.TokenDetailDTO {
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

  public func fetch(sources : SourceConfig.SourceCanisters, ledger : Principal) : async T.TokenDetailDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let meta_opt = await analyticsActor.get_token_metadata(ledger);
    switch (meta_opt) {
      case null defaultToken(ledger);
      case (?meta) {
        {
          ledger = meta.ledger;
          symbol = meta.symbol;
          decimals = meta.decimals;
          total_supply = Format.e8s(meta.total_supply_e8s, meta.decimals, meta.symbol);
          fee = Format.e8s(meta.fee_e8s, meta.decimals, meta.symbol);
          recent_transfers = [];
          generated_at_ns = Nat64.fromIntWrap(Time.now());
        };
      };
    };
  };

};
