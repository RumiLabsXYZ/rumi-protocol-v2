import Array "mo:core/Array";
import Text "mo:core/Text";
import Nat32 "mo:core/Nat32";
import Nat64 "mo:core/Nat64";
import Char "mo:core/Char";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import Format "Format";
import SourceConfig "SourceConfig";

module {

  func textToEventTypeFilter(s : Text) : ?ST.EventTypeFilter {
    switch s {
      case "open_vault" ?#OpenVault;
      case "close_vault" ?#CloseVault;
      case "adjust_vault" ?#AdjustVault;
      case "borrow" ?#Borrow;
      case "repay" ?#Repay;
      case "liquidation" ?#Liquidation;
      case "partial_liquidation" ?#PartialLiquidation;
      case "redemption" ?#Redemption;
      case "reserve_redemption" ?#ReserveRedemption;
      case "stability_pool_deposit" ?#StabilityPoolDeposit;
      case "stability_pool_withdraw" ?#StabilityPoolWithdraw;
      case "admin_mint" ?#AdminMint;
      case "admin_sweep_to_treasury" ?#AdminSweepToTreasury;
      case "admin" ?#Admin;
      case "price_update" ?#PriceUpdate;
      case "accrue_interest" ?#AccrueInterest;
      case _ null;
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

  // Parse a cursor like "backend:42" -> ?42
  // Finds the ':' manually then parses the digits after it.
  func parseStartCursor(cursor : T.ActivityCursor) : Nat64 {
    switch (cursor.before_global_id) {
      case null 0 : Nat64;
      case (?cursorText) {
        var colonPos : ?Nat = null;
        var idx : Nat = 0;
        for (c in cursorText.chars()) {
          if (c == ':' and colonPos == null) colonPos := ?idx;
          idx += 1;
        };
        switch (colonPos) {
          case null 0;
          case (?pos) {
            var n : Nat64 = 0;
            var i : Nat = 0;
            var any = false;
            for (c in cursorText.chars()) {
              if (i > pos) {
                let cp : Nat32 = Char.toNat32(c);
                if (cp >= 48 and cp <= 57) {
                  n := n * 10 + Nat64.fromNat(Nat32.toNat(cp - 48));
                  any := true;
                } else { return 0 };
              };
              i += 1;
            };
            if (any) n else 0;
          };
        };
      };
    };
  };

  public func fetch(
    sources : SourceConfig.SourceCanisters,
    filter : T.ActivityFilter,
    cursor : T.ActivityCursor,
  ) : async T.ActivityFeedDTO {
    let backendActor = SA.backend(sources.backend);

    let typeFilters : ?[ST.EventTypeFilter] = switch (filter.types) {
      case null null;
      case (?ts) {
        let mapped = Array.filterMap<Text, ST.EventTypeFilter>(ts, textToEventTypeFilter);
        if (mapped.size() == 0) null else ?mapped;
      };
    };

    let timeRange : ?ST.TimeRange = switch (filter.from_ns, filter.to_ns) {
      case (null, null) null;
      case (from, to) {
        let f : Nat64 = switch from { case null 0; case (?v) v };
        let t : Nat64 = switch to { case null 18_446_744_073_709_551_615; case (?v) v };
        ?{ from_ns = f; to_ns = t };
      };
    };

    let start = parseStartCursor(cursor);
    let length : Nat64 = Nat64.fromNat(Nat32.toNat(cursor.page_size));

    let resp = await backendActor.get_events_filtered({
      start = start;
      length = length;
      types = typeFilters;
      principal = filter.filter_principal;
      collateral_token = null;
      time_range = timeRange;
      min_size_e8s = null;
      admin_labels = null;
    });

    let events = Array.map<ST.EventSummary, T.EventRowDTO>(resp.events, mapEvent);

    let endIdx : Nat64 = start + Nat64.fromNat(resp.events.size());
    let next_cursor : ?Text =
      if (endIdx >= resp.total) null
      else ?("backend:" # debug_show(endIdx));

    {
      events = events;
      next_cursor = next_cursor;
      total_estimated = resp.total;
      filters_applied = filter;
    };
  };

};
