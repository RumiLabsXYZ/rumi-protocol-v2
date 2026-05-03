import Principal "mo:core/Principal";
import Array "mo:core/Array";
import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func mapStatus(s : ST.VaultStatus) : T.VaultStatus {
    switch s {
      case (#Open) #Open;
      case (#Closed) #Closed;
      case (#Liquidated) #Liquidated;
    };
  };

  func mapEvent(ev : ST.EventSummary) : T.EventRowDTO {
    {
      source = "backend";
      source_event_id = ev.global_index;
      global_id = "backend:" # debug_show(ev.global_index);
      kind = ev.kind;
      timestamp_ns = ev.timestamp_ns;
      primary_principal = ev.primary_principal;
      primary_amount = switch (ev.amount_e8s) {
        case null null;
        case (?amt) ?Format.e8s(amt, 8, "");
      };
      secondary_principal = null;
      approximate = false;
      payload_summary = ev.payload_summary;
    };
  };

  // Synthesize a vault summary from history for closed vaults where
  // get_vault_summary returned null.
  func synthesize(vault_id : Nat64, history : [ST.EventSummary]) : T.VaultDetailDTO {
    var status : T.VaultStatus = #Closed;

    for (ev in history.vals()) {
      if (ev.kind == "liquidation" or ev.kind == "partial_liquidation") {
        status := #Liquidated;
      };
    };

    {
      vault_id = vault_id;
      status = status;
      owner = Principal.fromText("aaaaa-aa");
      collateral_type = Principal.fromText("aaaaa-aa");
      collateral_amount = Format.e8s(0, 8, "");
      debt_icusd = Format.e8s(0, 8, "icUSD");
      collateral_ratio = null;
      history = Array.map<ST.EventSummary, T.EventRowDTO>(history, mapEvent);
      closed_synthesized = true;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetch(sources : SourceConfig.SourceCanisters, vault_id : Nat64) : async T.VaultDetailDTO {
    let backendActor = SA.backend(sources.backend);

    let summary_opt = await backendActor.get_vault_summary(vault_id);
    let history = await backendActor.get_vault_history(vault_id);

    switch (summary_opt) {
      case null synthesize(vault_id, history);
      case (?summary) {
        {
          vault_id = summary.vault_id;
          status = mapStatus(summary.status);
          owner = summary.owner;
          collateral_type = summary.collateral_type;
          collateral_amount = Format.e8s(summary.collateral_amount_e8s, 8, "");
          debt_icusd = Format.e8s(summary.debt_icusd_e8s, 8, "icUSD");
          collateral_ratio = summary.collateral_ratio;
          history = Array.map<ST.EventSummary, T.EventRowDTO>(history, mapEvent);
          closed_synthesized = false;
          generated_at_ns = Nat64.fromIntWrap(Time.now());
        };
      };
    };
  };

};
