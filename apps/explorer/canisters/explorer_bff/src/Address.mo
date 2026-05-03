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

  func mapTokenBalance(b : ST.TokenBalance) : T.TokenBalanceDTO {
    {
      ledger = b.ledger;
      symbol = b.symbol;
      balance = Format.e8s(b.balance_e8s, b.decimals, b.symbol);
      value_usd = null;
    };
  };

  func mapSpDeposit(d : ST.SpDeposit) : T.SpDepositDTO {
    {
      total_deposited = Format.e8s(d.total_deposited_e8s, 8, "icUSD");
      current_balance = Format.e8s(d.current_balance_e8s, 8, "icUSD");
      earned_collateral = Array.map<(Principal, Nat64), (Principal, T.FormattedNumber)>(
        d.earned_collateral,
        func((p, amt)) { (p, Format.e8s(amt, 8, "")) }
      );
    };
  };

  func vaultIdToSummary(id : Nat64) : T.VaultSummaryDTO {
    // Minimal summary with raw id. Plan 5+ can fan out get_vault_summary calls
    // if richer data is needed here.
    {
      vault_id = id;
      status = #Open;
      collateral_type = Principal.fromText("aaaaa-aa");
      collateral_amount = Format.e8s(0, 8, "");
      debt_icusd = Format.e8s(0, 8, "icUSD");
      collateral_ratio = null;
    };
  };

  public func fetch(sources : SourceConfig.SourceCanisters, p : Principal) : async T.AddressDTO {
    let analyticsActor = SA.analytics(sources.analytics);
    let backendActor = SA.backend(sources.backend);

    let holdings = await analyticsActor.get_address_holdings(p);
    let events_resp = await backendActor.get_events_filtered({
      start = 0 : Nat64;
      length = 25 : Nat64;
      types = null;
      principal = ?p;
      collateral_token = null;
      time_range = null;
      min_size_e8s = null;
      admin_labels = null;
    });

    {
      owner = p;
      vaults_owned = Array.map<Nat64, T.VaultSummaryDTO>(holdings.vaults_owned_ids, vaultIdToSummary);
      sp_deposits = Array.map<ST.SpDeposit, T.SpDepositDTO>(holdings.sp_deposits, mapSpDeposit);
      amm_lp_positions = [];
      token_balances = Array.map<ST.TokenBalance, T.TokenBalanceDTO>(holdings.token_balances, mapTokenBalance);
      recent_events = Array.map<ST.EventSummary, T.EventRowDTO>(events_resp.events, mapEvent);
      total_value_usd = holdings.total_value_usd;
      approximate_sources = [];
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
