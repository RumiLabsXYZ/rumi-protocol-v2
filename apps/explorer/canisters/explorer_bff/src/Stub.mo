import Principal "mo:core/Principal";
import T "Types";

module {

  func samplePrincipal() : Principal = Principal.fromText("tfesu-vyaaa-aaaap-qrd7a-cai");
  func sampleLedger() : Principal = Principal.fromText("t6bor-paaaa-aaaap-qrd5q-cai");

  func fmt(amount : Nat64, decimals : Nat8, formatted : Text) : T.FormattedNumber {
    {
      raw_e8s = amount;
      decimals = decimals;
      formatted = formatted;
    };
  };

  func sampleEvent(id : Nat64, kind : Text, summary : Text) : T.EventRowDTO {
    {
      source = "backend";
      source_event_id = id;
      global_id = "backend:" # debug_show(id);
      kind = kind;
      timestamp_ns = 1_730_000_000_000_000_000 + id * 60_000_000_000;
      primary_principal = ?samplePrincipal();
      primary_amount = ?fmt(123_45000000, 8, "123.45");
      secondary_principal = null;
      approximate = false;
      payload_summary = summary;
    };
  };

  public func health() : T.HealthSummaryDTO {
    {
      level = #Green;
      message = "All systems nominal (stub data).";
      analytics_cursor_lag_seconds = 12;
      any_breaker_tripped = false;
      protocol_mode = #GeneralAvailability;
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

  public func overview() : T.OverviewDTO {
    {
      tvl_usd = 1_234_567.89;
      icusd_supply = fmt(500_000_00000000, 8, "500,000.00");
      icusd_peg_usd = 1.0023;
      protocol_mode = #GeneralAvailability;
      vault_count_open = 142;
      recent_activity = [
        sampleEvent(1, "open_vault", "Vault #142 opened with 1.5 ICP collateral"),
        sampleEvent(2, "borrow", "Borrowed 50 icUSD from vault #142"),
        sampleEvent(3, "swap", "Swapped 100 icUSD for 99.97 ckUSDC on 3pool"),
      ];
      health = health();
      generated_at_ns = 1_730_000_000_000_000_000;
      cache_age_ms = 0;
    };
  };

  public func activity(filter : T.ActivityFilter, _cursor : T.ActivityCursor) : T.ActivityFeedDTO {
    {
      events = [
        sampleEvent(1, "open_vault", "Vault #142 opened"),
        sampleEvent(2, "borrow", "Borrowed 50 icUSD"),
        sampleEvent(3, "swap", "Swapped 100 icUSD"),
        sampleEvent(4, "stability_pool_deposit", "Deposited 1000 icUSD to SP"),
        sampleEvent(5, "redemption", "Redeemed 200 icUSD"),
      ];
      next_cursor = ?"backend:0";
      total_estimated = 5;
      filters_applied = filter;
    };
  };

  public func address(p : Principal) : T.AddressDTO {
    {
      principal = p;
      vaults_owned = [{
        vault_id = 142;
        status = #Open;
        collateral_type = sampleLedger();
        collateral_amount = fmt(1_50000000, 8, "1.50 ICP");
        debt_icusd = fmt(50_00000000, 8, "50.00 icUSD");
        collateral_ratio = ?2.85;
      }];
      sp_deposits = [{
        total_deposited = fmt(1000_00000000, 8, "1,000.00 icUSD");
        current_balance = fmt(987_50000000, 8, "987.50 icUSD");
        earned_collateral = [(sampleLedger(), fmt(0_05000000, 8, "0.05 ICP"))];
      }];
      amm_lp_positions = [];
      token_balances = [{
        ledger = sampleLedger();
        symbol = "icUSD";
        balance = fmt(250_00000000, 8, "250.00");
        value_usd = ?250.58;
      }];
      recent_events = [sampleEvent(1, "open_vault", "Vault #142 opened")];
      total_value_usd = 1_234.56;
      approximate_sources = [];
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

  public func vault(vault_id : Nat64) : T.VaultDetailDTO {
    {
      vault_id = vault_id;
      status = #Open;
      owner = samplePrincipal();
      collateral_type = sampleLedger();
      collateral_amount = fmt(1_50000000, 8, "1.50 ICP");
      debt_icusd = fmt(50_00000000, 8, "50.00 icUSD");
      collateral_ratio = ?2.85;
      history = [sampleEvent(1, "open_vault", "Vault opened")];
      closed_synthesized = false;
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

  public func pool(pool_id : Text) : T.PoolDetailDTO {
    {
      pool_id = pool_id;
      pool_label = "Rumi 3Pool";
      pool_kind = "stable";
      reserves = [
        (sampleLedger(), fmt(1_000_000_00000000, 8, "1,000,000.00")),
      ];
      lp_total_supply = fmt(2_500_000_00000000, 8, "2,500,000.00");
      virtual_price = ?1.0631;
      recent_events = [sampleEvent(1, "swap", "Stub swap event")];
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

  public func token(ledger : Principal) : T.TokenDetailDTO {
    {
      ledger = ledger;
      symbol = "icUSD";
      decimals = 8;
      total_supply = fmt(500_000_00000000, 8, "500,000.00");
      fee = fmt(10_000, 8, "0.0001");
      recent_transfers = [sampleEvent(1, "transfer", "Stub transfer")];
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

  public func event(global_id : Text) : T.EventDetailDTO {
    {
      global_id = global_id;
      source = "backend";
      source_event_id = 1;
      kind = "open_vault";
      timestamp_ns = 1_730_000_000_000_000_000;
      payload_summary = "Stub event detail";
      payload_json = "{\"stub\": true}";
      related_event_ids = [];
      generated_at_ns = 1_730_000_000_000_000_000;
    };
  };

};
