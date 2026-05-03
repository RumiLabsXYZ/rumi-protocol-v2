import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";

module {

  public func defaultHealth() : T.HealthSummaryDTO {
    {
      level = #Yellow;
      message = "Initializing — cache not yet seeded.";
      analytics_cursor_lag_seconds = 0 : Nat64;
      any_breaker_tripped = false;
      protocol_mode = #GeneralAvailability;
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
